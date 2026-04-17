//! DB queries for the `wear_periods` table and the signals that feed
//! wear-period derivation.

use chrono::NaiveDateTime;
use openwhoop_algos::{SkinContactRun, WearEvent, WearPeriod, WearSource};
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

    /// Derive contiguous runs of `skin_contact = 1` from
    /// `heart_rate.sensor_data` in a time range. Allows up to
    /// `SKIN_CONTACT_MERGE_GAP_SECS` gap inside a run.
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
                    run.end = row.time;
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

    /// Sum wear minutes over a date range — snapshot field.
    pub async fn wear_minutes_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<f64> {
        let rows: Vec<f64> = wear_periods::Entity::find()
            .filter(wear_periods::Column::End.gte(start))
            .filter(wear_periods::Column::Start.lte(end))
            .select_only()
            .column(wear_periods::Column::DurationMinutes)
            .into_tuple()
            .all(&self.db)
            .await?;
        Ok(rows.iter().sum())
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
}
