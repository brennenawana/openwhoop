//! DB queries for the `sync_log` table.
//!
//! The logger is built around three calls:
//! - `begin_sync_attempt` — writes an `in_progress` row, returns its id
//! - `finish_sync_attempt` — updates that id with `success` + counts
//! - `fail_sync_attempt` — updates that id with `error` + message
//!
//! All three are fail-safe at the call site: if the sync fails, the
//! pipeline catches the error and writes `fail_sync_attempt`, but if
//! the write itself errors, the caller logs-and-continues — sync
//! completion MUST NOT depend on logger success.

use chrono::NaiveDateTime;
use openwhoop_entities::sync_log;
use sea_orm::{
    ActiveModelTrait, ActiveValue::{NotSet, Set}, EntityTrait, QueryOrder, QuerySelect,
};

use crate::DatabaseHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    Success,
    Error,
    Cancelled,
    Timeout,
    InProgress,
}

impl SyncOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncOutcome::Success => "success",
            SyncOutcome::Error => "error",
            SyncOutcome::Cancelled => "cancelled",
            SyncOutcome::Timeout => "timeout",
            SyncOutcome::InProgress => "in_progress",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SyncCounts {
    pub heart_rate_rows_added: i32,
    pub packets_downloaded: i32,
    pub sleep_cycles_created: i32,
}

impl DatabaseHandler {
    pub async fn begin_sync_attempt(
        &self,
        started_at: NaiveDateTime,
        trigger: Option<String>,
    ) -> anyhow::Result<i32> {
        let model = sync_log::ActiveModel {
            id: NotSet,
            attempt_started_at: Set(started_at),
            attempt_ended_at: Set(None),
            outcome: Set(SyncOutcome::InProgress.as_str().to_string()),
            error_message: Set(None),
            heart_rate_rows_added: Set(Some(0)),
            packets_downloaded: Set(Some(0)),
            sleep_cycles_created: Set(Some(0)),
            trigger: Set(trigger),
        };
        let inserted = model.insert(&self.db).await?;
        Ok(inserted.id)
    }

    pub async fn finish_sync_attempt(
        &self,
        id: i32,
        ended_at: NaiveDateTime,
        outcome: SyncOutcome,
        counts: SyncCounts,
    ) -> anyhow::Result<()> {
        let model = sync_log::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(id),
            attempt_started_at: NotSet,
            attempt_ended_at: Set(Some(ended_at)),
            outcome: Set(outcome.as_str().to_string()),
            error_message: NotSet,
            heart_rate_rows_added: Set(Some(counts.heart_rate_rows_added)),
            packets_downloaded: Set(Some(counts.packets_downloaded)),
            sleep_cycles_created: Set(Some(counts.sleep_cycles_created)),
            trigger: NotSet,
        };
        model.update(&self.db).await?;
        Ok(())
    }

    pub async fn fail_sync_attempt(
        &self,
        id: i32,
        ended_at: NaiveDateTime,
        error_message: String,
    ) -> anyhow::Result<()> {
        let model = sync_log::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(id),
            attempt_started_at: NotSet,
            attempt_ended_at: Set(Some(ended_at)),
            outcome: Set(SyncOutcome::Error.as_str().to_string()),
            error_message: Set(Some(error_message)),
            heart_rate_rows_added: NotSet,
            packets_downloaded: NotSet,
            sleep_cycles_created: NotSet,
            trigger: NotSet,
        };
        model.update(&self.db).await?;
        Ok(())
    }

    pub async fn get_recent_sync_log(
        &self,
        limit: u64,
    ) -> anyhow::Result<Vec<sync_log::Model>> {
        Ok(sync_log::Entity::find()
            .order_by_desc(sync_log::Column::AttemptStartedAt)
            .limit(limit)
            .all(&self.db)
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
    async fn begin_finish_success_path() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let id = db
            .begin_sync_attempt(dt(), Some("manual".to_string()))
            .await
            .unwrap();
        db.finish_sync_attempt(
            id,
            dt() + chrono::Duration::seconds(30),
            SyncOutcome::Success,
            SyncCounts {
                heart_rate_rows_added: 100,
                packets_downloaded: 500,
                sleep_cycles_created: 1,
            },
        )
        .await
        .unwrap();
        let rows = db.get_recent_sync_log(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, "success");
        assert_eq!(rows[0].heart_rate_rows_added, Some(100));
        assert_eq!(rows[0].trigger.as_deref(), Some("manual"));
    }

    #[tokio::test]
    async fn begin_fail_error_path() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let id = db.begin_sync_attempt(dt(), None).await.unwrap();
        db.fail_sync_attempt(
            id,
            dt() + chrono::Duration::seconds(5),
            "BLE timeout".to_string(),
        )
        .await
        .unwrap();
        let rows = db.get_recent_sync_log(10).await.unwrap();
        assert_eq!(rows[0].outcome, "error");
        assert_eq!(rows[0].error_message.as_deref(), Some("BLE timeout"));
    }

    #[tokio::test]
    async fn recent_orders_by_start_desc() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let _a = db.begin_sync_attempt(dt(), None).await.unwrap();
        let _b = db
            .begin_sync_attempt(dt() + chrono::Duration::hours(1), None)
            .await
            .unwrap();
        let rows = db.get_recent_sync_log(10).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].attempt_started_at > rows[1].attempt_started_at);
    }
}
