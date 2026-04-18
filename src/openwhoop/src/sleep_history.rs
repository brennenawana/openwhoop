//! 14-night history rollup for the tray's History page.
//!
//! Two parallel views over the last N days:
//! - `nights` — per sleep cycle projection (score, stages, hypnogram,
//!   efficiency, sleep RMSSD). One entry per `sleep_cycles` row whose
//!   `start` falls in range.
//! - `daily` — per calendar-day rollup of non-sleep data (wear
//!   minutes, daytime HRV average, activity breakdown). One entry per
//!   day in range, including empty days so the UI can render an
//!   aligned timeline.

use chrono::{Days, Local, NaiveDate, NaiveDateTime};
use openwhoop_db::DatabaseHandler;
use serde::Serialize;

use crate::daily_snapshot::ActivityBreakdown;
use crate::sleep_staging::{HypnogramEntry, SleepStageTotals};

/// Default window the tray asks for.
pub const DEFAULT_HISTORY_DAYS: u32 = 14;

#[derive(Debug, Clone, Serialize)]
pub struct SleepHistory {
    pub generated_at: NaiveDateTime,
    /// Inclusive calendar-date bounds of the window, in local time.
    pub range_start: NaiveDate,
    pub range_end: NaiveDate,
    pub nights: Vec<NightEntry>,
    pub daily: Vec<DailyRollup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NightEntry {
    /// `sleep_cycles.sleep_id` — the calendar date this night is
    /// associated with. Sleeps starting after midnight are still
    /// attributed to the prior evening by the detector.
    pub sleep_id: NaiveDate,
    pub sleep_start: NaiveDateTime,
    pub sleep_end: NaiveDateTime,
    pub performance_score: Option<f64>,
    pub stages: SleepStageTotals,
    pub hypnogram: Vec<HypnogramEntry>,
    pub sleep_efficiency: Option<f64>,
    pub total_sleep_minutes: Option<f64>,
    pub sleep_need_hours: Option<f64>,
    pub sleep_debt_hours: Option<f64>,
    pub avg_hrv: i32,
    pub classifier_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyRollup {
    pub date: NaiveDate,
    pub wear_minutes: f64,
    pub daytime_rmssd_avg: Option<f64>,
    pub daytime_rmssd_samples: i32,
    pub activity: ActivityBreakdown,
}

/// Build the history payload. `days` is the window length in days,
/// including today; `14` matches the Phase 3.1 plan. Fail-soft per
/// source — a query failure substitutes empty/None rather than
/// short-circuiting the whole payload.
pub async fn get_sleep_history(
    db: &DatabaseHandler,
    days: u32,
) -> anyhow::Result<SleepHistory> {
    let now = Local::now().naive_local();
    let today = now.date();
    let span = days.max(1);
    let range_start = today
        .checked_sub_days(Days::new(u64::from(span - 1)))
        .unwrap_or(today);
    let range_end = today;

    let window_start = range_start
        .and_hms_opt(0, 0, 0)
        .unwrap_or(now);
    let window_end = range_end
        .and_hms_opt(23, 59, 59)
        .unwrap_or(now);

    let nights = load_nights(db, window_start, window_end).await;
    let daily = load_daily_rollups(db, range_start, range_end).await;

    Ok(SleepHistory {
        generated_at: now,
        range_start,
        range_end,
        nights,
        daily,
    })
}

async fn load_nights(
    db: &DatabaseHandler,
    window_start: NaiveDateTime,
    window_end: NaiveDateTime,
) -> Vec<NightEntry> {
    let cycles = match db.get_sleep_cycles_in_range(window_start, window_end).await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("sleep_cycles range query failed: {e:#}");
            return Vec::new();
        }
    };

    let mut out = Vec::with_capacity(cycles.len());
    for cycle in cycles {
        let epochs = db
            .get_sleep_epochs_for_cycle(cycle.id)
            .await
            .unwrap_or_else(|e| {
                log::warn!(
                    "sleep_epochs query failed for cycle {}: {:#}",
                    cycle.id, e
                );
                Vec::new()
            });

        let epoch_stages: Vec<openwhoop_algos::sleep_staging::EpochStage> = epochs
            .iter()
            .map(|e| openwhoop_algos::sleep_staging::EpochStage {
                epoch_start: e.epoch_start,
                epoch_end: e.epoch_end,
                stage: openwhoop_algos::sleep_staging::SleepStage::parse(&e.stage)
                    .unwrap_or(openwhoop_algos::sleep_staging::SleepStage::Unknown),
                classifier_version: "",
            })
            .collect();

        let hypnogram: Vec<HypnogramEntry> =
            openwhoop_algos::sleep_staging::quantized_hypnogram(&epoch_stages)
                .into_iter()
                .map(|seg| HypnogramEntry {
                    start: seg.start,
                    end: seg.end,
                    stage: seg.stage.as_str().to_string(),
                })
                .collect();

        let stages = SleepStageTotals {
            awake_min: cycle.awake_minutes.unwrap_or(0.0),
            light_min: cycle.light_minutes.unwrap_or(0.0),
            deep_min: cycle.deep_minutes.unwrap_or(0.0),
            rem_min: cycle.rem_minutes.unwrap_or(0.0),
        };
        let total_sleep_minutes = match (cycle.light_minutes, cycle.deep_minutes, cycle.rem_minutes)
        {
            (Some(l), Some(d), Some(r)) => Some(l + d + r),
            _ => None,
        };

        out.push(NightEntry {
            sleep_id: cycle.sleep_id,
            sleep_start: cycle.start,
            sleep_end: cycle.end,
            performance_score: cycle.performance_score.or(cycle.score),
            stages,
            hypnogram,
            sleep_efficiency: cycle.sleep_efficiency,
            total_sleep_minutes,
            sleep_need_hours: cycle.sleep_need_hours,
            sleep_debt_hours: cycle.sleep_debt_hours,
            avg_hrv: cycle.avg_hrv,
            classifier_version: cycle.classifier_version.clone(),
        });
    }
    out
}

