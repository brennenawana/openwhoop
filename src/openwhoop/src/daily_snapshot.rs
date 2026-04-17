//! Per-day rollup for the tray's "Today" card and anything else that
//! wants a single-call view of today's non-sleep data.
//!
//! Pure over the DB — just projects existing rows (events,
//! device_info, alarm_history, wear_periods, hrv_samples, sync_log,
//! activity_samples) into slim Serialize-friendly shapes.

use chrono::{Local, NaiveDateTime};
use openwhoop_db::DatabaseHandler;
use openwhoop_entities::{alarm_history, device_info, events, hrv_samples, sync_log};
use serde::Serialize;

/// The Today card's full payload. Fields default to empty/None when
/// the backing tables have no relevant rows.
#[derive(Debug, Clone, Serialize)]
pub struct DailySnapshot {
    /// Local-midnight boundary used for every "today" aggregate in
    /// this snapshot. The UI can show this as the "as of" timestamp.
    pub day_start: NaiveDateTime,
    pub generated_at: NaiveDateTime,

    pub today_wear_minutes: f64,
    pub today_hrv_samples: Vec<HrvSampleLite>,
    pub today_activity_breakdown: ActivityBreakdown,
    pub recent_events: Vec<EventLite>,
    pub device_info: Option<DeviceInfoLite>,
    pub alarm_history: Vec<AlarmLite>,
    pub recent_sync_log: Vec<SyncLogLite>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HrvSampleLite {
    pub window_start: NaiveDateTime,
    pub window_end: NaiveDateTime,
    pub rmssd: f64,
    pub mean_hr: f64,
    pub context: String,
}

impl From<&hrv_samples::Model> for HrvSampleLite {
    fn from(m: &hrv_samples::Model) -> Self {
        Self {
            window_start: m.window_start,
            window_end: m.window_end,
            rmssd: m.rmssd,
            mean_hr: m.mean_hr,
            context: m.context.clone(),
        }
    }
}

/// Minutes in each classification bucket across today's
/// activity_samples. Unknown epochs are counted separately so the
/// UI can surface "we couldn't tell" time honestly.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ActivityBreakdown {
    pub sedentary_min: f64,
    pub light_min: f64,
    pub moderate_min: f64,
    pub vigorous_min: f64,
    pub unknown_min: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventLite {
    pub timestamp: NaiveDateTime,
    pub event_id: i32,
    pub event_name: String,
}

impl From<&events::Model> for EventLite {
    fn from(m: &events::Model) -> Self {
        Self {
            timestamp: m.timestamp,
            event_id: m.event_id,
            event_name: m.event_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfoLite {
    pub recorded_at: NaiveDateTime,
    pub harvard_version: Option<String>,
    pub boylston_version: Option<String>,
    pub device_name: Option<String>,
}

impl From<&device_info::Model> for DeviceInfoLite {
    fn from(m: &device_info::Model) -> Self {
        Self {
            recorded_at: m.recorded_at,
            harvard_version: m.harvard_version.clone(),
            boylston_version: m.boylston_version.clone(),
            device_name: m.device_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AlarmLite {
    pub action: String,
    pub action_at: NaiveDateTime,
    pub scheduled_for: Option<NaiveDateTime>,
    pub enabled: Option<bool>,
}

impl From<&alarm_history::Model> for AlarmLite {
    fn from(m: &alarm_history::Model) -> Self {
        Self {
            action: m.action.clone(),
            action_at: m.action_at,
            scheduled_for: m.scheduled_for,
            enabled: m.enabled,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncLogLite {
    pub attempt_started_at: NaiveDateTime,
    pub attempt_ended_at: Option<NaiveDateTime>,
    pub outcome: String,
    pub error_message: Option<String>,
    pub heart_rate_rows_added: Option<i32>,
    pub sleep_cycles_created: Option<i32>,
    pub trigger: Option<String>,
}

impl From<&sync_log::Model> for SyncLogLite {
    fn from(m: &sync_log::Model) -> Self {
        Self {
            attempt_started_at: m.attempt_started_at,
            attempt_ended_at: m.attempt_ended_at,
            outcome: m.outcome.clone(),
            error_message: m.error_message.clone(),
            heart_rate_rows_added: m.heart_rate_rows_added,
            sleep_cycles_created: m.sleep_cycles_created,
            trigger: m.trigger.clone(),
        }
    }
}

/// Build today's snapshot. "Today" = local-midnight to now. All
/// individual queries are fail-soft — if one errors we log-warn and
/// substitute an empty result, so a partial snapshot is still usable.
pub async fn get_daily_snapshot(db: &DatabaseHandler) -> anyhow::Result<DailySnapshot> {
    let now = Local::now().naive_local();
    let day_start = now
        .date()
        .and_hms_opt(0, 0, 0)
        .unwrap_or(now);

    let today_wear_minutes = db
        .wear_minutes_in_range(day_start, now)
        .await
        .unwrap_or_else(|e| {
            log::warn!("wear_minutes query failed: {e:#}");
            0.0
        });

    let today_hrv_samples: Vec<HrvSampleLite> = db
        .get_hrv_samples_in_range(day_start, now)
        .await
        .unwrap_or_else(|e| {
            log::warn!("hrv_samples query failed: {e:#}");
            Vec::new()
        })
        .iter()
        .map(HrvSampleLite::from)
        .collect();

    let today_activity_breakdown = activity_breakdown(db, day_start, now).await;

    let recent_events: Vec<EventLite> = db
        .get_recent_events(50)
        .await
        .unwrap_or_else(|e| {
            log::warn!("recent_events query failed: {e:#}");
            Vec::new()
        })
        .iter()
        .map(EventLite::from)
        .collect();

    let device_info = db
        .latest_device_info()
        .await
        .unwrap_or(None)
        .as_ref()
        .map(DeviceInfoLite::from);

    let alarm_history: Vec<AlarmLite> = db
        .get_recent_alarms(10)
        .await
        .unwrap_or_else(|e| {
            log::warn!("recent_alarms query failed: {e:#}");
            Vec::new()
        })
        .iter()
        .map(AlarmLite::from)
        .collect();

    let recent_sync_log: Vec<SyncLogLite> = db
        .get_recent_sync_log(10)
        .await
        .unwrap_or_else(|e| {
            log::warn!("recent_sync_log query failed: {e:#}");
            Vec::new()
        })
        .iter()
        .map(SyncLogLite::from)
        .collect();

    Ok(DailySnapshot {
        day_start,
        generated_at: now,
        today_wear_minutes,
        today_hrv_samples,
        today_activity_breakdown,
        recent_events,
        device_info,
        alarm_history,
        recent_sync_log,
    })
}

/// Sum activity_samples minutes per classification bucket across the
/// given range. Window length is 1 minute per `rule-v0` classifier;
/// we treat each row as exactly 1 minute to avoid recomputing from
/// `window_end - window_start`.
async fn activity_breakdown(
    db: &DatabaseHandler,
    start: NaiveDateTime,
    end: NaiveDateTime,
) -> ActivityBreakdown {
    let samples = match db.get_activity_samples_in_range(start, end).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("activity_samples query failed: {e:#}");
            return ActivityBreakdown::default();
        }
    };
    let mut b = ActivityBreakdown::default();
    for s in samples {
        let duration = (s.window_end - s.window_start).num_seconds() as f64 / 60.0;
        match s.classification.as_str() {
            "sedentary" => b.sedentary_min += duration,
            "light" => b.light_min += duration,
            "moderate" => b.moderate_min += duration,
            "vigorous" => b.vigorous_min += duration,
            _ => b.unknown_min += duration,
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate};

    fn dt(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 18)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn empty_db_returns_zero_snapshot() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let snap = get_daily_snapshot(&db).await.unwrap();
        assert_eq!(snap.today_wear_minutes, 0.0);
        assert!(snap.today_hrv_samples.is_empty());
        assert_eq!(snap.today_activity_breakdown.sedentary_min, 0.0);
        assert!(snap.recent_events.is_empty());
        assert!(snap.device_info.is_none());
        assert!(snap.alarm_history.is_empty());
        assert!(snap.recent_sync_log.is_empty());
    }

    #[tokio::test]
    async fn snapshot_picks_up_device_info() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_device_info(
            dt(12, 0),
            Some("41.16.6.0".to_string()),
            Some("17.2.2.0".to_string()),
            None,
        )
        .await
        .unwrap();
        let snap = get_daily_snapshot(&db).await.unwrap();
        let info = snap.device_info.expect("device_info should be populated");
        assert_eq!(info.harvard_version.as_deref(), Some("41.16.6.0"));
    }

    #[tokio::test]
    async fn activity_breakdown_sums_by_classification() {
        // Seed two sedentary + one moderate minute directly — verifies
        // the mapping logic without exercising the classifier pipeline.
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let base = dt(10, 0);
        for (i, class) in ["sedentary", "sedentary", "moderate"].iter().enumerate() {
            let sample = openwhoop_algos::ActivitySample {
                window_start: base + Duration::minutes(i as i64),
                window_end: base + Duration::minutes(i as i64 + 1),
                classification: match *class {
                    "sedentary" => openwhoop_algos::ActivityClass::Sedentary,
                    "moderate" => openwhoop_algos::ActivityClass::Moderate,
                    _ => openwhoop_algos::ActivityClass::Unknown,
                },
                accel_magnitude_mean: 1.0,
                accel_magnitude_std: 0.0,
                gyro_magnitude_mean: 0.0,
                dominant_frequency_hz: 0.0,
                mean_hr: 60.0,
            };
            db.create_activity_sample(&sample).await.unwrap();
        }
        let b = activity_breakdown(&db, base - Duration::hours(1), base + Duration::hours(1)).await;
        assert!((b.sedentary_min - 2.0).abs() < 1e-9);
        assert!((b.moderate_min - 1.0).abs() < 1e-9);
        assert_eq!(b.light_min, 0.0);
    }
}
