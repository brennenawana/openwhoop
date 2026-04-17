//! Database queries that back the sleep-staging pipeline.

use chrono::NaiveDateTime;
use openwhoop_algos::sleep_staging::{
    ArchitectureMetrics, BaselineSnapshot, EpochFeatures, EpochStage, NightAggregate,
    PerformanceScore, RespiratoryStats,
};
use openwhoop_entities::{heart_rate, sleep_cycles, sleep_epochs, user_baselines};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use uuid::Uuid;

use crate::DatabaseHandler;

/// One write-payload produced by the staging pipeline per cycle.
pub struct StageCycleUpdate<'a> {
    pub cycle_id: Uuid,
    pub epochs: &'a [EpochStage],
    pub features: &'a [EpochFeatures],
    pub metrics: &'a ArchitectureMetrics,
    pub respiratory: Option<&'a RespiratoryStats>,
    pub skin_temp_deviation_c: Option<f64>,
    pub sleep_need_hours: f64,
    pub sleep_debt_hours: f64,
    pub performance: &'a PerformanceScore,
    pub classifier_version: &'a str,
}

impl DatabaseHandler {
    /// Load every `sleep_cycles` row (full model) in chronological order.
    /// Used by the staging pipeline to find cycles that need (re)classification.
    pub async fn get_sleep_cycle_models(
        &self,
        start: Option<NaiveDateTime>,
    ) -> anyhow::Result<Vec<sleep_cycles::Model>> {
        let mut q = sleep_cycles::Entity::find().order_by_asc(sleep_cycles::Column::Start);
        if let Some(s) = start {
            q = q.filter(sleep_cycles::Column::Start.gte(s));
        }
        Ok(q.all(&self.db).await?)
    }

    /// Sleep cycles whose classifier_version is NULL or "failed" —
    /// the staging pipeline's work queue.
    pub async fn get_unstaged_sleep_cycles(
        &self,
    ) -> anyhow::Result<Vec<sleep_cycles::Model>> {
        Ok(sleep_cycles::Entity::find()
            .filter(
                sleep_cycles::Column::ClassifierVersion
                    .is_null()
                    .or(sleep_cycles::Column::ClassifierVersion.eq("failed")),
            )
            .order_by_asc(sleep_cycles::Column::Start)
            .all(&self.db)
            .await?)
    }

    /// Sleep cycles in a closed date range — the work queue for
    /// `reclassify-sleep`.
    pub async fn get_sleep_cycles_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<sleep_cycles::Model>> {
        Ok(sleep_cycles::Entity::find()
            .filter(sleep_cycles::Column::Start.gte(start))
            .filter(sleep_cycles::Column::Start.lte(end))
            .order_by_asc(sleep_cycles::Column::Start)
            .all(&self.db)
            .await?)
    }

