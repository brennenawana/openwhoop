//! DB queries for the `activity_samples` table.

use chrono::NaiveDateTime;
use openwhoop_algos::ActivitySample;
use openwhoop_entities::activity_samples;
use sea_orm::{
    ActiveModelTrait, ActiveValue::{NotSet, Set}, ColumnTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::DatabaseHandler;

impl DatabaseHandler {
    pub async fn create_activity_sample(&self, sample: &ActivitySample) -> anyhow::Result<()> {
        let model = activity_samples::ActiveModel {
            id: NotSet,
            window_start: Set(sample.window_start),
            window_end: Set(sample.window_end),
            classification: Set(sample.classification.as_str().to_string()),
            accel_magnitude_mean: Set(Some(sample.accel_magnitude_mean)),
            accel_magnitude_std: Set(Some(sample.accel_magnitude_std)),
            gyro_magnitude_mean: Set(Some(sample.gyro_magnitude_mean)),
            dominant_frequency_hz: Set(Some(sample.dominant_frequency_hz)),
            mean_hr: Set(Some(sample.mean_hr)),
        };
        model.insert(&self.db).await?;
        Ok(())
    }

    pub async fn get_activity_samples_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<activity_samples::Model>> {
        Ok(activity_samples::Entity::find()
            .filter(activity_samples::Column::WindowStart.gte(start))
            .filter(activity_samples::Column::WindowStart.lte(end))
            .order_by_asc(activity_samples::Column::WindowStart)
            .all(&self.db)
            .await?)
    }

    /// Delete all activity_samples rows whose `window_start` falls in
    /// `[start, end]`. Used by [`classify_activities`] to make re-runs
    /// idempotent; without this, every sync re-inserted copies of every
    /// minute-window and downstream sums over-counted by the number of
    /// syncs.
    pub async fn delete_activity_samples_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<u64> {
        let res = activity_samples::Entity::delete_many()
            .filter(activity_samples::Column::WindowStart.gte(start))
            .filter(activity_samples::Column::WindowStart.lte(end))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use openwhoop_algos::ActivityClass;

    fn dt() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 17)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn create_and_query_activity_sample() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let sample = ActivitySample {
            window_start: dt(),
            window_end: dt() + chrono::Duration::minutes(1),
            classification: ActivityClass::Moderate,
            accel_magnitude_mean: 1.2,
            accel_magnitude_std: 0.3,
            gyro_magnitude_mean: 45.0,
            dominant_frequency_hz: 2.5,
            mean_hr: 120.0,
        };
        db.create_activity_sample(&sample).await.unwrap();
        let rows = db
            .get_activity_samples_in_range(dt(), dt() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].classification, "moderate");
    }
}
