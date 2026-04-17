//! DB queries for the `dev_notes` table — the agent ↔ dev async
//! channel backing the lab dashboard (see docs/DEV_DASHBOARD_CONCEPT.md).

use chrono::NaiveDateTime;
use openwhoop_entities::dev_notes;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::{NotSet, Set},
    ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};

use crate::DatabaseHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevNoteKind {
    /// Freeform update ("I worked on X, here's what I observed").
    Note,
    /// Awaiting dev response ("pick one of A/B/C").
    Question,
    /// Algorithm-tweak trial ("ran with threshold X, got Y").
    Experiment,
    /// Before/after delta ("Deep % was N, is now M").
    Diff,
    /// Status update on an ongoing workstream ("refresh_baseline_if_stale
    /// is wired but not yet pipeline-integrated").
    Status,
}

impl DevNoteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DevNoteKind::Note => "note",
            DevNoteKind::Question => "question",
            DevNoteKind::Experiment => "experiment",
            DevNoteKind::Diff => "diff",
            DevNoteKind::Status => "status",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "note" => Some(Self::Note),
            "question" => Some(Self::Question),
            "experiment" => Some(Self::Experiment),
            "diff" => Some(Self::Diff),
            "status" => Some(Self::Status),
            _ => None,
        }
    }
}

/// Builder-style convenience for agents writing notes. Keeps the
/// `DatabaseHandler::create_dev_note` signature compact.
#[derive(Debug, Clone, Default)]
pub struct DevNoteInput {
    pub author: Option<String>,        // defaults to "agent" if None
    pub kind: Option<DevNoteKind>,     // defaults to Note
    pub title: String,
    pub body_md: Option<String>,
    pub related_commit: Option<String>,
    pub related_feature: Option<String>,
    pub related_range_start: Option<NaiveDateTime>,
    pub related_range_end: Option<NaiveDateTime>,
    pub payload_json: Option<serde_json::Value>,
}