async fn load_daily_rollups(
    db: &DatabaseHandler,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> Vec<DailyRollup> {
    let total_days = (range_end - range_start).num_days().max(0) as u32 + 1;
    let mut out = Vec::with_capacity(total_days as usize);

    for i in 0..total_days {
        let date = range_start
            .checked_add_days(Days::new(u64::from(i)))
            .unwrap_or(range_end);
        let day_start = match date.and_hms_opt(0, 0, 0) {
            Some(v) => v,
            None => continue,
        };
        let day_end = match date.and_hms_opt(23, 59, 59) {
            Some(v) => v,
            None => continue,
        };

        let wear_minutes = db
            .wear_minutes_in_range(day_start, day_end)
            .await
            .unwrap_or_else(|e| {
                log::warn!("wear_minutes query failed for {date}: {e:#}");
                0.0
            });

        let hrv_samples = db
            .get_hrv_samples_in_range(day_start, day_end)
            .await
            .unwrap_or_else(|e| {
                log::warn!("hrv_samples query failed for {date}: {e:#}");
                Vec::new()
            });
        let daytime_rmssd_samples = hrv_samples.len() as i32;
        let daytime_rmssd_avg = if hrv_samples.is_empty() {
            None
        } else {
            Some(hrv_samples.iter().map(|s| s.rmssd).sum::<f64>() / hrv_samples.len() as f64)
        };

        let activity = activity_breakdown_for(db, day_start, day_end).await;

        out.push(DailyRollup {
            date,
            wear_minutes,
            daytime_rmssd_avg,
            daytime_rmssd_samples,
            activity,
        });
    }

    out
}

async fn activity_breakdown_for(
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

    #[tokio::test]
    async fn empty_db_returns_empty_history() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let h = get_sleep_history(&db, 14).await.unwrap();
        assert!(h.nights.is_empty());
        assert_eq!(h.daily.len(), 14);
        for d in &h.daily {
            assert_eq!(d.wear_minutes, 0.0);
            assert!(d.daytime_rmssd_avg.is_none());
            assert_eq!(d.daytime_rmssd_samples, 0);
            assert_eq!(d.activity.sedentary_min, 0.0);
        }
    }

    #[tokio::test]
    async fn daily_rollup_window_respects_days_arg() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let h = get_sleep_history(&db, 7).await.unwrap();
        assert_eq!(h.daily.len(), 7);
        // Dates are contiguous and end at today.
        for pair in h.daily.windows(2) {
            assert_eq!(
                (pair[1].date - pair[0].date).num_days(),
                1,
                "daily rollups should be consecutive days"
            );
        }
        assert_eq!(h.daily.last().unwrap().date, h.range_end);
        assert_eq!(h.daily.first().unwrap().date, h.range_start);
    }

    #[tokio::test]
    async fn activity_breakdown_sums_per_day() {
        use chrono::Duration;
        use openwhoop_algos::{ActivityClass, ActivitySample};

        let db = DatabaseHandler::new("sqlite::memory:").await;
        let today = Local::now().naive_local().date();
        let base = today.and_hms_opt(12, 0, 0).unwrap();
        for (i, class) in [ActivityClass::Moderate, ActivityClass::Moderate].iter().enumerate() {
            let s = ActivitySample {
                window_start: base + Duration::minutes(i as i64),
                window_end: base + Duration::minutes(i as i64 + 1),
                classification: *class,
                accel_magnitude_mean: 1.5,
                accel_magnitude_std: 0.2,
                gyro_magnitude_mean: 0.1,
                dominant_frequency_hz: 1.0,
                mean_hr: 110.0,
            };
            db.create_activity_sample(&s).await.unwrap();
        }
        let h = get_sleep_history(&db, 14).await.unwrap();
        let today_entry = h.daily.iter().find(|d| d.date == today).unwrap();
        assert!((today_entry.activity.moderate_min - 2.0).abs() < 1e-9);
    }
}
