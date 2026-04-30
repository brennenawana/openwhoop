use chrono::{NaiveDate, NaiveDateTime};
use openwhoop_algos::SleepCycle;
use openwhoop_entities::sleep_cycles;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::DatabaseHandler;

impl DatabaseHandler {
    pub async fn get_sleep_cycles(
        &self,
        start: Option<NaiveDateTime>,
    ) -> anyhow::Result<Vec<SleepCycle>> {
        let filter = Condition::all().add_option(start.map(|s| sleep_cycles::Column::Start.gte(s)));

        Ok(sleep_cycles::Entity::find()
            .order_by_asc(sleep_cycles::Column::Start)
            .filter(filter)
            .all(&self.db)
            .await?
            .into_iter()
            .map(map_sleep_cycle)
            .collect())
    }

    /// Find a sleep cycle row by its sleep_id (the unique YYYY-MM-DD key).
    /// Returns the raw entity model so callers can access override columns
    /// and other fields that the algos-level `SleepCycle` strips.
    pub async fn find_sleep_cycle_by_sleep_id(
        &self,
        sleep_id: NaiveDate,
    ) -> anyhow::Result<Option<sleep_cycles::Model>> {
        Ok(sleep_cycles::Entity::find()
            .filter(sleep_cycles::Column::SleepId.eq(sleep_id))
            .one(&self.db)
            .await?)
    }

    /// Apply user-supplied bounds to a sleep cycle. Rewrites the canonical
    /// `start`/`end` to the user's values (so all downstream consumers
    /// keep reading `start`/`end` without indirection), saves the
    /// detector's *previous* bounds into `original_start`/`original_end`
    /// so a "reset to detected" can later restore them, and updates
    /// HR-derived metrics. Staging is invalidated unconditionally —
    /// bounds changed, the prior epochs no longer cover the night, and
    /// `stage_sleep` will repopulate them on the next pipeline run.
    ///
    /// `original_start`/`original_end` are passed by the caller because
    /// only the caller knows whether this is the *first* override (in
    /// which case the existing `row.start`/`row.end` are the detector
    /// values worth preserving) or a *subsequent* edit (in which case the
    /// already-stored `row.original_*` values are the canonical detector
    /// bounds and must be passed through unchanged).
    pub async fn apply_sleep_bounds_override(
        &self,
        sleep_id: NaiveDate,
        recomputed: SleepCycle,
        original_start: Option<NaiveDateTime>,
        original_end: Option<NaiveDateTime>,
    ) -> anyhow::Result<()> {
        let row = self
            .find_sleep_cycle_by_sleep_id(sleep_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("sleep_cycle not found for sleep_id {sleep_id}"))?;
        let cycle_id = row.id;

        let upd = sleep_cycles::ActiveModel {
            id: Set(cycle_id),
            start: Set(recomputed.start),
            end: Set(recomputed.end),
            min_bpm: Set(recomputed.min_bpm.into()),
            max_bpm: Set(recomputed.max_bpm.into()),
            avg_bpm: Set(recomputed.avg_bpm.into()),
            min_hrv: Set(recomputed.min_hrv.into()),
            max_hrv: Set(recomputed.max_hrv.into()),
            avg_hrv: Set(recomputed.avg_hrv.into()),
            score: Set(Some(recomputed.score)),
            original_start: Set(original_start),
            original_end: Set(original_end),
            ..Default::default()
        };
        upd.update(&self.db).await?;

        // Bounds changed by definition (override doesn't fire otherwise),
        // so blow away staging output. The next stage_sleep run picks the
        // cycle back up via its `classifier_version IS NULL` filter.
        self.delete_sleep_epochs_for_cycle(cycle_id).await?;
        self.reset_cycle_staging_fields(cycle_id).await?;

        Ok(())
    }
}