    /// Delete all sleep_epochs rows for a given cycle. Used during
    /// reclassification.
    pub async fn delete_sleep_epochs_for_cycle(&self, cycle_id: Uuid) -> anyhow::Result<()> {
        sleep_epochs::Entity::delete_many()
            .filter(sleep_epochs::Column::SleepCycleId.eq(cycle_id))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Persist one cycle's worth of staging output: delete-then-insert
    /// the `sleep_epochs` rows and update the `sleep_cycles` row's
    /// staging columns.
    pub async fn apply_staging_update(&self, update: StageCycleUpdate<'_>) -> anyhow::Result<()> {
        self.delete_sleep_epochs_for_cycle(update.cycle_id).await?;

        if !update.epochs.is_empty() {
            let rows: Vec<sleep_epochs::ActiveModel> = update
                .epochs
                .iter()
                .zip(update.features.iter())
                .map(|(e, f)| build_epoch_active(update.cycle_id, e, f, update.classifier_version))
                .collect();

            // SQLite caps at 999 bound variables per statement; sleep_epochs
            // has 22 cols so max ~45 per batch.
            for chunk in rows.chunks(45) {
                sleep_epochs::Entity::insert_many(chunk.to_vec())
                    .exec(&self.db)
                    .await?;
            }
        }

        let m = update.metrics;
        let perf = update.performance;
        let resp_avg = update.respiratory.map(|r| r.avg);
        let resp_min = update.respiratory.map(|r| r.min);
        let resp_max = update.respiratory.map(|r| r.max);

        let cycle_update = sleep_cycles::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(update.cycle_id),
            awake_minutes: Set(Some(m.awake_minutes)),
            light_minutes: Set(Some(m.light_minutes)),
            deep_minutes: Set(Some(m.deep_minutes)),
            rem_minutes: Set(Some(m.rem_minutes)),
            sleep_latency_minutes: Set(Some(m.sleep_latency_minutes)),
            waso_minutes: Set(Some(m.waso_minutes)),
            sleep_efficiency: Set(Some(m.sleep_efficiency)),
            wake_event_count: Set(Some(m.wake_event_count as i32)),
            cycle_count: Set(Some(m.cycle_count as i32)),
            avg_respiratory_rate: Set(resp_avg),
            min_respiratory_rate: Set(resp_min),
            max_respiratory_rate: Set(resp_max),
            skin_temp_deviation_c: Set(update.skin_temp_deviation_c),
            sleep_need_hours: Set(Some(update.sleep_need_hours)),
            sleep_debt_hours: Set(Some(update.sleep_debt_hours)),
            performance_score: Set(Some(perf.total)),
            classifier_version: Set(Some(update.classifier_version.to_string())),
            // Backwards-compat: keep `score` populated with the new
            // performance score so downstream code that reads `score`
            // sees something current.
            score: Set(Some(perf.total)),
            ..Default::default()
        };
        cycle_update.update(&self.db).await?;

        Ok(())
    }

