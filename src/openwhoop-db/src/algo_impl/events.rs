//! DB queries for the `events` table.

use chrono::NaiveDateTime;
use openwhoop_entities::events;
use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::OnConflict,
};

use crate::DatabaseHandler;

impl DatabaseHandler {
    /// Insert an event. `(timestamp, event_id)` has a UNIQUE constraint,
    /// so a replayed packet becomes a no-op — no duplicate rows.
    pub async fn create_event(
        &self,
        timestamp: NaiveDateTime,
        event_id: i32,
        event_name: &str,
        raw_data: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        let model = events::ActiveModel {
            id: NotSet,
            timestamp: Set(timestamp),
            event_id: Set(event_id),
            event_name: Set(event_name.to_string()),
            raw_data: Set(raw_data),
            synced: NotSet,
        };

        events::Entity::insert(model)
            .on_conflict(
                OnConflict::columns([events::Column::Timestamp, events::Column::EventId])
                    .do_nothing()
                    .to_owned(),
            )
            .do_nothing()
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Pull events in a time range, ordered chronologically. Consumed by
    /// wear-period derivation (WristOn/WristOff pairs).
    pub async fn get_events_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<events::Model>> {
        Ok(events::Entity::find()
            .filter(events::Column::Timestamp.gte(start))
            .filter(events::Column::Timestamp.lte(end))
            .order_by_asc(events::Column::Timestamp)
            .all(&self.db)
            .await?)
    }

    /// Most-recent N events — for the snapshot.
    pub async fn get_recent_events(&self, limit: u64) -> anyhow::Result<Vec<events::Model>> {
        Ok(events::Entity::find()
            .order_by_desc(events::Column::Timestamp)
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
    async fn create_event_basic() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_event(dt(), 9, "WristOn", None).await.unwrap();
        let rows = db.get_recent_events(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id, 9);
        assert_eq!(rows[0].event_name, "WristOn");
    }

    #[tokio::test]
    async fn create_event_idempotent_on_duplicate() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_event(dt(), 9, "WristOn", None).await.unwrap();
        db.create_event(dt(), 9, "WristOn", None).await.unwrap();
        db.create_event(dt(), 9, "WristOn", None).await.unwrap();
        let rows = db.get_recent_events(10).await.unwrap();
        assert_eq!(rows.len(), 1, "UNIQUE(timestamp, event_id) should dedupe");
    }

    #[tokio::test]
    async fn create_event_different_ids_coexist() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_event(dt(), 9, "WristOn", None).await.unwrap();
        db.create_event(dt(), 10, "WristOff", None).await.unwrap();
        let rows = db.get_recent_events(10).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn get_events_in_range_filters() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let base = dt();
        for (offset, id) in [(0, 9), (3600, 10), (7200, 9)] {
            let t = base + chrono::Duration::seconds(offset);
            db.create_event(t, id, "Test", None).await.unwrap();
        }
        let in_range = db
            .get_events_in_range(base, base + chrono::Duration::seconds(3600))
            .await
            .unwrap();
        assert_eq!(in_range.len(), 2, "inclusive of both endpoints");
    }
}