impl DevNoteInput {
    pub fn note(title: impl Into<String>) -> Self {
        Self {
            kind: Some(DevNoteKind::Note),
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn question(title: impl Into<String>) -> Self {
        Self {
            kind: Some(DevNoteKind::Question),
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body_md = Some(body.into());
        self
    }

    pub fn feature(mut self, f: impl Into<String>) -> Self {
        self.related_feature = Some(f.into());
        self
    }

    pub fn commit(mut self, sha: impl Into<String>) -> Self {
        self.related_commit = Some(sha.into());
        self
    }

    pub fn range(mut self, start: NaiveDateTime, end: NaiveDateTime) -> Self {
        self.related_range_start = Some(start);
        self.related_range_end = Some(end);
        self
    }
}

impl DatabaseHandler {
    /// Write one dev note. Returns the inserted row's id so the caller
    /// can reference it later (e.g. to auto-resolve on follow-up).
    pub async fn create_dev_note(
        &self,
        now: NaiveDateTime,
        input: DevNoteInput,
    ) -> anyhow::Result<i32> {
        let author = input.author.unwrap_or_else(|| "agent".to_string());
        let kind = input.kind.unwrap_or(DevNoteKind::Note).as_str().to_string();

        let model = dev_notes::ActiveModel {
            id: NotSet,
            created_at: Set(now),
            author: Set(author),
            kind: Set(kind),
            title: Set(input.title),
            body_md: Set(input.body_md),
            related_commit: Set(input.related_commit),
            related_feature: Set(input.related_feature),
            related_range_start: Set(input.related_range_start),
            related_range_end: Set(input.related_range_end),
            resolved_at: Set(None),
            resolved_by: Set(None),
            payload_json: Set(input.payload_json),
        };
        let inserted = model.insert(&self.db).await?;
        Ok(inserted.id)
    }

    /// Unresolved notes — the dashboard's "inbox".
    pub async fn list_unresolved_notes(
        &self,
        limit: u64,
    ) -> anyhow::Result<Vec<dev_notes::Model>> {
        Ok(dev_notes::Entity::find()
            .filter(dev_notes::Column::ResolvedAt.is_null())
            .order_by_desc(dev_notes::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await?)
    }

    /// All notes for a given feature (both resolved and not), newest first.
    pub async fn list_notes_for_feature(
        &self,
        feature: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<dev_notes::Model>> {
        Ok(dev_notes::Entity::find()
            .filter(dev_notes::Column::RelatedFeature.eq(feature))
            .order_by_desc(dev_notes::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await?)
    }

    /// Notes attached to a specific commit (either by SHA or "HEAD").
    pub async fn list_notes_for_commit(
        &self,
        commit_sha: &str,
    ) -> anyhow::Result<Vec<dev_notes::Model>> {
        Ok(dev_notes::Entity::find()
            .filter(dev_notes::Column::RelatedCommit.eq(commit_sha))
            .order_by_desc(dev_notes::Column::CreatedAt)
            .all(&self.db)
            .await?)
    }

    /// Recent activity across all notes, regardless of resolution state.
    /// Dashboard shows this as a timeline panel.
    pub async fn list_recent_notes(&self, limit: u64) -> anyhow::Result<Vec<dev_notes::Model>> {
        Ok(dev_notes::Entity::find()
            .order_by_desc(dev_notes::Column::CreatedAt)
            .limit(limit)
            .all(&self.db)
            .await?)
    }

    /// Mark a note resolved. `resolved_by` is typically "dev" when
    /// answered from the dashboard, "agent" when the agent self-resolves
    /// (e.g. shipping the thing the note flagged).
    pub async fn resolve_dev_note(
        &self,
        id: i32,
        resolved_at: NaiveDateTime,
        resolved_by: &str,
    ) -> anyhow::Result<()> {
        let model = dev_notes::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(id),
            resolved_at: Set(Some(resolved_at)),
            resolved_by: Set(Some(resolved_by.to_string())),
            ..Default::default()
        };
        model.update(&self.db).await?;
        Ok(())
    }

    /// Combined "open items" scoped to a feature — used by the dashboard
    /// per-feature panels.
    pub async fn list_open_notes_for_feature(
        &self,
        feature: &str,
        limit: u64,
    ) -> anyhow::Result<Vec<dev_notes::Model>> {
        Ok(dev_notes::Entity::find()
            .filter(
                Condition::all()
                    .add(dev_notes::Column::RelatedFeature.eq(feature))
                    .add(dev_notes::Column::ResolvedAt.is_null()),
            )
            .order_by_desc(dev_notes::Column::CreatedAt)
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
        NaiveDate::from_ymd_opt(2026, 4, 18)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn agent_writes_note_defaults_author_and_kind() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let id = db
            .create_dev_note(dt(), DevNoteInput::note("Deep HR issue spotted"))
            .await
            .unwrap();
        assert!(id > 0);
        let rows = db.list_recent_notes(5).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].author, "agent");
        assert_eq!(rows[0].kind, "note");
        assert_eq!(rows[0].title, "Deep HR issue spotted");
        assert!(rows[0].resolved_at.is_none());
    }

    #[tokio::test]
    async fn unresolved_filters_out_resolved() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let id1 = db
            .create_dev_note(dt(), DevNoteInput::note("a"))
            .await
            .unwrap();
        let _id2 = db
            .create_dev_note(dt(), DevNoteInput::note("b"))
            .await
            .unwrap();
        db.resolve_dev_note(id1, dt(), "dev").await.unwrap();
        let open = db.list_unresolved_notes(10).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "b");
    }

    #[tokio::test]
    async fn feature_scoping_works() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_dev_note(
            dt(),
            DevNoteInput::note("classifier tweak").feature("sleep_staging"),
        )
        .await
        .unwrap();
        db.create_dev_note(
            dt(),
            DevNoteInput::note("activity thresholds").feature("activity"),
        )
        .await
        .unwrap();

        let sleep_notes = db
            .list_notes_for_feature("sleep_staging", 10)
            .await
            .unwrap();
        assert_eq!(sleep_notes.len(), 1);
        assert_eq!(sleep_notes[0].title, "classifier tweak");
    }

    #[tokio::test]
    async fn question_with_body_roundtrips() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.create_dev_note(
            dt(),
            DevNoteInput::question("Pick Option 1, 2, or 3 for the Deep HR gate")
                .body("See docs/DEEP_HR_RESEARCH_PROMPT.md for the three approaches."),
        )
        .await
        .unwrap();
        let rows = db.list_unresolved_notes(10).await.unwrap();
        assert_eq!(rows[0].kind, "question");
        assert!(rows[0].body_md.as_deref().unwrap().contains("DEEP_HR"));
    }

    #[tokio::test]
    async fn resolve_flags_author_and_timestamp() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let id = db
            .create_dev_note(dt(), DevNoteInput::note("TODO"))
            .await
            .unwrap();
        let resolved_at = dt() + chrono::Duration::hours(2);
        db.resolve_dev_note(id, resolved_at, "dev").await.unwrap();
        let rows = db.list_recent_notes(5).await.unwrap();
        assert_eq!(rows[0].resolved_at, Some(resolved_at));
        assert_eq!(rows[0].resolved_by.as_deref(), Some("dev"));
    }

    #[tokio::test]
    async fn open_notes_scoped_to_feature() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let id1 = db
            .create_dev_note(
                dt(),
                DevNoteInput::note("a").feature("sleep_staging"),
            )
            .await
            .unwrap();
        db.create_dev_note(
            dt(),
            DevNoteInput::note("b").feature("sleep_staging"),
        )
        .await
        .unwrap();
        db.resolve_dev_note(id1, dt(), "dev").await.unwrap();
        let open = db
            .list_open_notes_for_feature("sleep_staging", 10)
            .await
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "b");
    }
}