    /// Reset staging fields on a cycle. Used before re-running
    /// classification so the row reflects the in-progress state.
    pub async fn reset_cycle_staging_fields(&self, cycle_id: Uuid) -> anyhow::Result<()> {
        let upd = sleep_cycles::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(cycle_id),
            awake_minutes: Set(None),
            light_minutes: Set(None),
            deep_minutes: Set(None),
            rem_minutes: Set(None),
            sleep_latency_minutes: Set(None),
            waso_minutes: Set(None),
            sleep_efficiency: Set(None),
            wake_event_count: Set(None),
            cycle_count: Set(None),
            avg_respiratory_rate: Set(None),
            min_respiratory_rate: Set(None),
            max_respiratory_rate: Set(None),
            skin_temp_deviation_c: Set(None),
            sleep_need_hours: Set(None),
            sleep_debt_hours: Set(None),
            performance_score: Set(None),
            classifier_version: Set(None),
            ..Default::default()
        };
        upd.update(&self.db).await?;
        Ok(())
    }

    /// Mark a sleep cycle as having failed staging (the pipeline writes
    /// this when feature extraction or classification errored).
    pub async fn mark_cycle_staging_failed(&self, cycle_id: Uuid) -> anyhow::Result<()> {
        let upd = sleep_cycles::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(cycle_id),
            classifier_version: Set(Some("failed".to_string())),
            ..Default::default()
        };
        upd.update(&self.db).await?;
        Ok(())
    }

    /// All `sleep_epochs` rows for a given cycle, in chronological
    /// order. Used by baseline computation.
    pub async fn get_sleep_epochs_for_cycle(
        &self,
        cycle_id: Uuid,
    ) -> anyhow::Result<Vec<sleep_epochs::Model>> {
        Ok(sleep_epochs::Entity::find()
            .filter(sleep_epochs::Column::SleepCycleId.eq(cycle_id))
            .order_by_asc(sleep_epochs::Column::EpochStart)
            .all(&self.db)
            .await?)
    }

    /// Skin-temperature samples inside a time range. Returns (time, °C)
    /// pairs suitable for [`openwhoop_algos::sleep_staging::nightly_skin_temp`].
    pub async fn get_skin_temp_samples_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Vec<(NaiveDateTime, f64)>> {
        let rows: Vec<(NaiveDateTime, Option<f64>)> = heart_rate::Entity::find()
            .filter(heart_rate::Column::Time.gte(start))
            .filter(heart_rate::Column::Time.lte(end))
            .filter(heart_rate::Column::SkinTemp.is_not_null())
            .order_by_asc(heart_rate::Column::Time)
            .select_only()
            .column(heart_rate::Column::Time)
            .column(heart_rate::Column::SkinTemp)
            .into_tuple()
            .all(&self.db)
            .await?;

        Ok(rows.into_iter().filter_map(|(t, v)| v.map(|v| (t, v))).collect())
    }

    /// Most-recent user_baseline row (if any).
    pub async fn get_latest_user_baseline(
        &self,
    ) -> anyhow::Result<Option<user_baselines::Model>> {
        Ok(user_baselines::Entity::find()
            .order_by_desc(user_baselines::Column::ComputedAt)
            .one(&self.db)
            .await?)
    }

    /// Insert a new user_baseline snapshot. Each recompute appends a
    /// row; the classifier reads the latest.
    pub async fn insert_user_baseline(
        &self,
        snapshot: &BaselineSnapshot,
        computed_at: NaiveDateTime,
    ) -> anyhow::Result<()> {
        let model = user_baselines::ActiveModel {
            id: NotSet,
            computed_at: Set(computed_at),
            window_nights: Set(snapshot.window_nights),
            resting_hr: Set(snapshot.resting_hr),
            sleep_rmssd_median: Set(snapshot.sleep_rmssd_median),
            sleep_rmssd_p25: Set(snapshot.sleep_rmssd_p25),
            sleep_rmssd_p75: Set(snapshot.sleep_rmssd_p75),
            hf_power_median: Set(snapshot.hf_power_median),
            lf_hf_ratio_median: Set(snapshot.lf_hf_ratio_median),
            sleep_duration_mean_hours: Set(snapshot.sleep_duration_mean_hours),
            respiratory_rate_mean: Set(snapshot.respiratory_rate_mean),
            respiratory_rate_std: Set(snapshot.respiratory_rate_std),
            skin_temp_mean_c: Set(snapshot.skin_temp_mean_c),
            skin_temp_std_c: Set(snapshot.skin_temp_std_c),
        };
        model.insert(&self.db).await?;
        Ok(())
    }

    /// Build [`NightAggregate`]s for the most recent N cycles that have
    /// staging data. Consumed by baseline computation.
    pub async fn get_recent_night_aggregates(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<NightAggregate>> {
        let cycles = sleep_cycles::Entity::find()
            .filter(sleep_cycles::Column::ClassifierVersion.is_not_null())
            .filter(sleep_cycles::Column::ClassifierVersion.ne("failed"))
            .order_by_desc(sleep_cycles::Column::Start)
            .limit(Some(limit as u64))
            .all(&self.db)
            .await?;

        let mut out = Vec::with_capacity(cycles.len());
        for cycle in cycles {
            let epochs = self.get_sleep_epochs_for_cycle(cycle.id).await?;
            let rmssd_samples: Vec<f64> = epochs.iter().filter_map(|e| e.rmssd).collect();
            let hf_power_samples: Vec<f64> = epochs.iter().filter_map(|e| e.hf_power).collect();
            let lf_hf_ratio_samples: Vec<f64> =
                epochs.iter().filter_map(|e| e.lf_hf_ratio).collect();
            // Min HR across epochs as resting-HR proxy.
            let resting_hr = epochs
                .iter()
                .filter_map(|e| e.hr_min)
                .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.min(v))));
            // Sleep duration from stage counts: non-Wake, non-Unknown
            // epochs × 0.5 min.
            let duration_min: f64 = epochs
                .iter()
                .filter(|e| e.stage != "Wake" && e.stage != "Unknown")
                .count() as f64
                * 0.5;
            let duration_hours = if duration_min > 0.0 {
                Some(duration_min / 60.0)
            } else {
                None
            };

            out.push(NightAggregate {
                sleep_start: cycle.start,
                sleep_end: cycle.end,
                resting_hr,
                rmssd_samples,
                hf_power_samples,
                lf_hf_ratio_samples,
                duration_hours,
                respiratory_rate_avg: cycle.avg_respiratory_rate,
                // We stored deviation, not absolute temp. Reconstruct
                // absolute by adding back the *prior* baseline. For the
                // first baseline computation (no prior), leave as None —
                // the skin-temp part of the baseline will fill in on the
                // second run once a prior baseline exists.
                skin_temp_nightly_c: None,
            });
        }
        Ok(out)
    }

    /// Average of `heart_rate.stress` (Baevsky 0-10) across the time
    /// range. Returns `None` when no rows in the range have a stress
    /// value (e.g. before `calculate-stress` has been run). Used by
    /// the staging pipeline's performance-score input.
    pub async fn avg_stress_in_range(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> anyhow::Result<Option<f64>> {
        let rows: Vec<Option<f64>> = heart_rate::Entity::find()
            .filter(heart_rate::Column::Time.gte(start))
            .filter(heart_rate::Column::Time.lte(end))
            .filter(heart_rate::Column::Stress.is_not_null())
            .select_only()
            .column(heart_rate::Column::Stress)
            .into_tuple()
            .all(&self.db)
            .await?;
        let values: Vec<f64> = rows.into_iter().flatten().collect();
        if values.is_empty() {
            return Ok(None);
        }
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        Ok(Some(mean))
    }

    /// Most-recent sleep cycle along with its epoch rows. Used by the
    /// Tauri tray app to build a snapshot of last night's sleep.
    pub async fn get_latest_sleep_with_epochs(
        &self,
    ) -> anyhow::Result<Option<(sleep_cycles::Model, Vec<sleep_epochs::Model>)>> {
        let cycle = sleep_cycles::Entity::find()
            .order_by_desc(sleep_cycles::Column::End)
            .one(&self.db)
            .await?;
        let Some(cycle) = cycle else {
            return Ok(None);
        };
        let epochs = self.get_sleep_epochs_for_cycle(cycle.id).await?;
        Ok(Some((cycle, epochs)))
    }

    /// Total nap minutes across activities in the prior 24 hours
    /// before `reference`. Used by sleep-need calculation.
    pub async fn sum_nap_minutes_in_prior_day(
        &self,
        reference: NaiveDateTime,
    ) -> anyhow::Result<f64> {
        use openwhoop_entities::activities;

        let window_start = reference - chrono::Duration::hours(24);
        let naps = activities::Entity::find()
            .filter(activities::Column::Activity.eq("Nap"))
            .filter(activities::Column::Start.gte(window_start))
            .filter(activities::Column::End.lte(reference))
            .all(&self.db)
            .await?;

        let total_seconds: i64 = naps
            .into_iter()
            .map(|a| a.end.signed_duration_since(a.start).num_seconds().max(0))
            .sum();
        Ok(total_seconds as f64 / 60.0)
    }
}

fn build_epoch_active(
    cycle_id: Uuid,
    stage: &EpochStage,
    features: &EpochFeatures,
    classifier_version: &str,
) -> sleep_epochs::ActiveModel {
    sleep_epochs::ActiveModel {
        id: NotSet,
        sleep_cycle_id: Set(cycle_id),
        epoch_start: Set(stage.epoch_start),
        epoch_end: Set(stage.epoch_end),
        stage: Set(stage.stage.as_str().to_string()),
        confidence: Set(None),
        hr_mean: Set(features.hr_mean),
        hr_std: Set(features.hr_std),
        hr_min: Set(features.hr_min),
        hr_max: Set(features.hr_max),
        rmssd: Set(features.rmssd),
        sdnn: Set(features.sdnn),
        pnn50: Set(features.pnn50),
        lf_power: Set(features.lf_power),
        hf_power: Set(features.hf_power),
        lf_hf_ratio: Set(features.lf_hf_ratio),
        motion_activity_count: Set(features.motion_activity_count),
        motion_stillness_ratio: Set(features.motion_stillness_ratio),
        resp_rate: Set(features.resp_rate),
        feature_blob: Set(None),
        classifier_version: Set(classifier_version.to_string()),
    }
}
