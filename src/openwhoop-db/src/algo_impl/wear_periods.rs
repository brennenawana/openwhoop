//! DB queries for the `wear_periods` table and the signals that feed
//! wear-period derivation.

use chrono::NaiveDateTime;
use openwhoop_algos::{
    SKIN_CONTACT_MERGE_GAP_SECS, SkinContactRun, WearEvent, WearPeriod, WearSource,
};
use openwhoop_codec::SensorData;
use openwhoop_entities::{events, heart_rate, wear_periods};
use sea_orm::{
    ActiveModelTrait, ActiveValue::{NotSet, Set}, ColumnTrait, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};

use crate::DatabaseHandler;

impl DatabaseHandler {
    /// Load WristOn (id=9) and WristOff (id=10) events in a time range
    /// as [`WearEvent`]s for wear-period derivation.
    pub async fn get_wrist_events(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<WearEvent>> {
        let rows = events::Entity::find()
            .filter(events::Column::Timestamp.gte(start))
            .filter(events::Column::Timestamp.lte(end))
            .filter(events::Column::EventId.is_in([9, 10]))
            .order_by_asc(events::Column::Timestamp)
            .all(&self.db)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| WearEvent {
                timestamp: r.timestamp,
                on: r.event_id == 9,
            })
            .collect())
    }

    /// Derive contiguous runs of skin contact from `heart_rate.sensor_data`
    /// in a time range. A run breaks on either (a) a sample with
    /// `skin_contact == 0`, or (b) a time gap between consecutive samples
    /// longer than `SKIN_CONTACT_MERGE_GAP_SECS`. The gap break matters
    /// because the strap stops sampling entirely when off-wrist on some
    /// firmware/generations — so without it, "no zero-sample ever appears"
    /// would extend a single run across multi-hour off-wrist gaps.
    pub async fn get_skin_contact_runs(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<SkinContactRun>> {
        let rows = heart_rate::Entity::find()
            .filter(heart_rate::Column::Time.gte(start))
            .filter(heart_rate::Column::Time.lte(end))
            .filter(heart_rate::Column::SensorData.is_not_null())
            .order_by_asc(heart_rate::Column::Time)
            .all(&self.db)
            .await?;

        let mut runs: Vec<SkinContactRun> = Vec::new();
        let mut current: Option<SkinContactRun> = None;

        for row in rows {
            let Some(json) = row.sensor_data else { continue };
            let Ok(sd) = serde_json::from_value::<SensorData>(json) else {
                continue;
            };
            if sd.skin_contact == 0 {
                if let Some(run) = current.take() {
                    runs.push(run);
                }
                continue;
            }
            match &mut current {
                None => {
                    current = Some(SkinContactRun {
                        start: row.time,
                        end: row.time,
                    });
                }
                Some(run) => {
                    let gap = (row.time - run.end).num_seconds();
                    if gap > SKIN_CONTACT_MERGE_GAP_SECS {
                        runs.push(run.clone());
                        *run = SkinContactRun {
                            start: row.time,
                            end: row.time,
                        };
                    } else {
                        run.end = row.time;
                    }
                }
            }
        }
        if let Some(run) = current {
            runs.push(run);
        }
        Ok(runs)
    }

    /// Write a wear period row. Caller computes duration from start/end.
    pub async fn create_wear_period(&self, period: &WearPeriod) -> anyhow::Result<()> {
        let duration = period.duration_minutes();
        let model = wear_periods::ActiveModel {
            id: NotSet,
            start: Set(period.start),
            end: Set(period.end),
            source: Set(period.source.as_str().to_string()),
            duration_minutes: Set(duration),
        };
        model.insert(&self.db).await?;
        Ok(())
    }

    /// Delete all wear_periods rows that overlap `[start, end]`. Used by
    /// [`update_wear_periods`] to make re-runs idempotent; without this,
    /// every sync appended a new copy of each period and downstream
    /// range sums over-counted by a growing multiple.
    pub async fn delete_wear_periods_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<u64> {
        let res = wear_periods::Entity::delete_many()
            .filter(wear_periods::Column::End.gte(start))
            .filter(wear_periods::Column::Start.lte(end))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected)
    }

    /// Wear periods overlapping a range. Used by downstream
    /// pipeline steps (daytime HRV, activity classifier) to gate
    /// window inclusion.
    pub async fn get_wear_periods_overlapping(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<wear_periods::Model>> {
        Ok(wear_periods::Entity::find()
            .filter(wear_periods::Column::End.gte(start))
            .filter(wear_periods::Column::Start.lte(end))
            .order_by_asc(wear_periods::Column::Start)
            .all(&self.db)
            .await?)
    }

    /// Sum wear minutes that fall inside `[start, end]`. Each intersecting
    /// wear period contributes only its overlap with the query range, not
    /// its full duration — so a wear period that spans multiple calendar
    /// days is prorated correctly when callers ask for a single day.
    pub async fn wear_minutes_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<f64> {
        let rows: Vec<(NaiveDateTime, NaiveDateTime)> = wear_periods::Entity::find()
            .filter(wear_periods::Column::End.gte(start))
            .filter(wear_periods::Column::Start.lte(end))
            .select_only()
            .column(wear_periods::Column::Start)
            .column(wear_periods::Column::End)
            .into_tuple()
            .all(&self.db)
            .await?;
        let total: f64 = rows
            .into_iter()
            .map(|(s, e)| {
                let clamped_start = s.max(start);
                let clamped_end = e.min(end);
                let delta = clamped_end - clamped_start;
                (delta.num_seconds() as f64 / 60.0).max(0.0)
            })
            .sum();
        Ok(total)
    }

    pub fn wear_source_from_str(s: &str) -> WearSource {
        match s {
            "events" => WearSource::Events,
            "skin_contact" => WearSource::SkinContact,
            _ => WearSource::Fused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 17)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    /// Insert a heart_rate row with a crafted sensor_data blob that
    /// only sets `skin_contact` (other fields zeroed/default). Used by
    /// the gap-split test below.
    async fn insert_hr_with_skin_contact(
        db: &DatabaseHandler,
        time: NaiveDateTime,
        skin_contact: u8,
    ) {
        use openwhoop_entities::heart_rate;
        use sea_orm::ActiveValue::Set;
        let sensor_json = serde_json::json!({
            "ppg_green": 0,
            "ppg_red_ir": 0,
            "spo2_red": 0,
            "spo2_ir": 0,
            "skin_temp_raw": 0,
            "ambient_light": 0,
            "led_drive_1": 0,
            "led_drive_2": 0,
            "resp_rate_raw": 0,
            "signal_quality": 0,
            "skin_contact": skin_contact,
            "accel_gravity": [0.0, 0.0, 1.0],
            "spo2_pct": null,
        });
        let model = heart_rate::ActiveModel {
            id: NotSet,
            bpm: Set(60),
            time: Set(time),
            rr_intervals: Set(String::new()),
            activity: NotSet,
            stress: NotSet,
            spo2: NotSet,
            skin_temp: NotSet,
            imu_data: NotSet,
            sensor_data: Set(Some(sensor_json)),
            synced: NotSet,
        };
        model.insert(&db.db).await.unwrap();
    }

    #[tokio::test]
    async fn get_skin_contact_runs_splits_on_time_gap() {
        // Reproduces the real-world bug where the strap stops sampling
        // entirely when off-wrist (no zero-readings, just a time gap).
        // Without the time-gap split, two separated wear periods get
        // collapsed into one continuous run that bridges the gap.
        let db = DatabaseHandler::new("sqlite::memory:").await;
        // First wear stretch: 12:00 - 12:02 (3 samples, 1s apart).
        insert_hr_with_skin_contact(&db, dt(), 70).await;
        insert_hr_with_skin_contact(&db, dt() + chrono::Duration::seconds(1), 198).await;
        insert_hr_with_skin_contact(&db, dt() + chrono::Duration::seconds(2), 197).await;
        // Off-wrist: no samples for 5 hours (strap stopped sampling).
        // Second wear stretch: 17:00 - 17:02.
        insert_hr_with_skin_contact(&db, dt() + chrono::Duration::hours(5), 70).await;
        insert_hr_with_skin_contact(
            &db,
            dt() + chrono::Duration::hours(5) + chrono::Duration::seconds(1),
            198,
        )
        .await;

        let runs = db
            .get_skin_contact_runs(dt() - chrono::Duration::hours(1), dt() + chrono::Duration::hours(7))
            .await
            .unwrap();
        assert_eq!(runs.len(), 2, "expected 2 runs split by gap, got: {runs:?}");
        assert_eq!(runs[0].start, dt());
        assert_eq!(runs[0].end, dt() + chrono::Duration::seconds(2));
        assert_eq!(runs[1].start, dt() + chrono::Duration::hours(5));
    }

    #[tokio::test]
    async fn get_wrist_events_filters_by_id() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_event(dt(), 9, "WristOn", None).await.unwrap();
        db.create_event(dt() + chrono::Duration::hours(2), 10, "WristOff", None)
            .await
            .unwrap();
        db.create_event(dt() + chrono::Duration::hours(3), 3, "BatteryLevel", None)
            .await
            .unwrap();
        let evs = db
            .get_wrist_events(dt() - chrono::Duration::hours(1), dt() + chrono::Duration::hours(10))
            .await
            .unwrap();
        assert_eq!(evs.len(), 2);
        assert!(evs[0].on);
        assert!(!evs[1].on);
    }

    #[tokio::test]
    async fn create_and_query_wear_period() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let p = WearPeriod {
            start: dt(),
            end: dt() + chrono::Duration::hours(2),
            source: WearSource::Events,
        };
        db.create_wear_period(&p).await.unwrap();
        let rows = db
            .get_wear_periods_overlapping(dt() - chrono::Duration::hours(1), dt() + chrono::Duration::hours(3))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "events");
        assert_eq!(rows[0].duration_minutes, 120.0);
    }

    #[tokio::test]
    async fn wear_minutes_in_range_sums() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_wear_period(&WearPeriod {
            start: dt(),
            end: dt() + chrono::Duration::minutes(30),
            source: WearSource::Events,
        })
        .await
        .unwrap();
        db.create_wear_period(&WearPeriod {
            start: dt() + chrono::Duration::hours(1),
            end: dt() + chrono::Duration::hours(1) + chrono::Duration::minutes(45),
            source: WearSource::SkinContact,
        })
        .await
        .unwrap();
        let total = db
            .wear_minutes_in_range(dt(), dt() + chrono::Duration::hours(3))
            .await
            .unwrap();
        assert!((total - 75.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn wear_minutes_in_range_prorates_multi_day_period() {
        // One period spans 72h total; queryig one 24h window must return
        // ~1440 min, not the full 4320 min of the row.
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let start = dt();
        let end = dt() + chrono::Duration::hours(72);
        db.create_wear_period(&WearPeriod {
            start,
            end,
            source: WearSource::Events,
        })
        .await
        .unwrap();

        // Query only the second calendar day.
        let q_start = dt() + chrono::Duration::hours(24);
        let q_end = dt() + chrono::Duration::hours(48);
        let total = db.wear_minutes_in_range(q_start, q_end).await.unwrap();
        assert!(
            (total - 24.0 * 60.0).abs() < 1e-6,
            "expected ~1440 min for a single-day slice of a 3-day period, got {total}",
        );
    }

    #[tokio::test]
    async fn wear_minutes_in_range_handles_partial_overlap() {
        // Period: [0h, 6h]. Query: [4h, 10h]. Overlap: 2h = 120 min.
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_wear_period(&WearPeriod {
            start: dt(),
            end: dt() + chrono::Duration::hours(6),
            source: WearSource::Events,
        })
        .await
        .unwrap();
        let total = db
            .wear_minutes_in_range(
                dt() + chrono::Duration::hours(4),
                dt() + chrono::Duration::hours(10),
            )
            .await
            .unwrap();
        assert!((total - 120.0).abs() < 1e-6, "got {total}");
    }

    #[tokio::test]
    async fn delete_wear_periods_in_range_removes_overlapping_rows() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        // One inside the range, one outside.
        db.create_wear_period(&WearPeriod {
            start: dt(),
            end: dt() + chrono::Duration::hours(2),
            source: WearSource::Events,
        })
        .await
        .unwrap();
        db.create_wear_period(&WearPeriod {
            start: dt() + chrono::Duration::days(5),
            end: dt() + chrono::Duration::days(5) + chrono::Duration::hours(2),
            source: WearSource::Events,
        })
        .await
        .unwrap();

        let deleted = db
            .delete_wear_periods_in_range(dt() - chrono::Duration::hours(1), dt() + chrono::Duration::hours(3))
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        let remaining = db
            .get_wear_periods_overlapping(dt() - chrono::Duration::days(1), dt() + chrono::Duration::days(10))
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
    }
}
