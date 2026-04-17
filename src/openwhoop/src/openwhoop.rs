use btleplug::api::ValueNotification;
use chrono::{DateTime, Local, NaiveDateTime, TimeDelta};
use openwhoop_entities::packets;
use openwhoop_db::{DatabaseHandler, SearchHistory};
use openwhoop_codec::{
    Activity, HistoryReading, WhoopData, WhoopPacket,
    constants::{CMD_FROM_STRAP, CommandNumber, DATA_FROM_STRAP, EVENTS_FROM_STRAP, MetadataType},
};

use crate::{
    algo::{
        ActivityPeriod, MAX_SLEEP_PAUSE, SkinTempCalculator, SleepCycle, SpO2Calculator,
        StressCalculator, helpers::format_hm::FormatHM,
    },
    types::activities,
};

/// Translate the u32 unix-seconds carried by event packets into the
/// local-naive datetime the DB uses everywhere else.
fn unix_to_local(unix: u32) -> anyhow::Result<NaiveDateTime> {
    DateTime::from_timestamp(i64::from(unix), 0)
        .map(|d| d.with_timezone(&Local).naive_local())
        .ok_or_else(|| anyhow::anyhow!("unix timestamp {unix} out of range"))
}

/// Event-id → human name mapping (audit §3.2 of the BLE writeup).
/// Unknown IDs fall through to the call site's `Unknown(N)` formatting.
fn event_name(id: u8) -> &'static str {
    match id {
        3 => "BatteryLevel",
        5 => "External5vOn",
        6 => "External5vOff",
        7 => "ChargingOn",
        8 => "ChargingOff",
        9 => "WristOn",
        10 => "WristOff",
        14 => "DoubleTap",
        63 => "ExtendedBatteryInformation",
        68 => "RunAlarm",
        96 => "HighFreqSyncPrompt",
        _ => match CommandNumber::from_u8(id) {
            Some(CommandNumber::SendR10R11Realtime) => "SendR10R11Realtime",
            Some(CommandNumber::ToggleRealtimeHr) => "ToggleRealtimeHr",
            Some(CommandNumber::GetClock) => "GetClock",
            Some(CommandNumber::RebootStrap) => "RebootStrap",
            Some(CommandNumber::ToggleR7DataCollection) => "ToggleR7DataCollection",
            Some(CommandNumber::ToggleGenericHrProfile) => "ToggleGenericHrProfile",
            _ => "Unknown",
        },
    }
}

pub struct OpenWhoop {
    pub database: DatabaseHandler,
    pub packet: Option<WhoopPacket>,
    pub last_history_packet: Option<HistoryReading>,
    pub history_packets: Vec<HistoryReading>,
}

impl OpenWhoop {
    pub fn new(database: DatabaseHandler) -> Self {
        Self {
            database,
            packet: None,
            last_history_packet: None,
            history_packets: Vec::new(),
        }
    }

    pub async fn store_packet(
        &self,
        notification: ValueNotification,
    ) -> anyhow::Result<packets::Model> {
        let packet = self
            .database
            .create_packet(notification.uuid, notification.value)
            .await?;

        Ok(packet)
    }

    pub async fn handle_packet(
        &mut self,
        packet: packets::Model,
    ) -> anyhow::Result<Option<WhoopPacket>> {
        let data = match packet.uuid {
            DATA_FROM_STRAP => {
                let packet = if let Some(mut whoop_packet) = self.packet.take() {
                    // TODO: maybe not needed but it would be nice to handle packet length here
                    // so if next packet contains end of one and start of another it is handled

                    whoop_packet.data.extend_from_slice(&packet.bytes);

                    if whoop_packet.data.len() + 3 >= whoop_packet.size {
                        whoop_packet
                    } else {
                        self.packet = Some(whoop_packet);
                        return Ok(None);
                    }
                } else {
                    let packet = WhoopPacket::from_data(packet.bytes)?;
                    if packet.partial {
                        self.packet = Some(packet);
                        return Ok(None);
                    }
                    packet
                };

                let Ok(data) = WhoopData::from_packet(packet) else {
                    return Ok(None);
                };
                data
            }
            CMD_FROM_STRAP => {
                let packet = WhoopPacket::from_data(packet.bytes)?;

                let Ok(data) = WhoopData::from_packet(packet) else {
                    return Ok(None);
                };

                data
            }
            EVENTS_FROM_STRAP => {
                // EVENTS_FROM_STRAP carries the same packet wire format as
                // DATA/CMD but was previously dropped in this match. Routing
                // it through the existing parser produces `WhoopData::Event`,
                // `UnknownEvent`, or `RunAlarm`, which handle_data now writes
                // to the events table.
                let packet = WhoopPacket::from_data(packet.bytes)?;
                let Ok(data) = WhoopData::from_packet(packet) else {
                    return Ok(None);
                };
                data
            }
            _ => return Ok(None),
        };

        self.handle_data(data).await
    }