fn map_sleep_cycle(value: sleep_cycles::Model) -> SleepCycle {
    SleepCycle {
        id: value.sleep_id,
        start: value.start,
        end: value.end,
        min_bpm: value.min_bpm.try_into().unwrap(),
        max_bpm: value.max_bpm.try_into().unwrap(),
        avg_bpm: value.avg_bpm.try_into().unwrap(),
        min_hrv: value.min_hrv.try_into().unwrap(),
        max_hrv: value.max_hrv.try_into().unwrap(),
        avg_hrv: value.avg_hrv.try_into().unwrap(),
        score: value
            .score
            .unwrap_or(SleepCycle::sleep_score(value.start, value.end)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn map_sleep_cycle_with_score() {
        let model = sleep_cycles::Model {
            id: uuid::Uuid::new_v4(),
            sleep_id: NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(),
            start: NaiveDate::from_ymd_opt(2025, 1, 1)
                .unwrap()
                .and_hms_opt(22, 0, 0)
                .unwrap(),
            end: NaiveDate::from_ymd_opt(2025, 1, 2)
                .unwrap()
                .and_hms_opt(6, 0, 0)
                .unwrap(),
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: Some(95.0),
            synced: false,
            awake_minutes: None,
            light_minutes: None,
            deep_minutes: None,
            rem_minutes: None,
            sleep_latency_minutes: None,
            waso_minutes: None,
            sleep_efficiency: None,
            wake_event_count: None,
            cycle_count: None,
            avg_respiratory_rate: None,
            min_respiratory_rate: None,
            max_respiratory_rate: None,
            skin_temp_deviation_c: None,
            sleep_need_hours: None,
            sleep_debt_hours: None,
            performance_score: None,
            classifier_version: None,
            original_start: None,
            original_end: None,
        };

        let cycle = map_sleep_cycle(model);
        assert_eq!(cycle.min_bpm, 50);
        assert_eq!(cycle.avg_hrv, 55);
        assert_eq!(cycle.score, 95.0);
    }

    #[test]
    fn map_sleep_cycle_without_score_uses_calculated() {
        let model = sleep_cycles::Model {
            id: uuid::Uuid::new_v4(),
            sleep_id: NaiveDate::from_ymd_opt(2025, 1, 2).unwrap(),
            start: NaiveDate::from_ymd_opt(2025, 1, 1)
                .unwrap()
                .and_hms_opt(22, 0, 0)
                .unwrap(),
            end: NaiveDate::from_ymd_opt(2025, 1, 2)
                .unwrap()
                .and_hms_opt(6, 0, 0)
                .unwrap(),
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: None, // No score stored
            synced: false,
            awake_minutes: None,
            light_minutes: None,
            deep_minutes: None,
            rem_minutes: None,
            sleep_latency_minutes: None,
            waso_minutes: None,
            sleep_efficiency: None,
            wake_event_count: None,
            cycle_count: None,
            avg_respiratory_rate: None,
            min_respiratory_rate: None,
            max_respiratory_rate: None,
            skin_temp_deviation_c: None,
            sleep_need_hours: None,
            sleep_debt_hours: None,
            performance_score: None,
            classifier_version: None,
            original_start: None,
            original_end: None,
        };

        let cycle = map_sleep_cycle(model);
        // 8 hours / 8 hours = 1.0 -> 100.0
        assert_eq!(cycle.score, 100.0);
    }

    #[tokio::test]
    async fn get_sleep_cycles_empty() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let cycles = db.get_sleep_cycles(None).await.unwrap();
        assert!(cycles.is_empty());
    }

    #[tokio::test]
    async fn get_sleep_cycles_returns_inserted() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let start = NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2025, 1, 2)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap();

        db.create_sleep(SleepCycle {
            id: end.date(),
            start,
            end,
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: 100.0,
        })
        .await
        .unwrap();

        let cycles = db.get_sleep_cycles(None).await.unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].min_bpm, 50);
    }

    #[tokio::test]
    async fn get_sleep_cycles_with_start_filter() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        // Insert two sleep cycles
        for day in [1, 3] {
            let start = NaiveDate::from_ymd_opt(2025, 1, day)
                .unwrap()
                .and_hms_opt(22, 0, 0)
                .unwrap();
            let end = NaiveDate::from_ymd_opt(2025, 1, day + 1)
                .unwrap()
                .and_hms_opt(6, 0, 0)
                .unwrap();

            db.create_sleep(SleepCycle {
                id: end.date(),
                start,
                end,
                min_bpm: 50,
                max_bpm: 70,
                avg_bpm: 60,
                min_hrv: 30,
                max_hrv: 80,
                avg_hrv: 55,
                score: 100.0,
            })
            .await
            .unwrap();
        }

        let filter_start = NaiveDate::from_ymd_opt(2025, 1, 2)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();

        let cycles = db.get_sleep_cycles(Some(filter_start)).await.unwrap();
        assert_eq!(cycles.len(), 1); // Only the Jan 3 sleep
    }
}
