//! DB queries for the `hrv_samples` table.

use chrono::NaiveDateTime;
use openwhoop_algos::HrvSample;
use openwhoop_entities::hrv_samples;
use sea_orm::{
    ActiveModelTrait, ActiveValue::{NotSet, Set}, ColumnTrait, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::DatabaseHandler;

impl DatabaseHandler {
    pub async fn create_hrv_sample(&self, sample: &HrvSample) -> anyhow::Result<()> {
        let model = hrv_samples::ActiveModel {
            id: NotSet,
            window_start: Set(sample.window_start),
            window_end: Set(sample.window_end),
            rmssd: Set(sample.rmssd),
            sdnn: Set(sample.sdnn),
            mean_hr: Set(sample.mean_hr),
            rr_count: Set(sample.rr_count as i32),
            stillness_ratio: Set(sample.stillness_ratio),
            context: Set(sample.context.as_str().to_string()),
        };
        model.insert(&self.db).await?;
        Ok(())
    }

    pub async fn get_hrv_samples_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<hrv_samples::Model>> {
        Ok(hrv_samples::Entity::find()
            .filter(hrv_samples::Column::WindowStart.gte(start))
            .filter(hrv_samples::Column::WindowStart.lte(end))
            .order_by_asc(hrv_samples::Column::WindowStart)
            .all(&self.db)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use openwhoop_algos::HrvContext;

    fn dt() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 17)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn create_and_query_hrv_sample() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let sample = HrvSample {
            window_start: dt(),
            window_end: dt() + chrono::Duration::minutes(5),
            rmssd: 42.0,
            sdnn: Some(55.5),
            mean_hr: 65.0,
            rr_count: 280,
            stillness_ratio: 0.95,
            context: HrvContext::Resting,
        };
        db.create_hrv_sample(&sample).await.unwrap();
        let rows = db
            .get_hrv_samples_in_range(dt(), dt() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rmssd, 42.0);
        assert_eq!(rows[0].context, "resting");
    }
}