    async fn handle_data(&mut self, data: WhoopData) -> anyhow::Result<Option<WhoopPacket>> {
        match data {
            WhoopData::HistoryReading(hr) if hr.is_valid() => {
                if let Some(last_packet) = self.last_history_packet.as_mut() {
                    if last_packet.unix == hr.unix && last_packet.bpm == hr.bpm {
                        return Ok(None);
                    } else {
                        last_packet.unix = hr.unix;
                        last_packet.bpm = hr.bpm;
                    }
                } else {
                    self.last_history_packet = Some(hr.clone());
                }

                let ptime = DateTime::from_timestamp_millis(i64::try_from(hr.unix)?)
                    .unwrap()
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S");

                if hr.imu_data.is_empty() {
                    info!(target: "HistoryReading", "time: {}", ptime);
                } else {
                    info!(target: "HistoryReading", "time: {}, (IMU)", ptime);
                }

                self.history_packets.push(hr);
            }
            WhoopData::HistoryMetadata { data, cmd, .. } => match cmd {
                MetadataType::HistoryComplete => {}
                MetadataType::HistoryStart => {}
                MetadataType::HistoryEnd => {
                    self.database
                        .create_readings(std::mem::take(&mut self.history_packets))
                        .await?;

                    let packet = WhoopPacket::history_end(data);
                    return Ok(Some(packet));
                }
            },
            WhoopData::ConsoleLog { log, .. } => {
                trace!(target: "ConsoleLog", "{}", log);
            }
            WhoopData::RunAlarm { unix } => {
                let ts = unix_to_local(unix)?;
                let _ = self
                    .database
                    .create_event(ts, 68, "RunAlarm", None)
                    .await;
                // Feature 3: mirror the fired alarm to alarm_history for
                // easy querying alongside set/cleared lifecycle rows.
                let _ = self
                    .database
                    .create_alarm_entry(
                        openwhoop_db::AlarmAction::Fired,
                        ts,
                        Some(ts),
                        Some(true),
                    )
                    .await;
            }
            WhoopData::AlarmInfo { enabled, unix } => {
                let now = Local::now().naive_local();
                let scheduled = unix_to_local(unix).ok();
                let _ = self
                    .database
                    .create_alarm_entry(
                        openwhoop_db::AlarmAction::Queried,
                        now,
                        scheduled,
                        Some(enabled),
                    )
                    .await;
            }
            WhoopData::Event { unix, event } => {
                let ts = unix_to_local(unix)?;
                let id = event.as_u8() as i32;
                let name = event_name(event.as_u8());
                let _ = self.database.create_event(ts, id, name, None).await;
            }
            WhoopData::UnknownEvent { unix, event } => {
                let ts = unix_to_local(unix)?;
                let id = event as i32;
                let name = format!("Unknown({})", event);
                let _ = self.database.create_event(ts, id, &name, None).await;
            }
            WhoopData::VersionInfo { harvard, boylston } => {
                info!("version harvard {} boylston {}", harvard, boylston);
                let now = Local::now().naive_local();
                let _ = self
                    .database
                    .create_device_info(now, Some(harvard), Some(boylston), None)
                    .await;
            }
            _ => {}
        }

        Ok(None)
    }

    pub async fn get_latest_sleep(&self) -> anyhow::Result<Option<SleepCycle>> {
        Ok(self.database.get_latest_sleep().await?.map(map_sleep_cycle))
    }

