//! DB queries for the `device_info` table.

use chrono::NaiveDateTime;
use openwhoop_entities::device_info;
use sea_orm::{
    ActiveModelTrait, ActiveValue::{NotSet, Set}, EntityTrait, QueryOrder,
};

use crate::DatabaseHandler;

impl DatabaseHandler {
    /// Record one device-info snapshot. Written on every successful
    /// `initialize()` — duplicates across runs are fine and let us see
    /// connection history (row count = connect count).
    pub async fn create_device_info(
        &self,
        recorded_at: NaiveDateTime,
        harvard_version: Option<String>,
        boylston_version: Option<String>,
        device_name: Option<String>,
    ) -> anyhow::Result<()> {
        let model = device_info::ActiveModel {
            id: NotSet,
            recorded_at: Set(recorded_at),
            harvard_version: Set(harvard_version),
            boylston_version: Set(boylston_version),
            device_name: Set(device_name),
        };
        model.insert(&self.db).await?;
        Ok(())
    }

    /// Most-recent device_info row — used by snapshot.
    pub async fn latest_device_info(&self) -> anyhow::Result<Option<device_info::Model>> {
        Ok(device_info::Entity::find()
            .order_by_desc(device_info::Column::RecordedAt)
            .one(&self.db)
            .await?)
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
    async fn create_and_fetch_latest() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_device_info(
            dt(),
            Some("41.16.6.0".to_string()),
            Some("17.2.2.0".to_string()),
            None,
        )
        .await
        .unwrap();
        let latest = db.latest_device_info().await.unwrap().unwrap();
        assert_eq!(latest.harvard_version.as_deref(), Some("41.16.6.0"));
        assert_eq!(latest.boylston_version.as_deref(), Some("17.2.2.0"));
        assert!(latest.device_name.is_none());
    }

    #[tokio::test]
    async fn latest_returns_most_recent() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let older = dt();
        let newer = dt() + chrono::Duration::hours(1);
        db.create_device_info(older, Some("A".to_string()), None, None)
            .await
            .unwrap();
        db.create_device_info(newer, Some("B".to_string()), None, None)
            .await
            .unwrap();
        let latest = db.latest_device_info().await.unwrap().unwrap();
        assert_eq!(latest.harvard_version.as_deref(), Some("B"));
    }

    #[tokio::test]
    async fn latest_none_on_empty_table() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        assert!(db.latest_device_info().await.unwrap().is_none());
    }
}
