//! DB queries for the `alarm_history` table.

use chrono::NaiveDateTime;
use openwhoop_entities::alarm_history;
use sea_orm::{
    ActiveModelTrait, ActiveValue::{NotSet, Set}, EntityTrait, QueryOrder, QuerySelect,
};

use crate::DatabaseHandler;

/// What kind of alarm action happened. Matches the `action` column
/// (stored as plain text so future variants don't need a migration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmAction {
    Set,
    Fired,
    Cleared,
    Queried,
}

impl AlarmAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AlarmAction::Set => "set",
            AlarmAction::Fired => "fired",
            AlarmAction::Cleared => "cleared",
            AlarmAction::Queried => "queried",
        }
    }
}

impl DatabaseHandler {
    pub async fn create_alarm_entry(
        &self,
        action: AlarmAction,
        action_at: NaiveDateTime,
        scheduled_for: Option<NaiveDateTime>,
        enabled: Option<bool>,
    ) -> anyhow::Result<()> {
        let model = alarm_history::ActiveModel {
            id: NotSet,
            action: Set(action.as_str().to_string()),
            action_at: Set(action_at),
            scheduled_for: Set(scheduled_for),
            enabled: Set(enabled),
        };
        model.insert(&self.db).await?;
        Ok(())
    }

    pub async fn get_recent_alarms(&self, limit: u64) -> anyhow::Result<Vec<alarm_history::Model>> {
        Ok(alarm_history::Entity::find()
            .order_by_desc(alarm_history::Column::ActionAt)
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
    async fn writes_set_action() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let scheduled = dt() + chrono::Duration::hours(8);
        db.create_alarm_entry(AlarmAction::Set, dt(), Some(scheduled), Some(true))
            .await
            .unwrap();
        let rows = db.get_recent_alarms(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "set");
        assert_eq!(rows[0].scheduled_for, Some(scheduled));
        assert_eq!(rows[0].enabled, Some(true));
    }

    #[tokio::test]
    async fn writes_fired_action() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_alarm_entry(AlarmAction::Fired, dt(), None, None)
            .await
            .unwrap();
        let rows = db.get_recent_alarms(10).await.unwrap();
        assert_eq!(rows[0].action, "fired");
    }

    #[tokio::test]
    async fn writes_cleared_action() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_alarm_entry(AlarmAction::Cleared, dt(), None, Some(false))
            .await
            .unwrap();
        let rows = db.get_recent_alarms(10).await.unwrap();
        assert_eq!(rows[0].action, "cleared");
        assert_eq!(rows[0].enabled, Some(false));
    }

    #[tokio::test]
    async fn writes_queried_action() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_alarm_entry(AlarmAction::Queried, dt(), Some(dt()), Some(true))
            .await
            .unwrap();
        let rows = db.get_recent_alarms(10).await.unwrap();
        assert_eq!(rows[0].action, "queried");
    }

    #[tokio::test]
    async fn recent_orders_by_action_at_desc() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let t1 = dt();
        let t2 = dt() + chrono::Duration::hours(1);
        db.create_alarm_entry(AlarmAction::Set, t1, None, Some(true))
            .await
            .unwrap();
        db.create_alarm_entry(AlarmAction::Fired, t2, None, None)
            .await
            .unwrap();
        let rows = db.get_recent_alarms(10).await.unwrap();
        assert_eq!(rows[0].action_at, t2);
        assert_eq!(rows[1].action_at, t1);
    }
}