    pub async fn detect_events(&self) -> anyhow::Result<()> {
        let latest_activity = self.database.get_latest_activity().await?;
        let start_from = latest_activity.map(|a| a.from);

        let sleeps = self
            .database
            .get_sleep_cycles(start_from)
            .await?
            .windows(2)
            .map(|sleep| (sleep[0].id, sleep[0].end, sleep[1].start))
            .collect::<Vec<_>>();

        for (cycle_id, start, end) in sleeps {
            let options = SearchHistory {
                from: Some(start),
                to: Some(end),
                ..Default::default()
            };

            let history = self.database.search_history(options).await?;
            let events = ActivityPeriod::detect_from_gravity(&history);

            // NOTE: `detect_from_gravity` is a sleep/wake detector —
            // it labels any non-still run as `Active`. That's not a
            // workout. Previously we wrote every `Active` run as an
            // `ActivityType::Activity` row, which the tray surfaces
            // as "Workouts" — so a whole waking day between two
            // sleep cycles became one 18h "workout" (false positive).
            //
            // Workouts are now derived from `activity_samples` via
            // the rule-v0 classifier (`classify_activities`); this
            // pass only records **Nap** events (short still runs
            // during the waking day — a legitimate signal from the
            // gravity detector).
            for event in events {
                if !matches!(event.activity, Activity::Sleep) {
                    continue;
                }

                let duration = event.end - event.start;
                let activity = activities::ActivityPeriod {
                    period_id: cycle_id,
                    from: event.start,
                    to: event.end,
                    activity: activities::ActivityType::Nap,
                };

                info!(
                    "Detected nap from: {} to: {}, duration: {}",
                    activity.from,
                    activity.to,
                    duration.format_hm()
                );
                self.database.create_activity(activity).await?;
            }
        }

        Ok(())
    }

    /// TODO: add handling for data splits
    pub async fn detect_sleeps(&self) -> anyhow::Result<()> {
        'a: loop {
            let last_sleep = self.get_latest_sleep().await?;

            let options = SearchHistory {
                from: last_sleep.map(|s| s.end),
                limit: Some(86400 * 2),
                ..Default::default()
            };

            let mut history = self.database.search_history(options).await?;
            let mut periods = ActivityPeriod::detect_from_gravity(&history);

            while let Some(mut sleep) = ActivityPeriod::find_sleep(&mut periods) {
                if let Some(last_sleep) = last_sleep {
                    let diff = sleep.start - last_sleep.end;

                    if diff < MAX_SLEEP_PAUSE {
                        history = self
                            .database
                            .search_history(SearchHistory {
                                from: Some(last_sleep.start),
                                to: Some(sleep.end),
                                ..Default::default()
                            })
                            .await?;

                        sleep.start = last_sleep.start;
                        sleep.duration = sleep.end - sleep.start;
                    } else {
                        let this_sleep_id = sleep.end.date();
                        let last_sleep_id = last_sleep.end.date();

                        if this_sleep_id == last_sleep_id {
                            if sleep.duration < last_sleep.duration() {
                                let nap = activities::ActivityPeriod {
                                    period_id: last_sleep.id,
                                    from: sleep.start,
                                    to: sleep.end,
                                    activity: activities::ActivityType::Nap,
                                };
                                self.database.create_activity(nap).await?;
                                continue;
                            } else {
                                let nap = activities::ActivityPeriod {
                                    period_id: last_sleep.id - TimeDelta::days(1),
                                    from: last_sleep.start,
                                    to: last_sleep.end,
                                    activity: activities::ActivityType::Nap,
                                };
                                self.database.create_activity(nap).await?;
                            }
                        }
                    }
                }

                let sleep_cycle = SleepCycle::from_event(sleep, &history)?;

                info!(
                    "Detected sleep from {} to {}, duration: {}",
                    sleep.start,
                    sleep.end,
                    sleep.duration.format_hm()
                );
                self.database.create_sleep(sleep_cycle).await?;
                continue 'a;
            }

            break;
        }

        Ok(())
    }

    /// Run the sleep-staging pipeline for every cycle that hasn't yet
    /// been classified (or whose classifier_version is "failed"). Also
    /// refreshes the user baseline if stale (>24 h since last write).
    /// Errors in a single cycle do not halt the pipeline — they mark
    /// the offending cycle as failed and the run continues.
    pub async fn stage_sleep(&self) -> anyhow::Result<()> {
        let result = crate::sleep_staging::stage_unclassified(&self.database).await?;
        info!(
            "sleep staging: considered={} succeeded={} failed={} baseline_refreshed={}",
            result.cycles_considered,
            result.cycles_succeeded,
            result.cycles_failed,
            result.baseline_refreshed
        );
        Ok(())
    }

    /// Feature 4: derive wear periods for the last 14 days from the
    /// events table (WristOn/Off) + heart_rate.sensor_data skin_contact.
    /// Idempotency: deletes existing wear_periods rows in the range
    /// before inserting fresh. Safe to re-run.
    pub async fn update_wear_periods(&self) -> anyhow::Result<()> {
        use openwhoop_algos::derive_wear_periods;
        let now = Local::now().naive_local();
        let window_start = now - TimeDelta::days(14);
        let window_end = now;
        let events = self
            .database
            .get_wrist_events(window_start, window_end)
            .await
            .unwrap_or_default();
        let runs = self
            .database
            .get_skin_contact_runs(window_start, window_end)
            .await
            .unwrap_or_default();
        let periods = derive_wear_periods(&events, &runs, window_start, window_end);
        info!("wear_periods: derived {} periods", periods.len());
        for p in &periods {
            if let Err(e) = self.database.create_wear_period(p).await {
                log::warn!("failed to persist wear period: {e:#}");
            }
        }
        Ok(())
    }

    /// Feature 5: compute daytime HRV samples for each wear period in
    /// the last 14 days, skipping windows that overlap sleep cycles.
    pub async fn compute_daytime_hrv(&self) -> anyhow::Result<()> {
        use openwhoop_algos::compute_daytime_hrv;
        let now = Local::now().naive_local();
        let window_start = now - TimeDelta::days(14);
        let window_end = now;
        let wear_periods = self
            .database
            .get_wear_periods_overlapping(window_start, window_end)
            .await
            .unwrap_or_default();
        let sleep_cycles = self.database.get_sleep_cycles(Some(window_start)).await?;
        let sleep_windows: Vec<(chrono::NaiveDateTime, chrono::NaiveDateTime)> =
            sleep_cycles.iter().map(|c| (c.start, c.end)).collect();
        let resting_hr = sleep_cycles
            .last()
            .map(|c| f64::from(c.min_bpm))
            .unwrap_or(60.0);
        let mut total = 0usize;
        for wp in &wear_periods {
            let readings = self
                .database
                .search_history(SearchHistory {
                    from: Some(wp.start),
                    to: Some(wp.end),
                    limit: None,
                })
                .await?;
            let samples = compute_daytime_hrv(wp.start, wp.end, &readings, &sleep_windows, resting_hr);
            for s in &samples {
                if let Err(e) = self.database.create_hrv_sample(s).await {
                    log::warn!("failed to persist hrv sample: {e:#}");
                }
            }
            total += samples.len();
        }
        info!("daytime hrv: {} samples across {} wear periods", total, wear_periods.len());
        Ok(())
    }

    /// Feature 7: rule-v0 activity classification in 1-minute windows
    /// across wear periods in the last 14 days, excluding sleep.
    pub async fn classify_activities(&self) -> anyhow::Result<()> {
        use openwhoop_algos::classify_activities;
        let now = Local::now().naive_local();
        let window_start = now - TimeDelta::days(14);
        let window_end = now;
        let wear_periods = self
            .database
            .get_wear_periods_overlapping(window_start, window_end)
            .await
            .unwrap_or_default();
        let sleep_cycles = self.database.get_sleep_cycles(Some(window_start)).await?;
        let sleep_windows: Vec<(chrono::NaiveDateTime, chrono::NaiveDateTime)> =
            sleep_cycles.iter().map(|c| (c.start, c.end)).collect();
        let mut total = 0usize;
        for wp in &wear_periods {
            let readings = self
                .database
                .search_history(SearchHistory {
                    from: Some(wp.start),
                    to: Some(wp.end),
                    limit: None,
                })
                .await?;
            let samples = classify_activities(wp.start, wp.end, &readings, &sleep_windows);
            for s in &samples {
                if let Err(e) = self.database.create_activity_sample(s).await {
                    log::warn!("failed to persist activity sample: {e:#}");
                }
            }
            total += samples.len();
        }
        info!("activity classification: {} samples across {} wear periods", total, wear_periods.len());
        Ok(())
    }

    pub async fn calculate_spo2(&self) -> anyhow::Result<()> {
        loop {
            let last = self.database.last_spo2_time().await?;
            let options = SearchHistory {
                from: last
                    .map(|t| t - TimeDelta::seconds(i64::try_from(SpO2Calculator::WINDOW_SIZE).unwrap_or(0))),
                to: None,
                limit: Some(86400),
            };

            let readings = self.database.search_sensor_readings(options).await?;
            if readings.is_empty() || readings.len() <= SpO2Calculator::WINDOW_SIZE {
                break;
            }

            let scores = readings
                .windows(SpO2Calculator::WINDOW_SIZE)
                .filter_map(SpO2Calculator::calculate);

            for score in scores {
                self.database.update_spo2_on_reading(score).await?;
            }
        }

        Ok(())
    }

    pub async fn calculate_skin_temp(&self) -> anyhow::Result<()> {
        loop {
            let readings = self
                .database
                .search_temp_readings(SearchHistory {
                    limit: Some(86400),
                    ..Default::default()
                })
                .await?;

            if readings.is_empty() {
                break;
            }

            for reading in &readings {
                if let Some(score) =
                    SkinTempCalculator::convert(reading.time, reading.skin_temp_raw)
                {
                    self.database.update_skin_temp_on_reading(score).await?;
                }
            }
        }

        Ok(())
    }

    pub async fn calculate_stress(&self) -> anyhow::Result<()> {
        loop {
            let last_stress = self.database.last_stress_time().await?;
            let options = SearchHistory {
                from: last_stress
                    .map(|t| t - TimeDelta::seconds(i64::try_from(StressCalculator::MIN_READING_PERIOD).unwrap_or(0))),
                to: None,
                limit: Some(86400),
            };

            let history = self.database.search_history(options).await?;
            if history.is_empty() || history.len() <= StressCalculator::MIN_READING_PERIOD {
                break;
            }

            let stress_scores = history
                .windows(StressCalculator::MIN_READING_PERIOD)
                .filter_map(StressCalculator::calculate_stress);

            for stress in stress_scores {
                self.database.update_stress_on_reading(stress).await?;
            }
        }

        Ok(())
    }
}

fn map_sleep_cycle(sleep: openwhoop_entities::sleep_cycles::Model) -> SleepCycle {
    SleepCycle {
        id: sleep.end.date(),
        start: sleep.start,
        end: sleep.end,
        min_bpm: sleep.min_bpm.try_into().unwrap(),
        max_bpm: sleep.max_bpm.try_into().unwrap(),
        avg_bpm: sleep.avg_bpm.try_into().unwrap(),
        min_hrv: sleep.min_hrv.try_into().unwrap(),
        max_hrv: sleep.max_hrv.try_into().unwrap(),
        avg_hrv: sleep.avg_hrv.try_into().unwrap(),
        score: sleep
            .score
            .unwrap_or_else(|| SleepCycle::sleep_score(sleep.start, sleep.end)),
    }
}

#[cfg(test)]
mod event_name_tests {
    use super::event_name;

    #[test]
    fn known_event_ids_map_to_audit_names() {
        assert_eq!(event_name(3), "BatteryLevel");
        assert_eq!(event_name(5), "External5vOn");
        assert_eq!(event_name(6), "External5vOff");
        assert_eq!(event_name(7), "ChargingOn");
        assert_eq!(event_name(8), "ChargingOff");
        assert_eq!(event_name(9), "WristOn");
        assert_eq!(event_name(10), "WristOff");
        assert_eq!(event_name(14), "DoubleTap");
        assert_eq!(event_name(63), "ExtendedBatteryInformation");
        assert_eq!(event_name(68), "RunAlarm");
        assert_eq!(event_name(96), "HighFreqSyncPrompt");
    }

    #[test]
    fn unknown_event_id_returns_unknown_marker() {
        assert_eq!(event_name(255), "Unknown");
        assert_eq!(event_name(200), "Unknown");
    }
}
