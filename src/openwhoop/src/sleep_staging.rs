//! Orchestration of the sleep-staging pipeline.
//!
//! Ties together feature extraction, classification, architecture
//! metrics, respiratory + skin-temp aggregation, sleep-need / debt
//! computation, performance scoring, and the user-baseline refresh.
//! Errors in one cycle are isolated — the pipeline logs and marks
//! that cycle `classifier_version = "failed"`, then continues.

use chrono::{Local, NaiveDateTime, TimeDelta};
use openwhoop_algos::sleep_staging::{
    self, ArchitectureMetrics, CLASSIFIER_VERSION, EpochFeatures, EpochStage, NightSleep,
    PerformanceScore, RespiratoryStats, ScoringInputs, SleepNeedInputs, UserBaseline,
    build_epochs, classify_epochs, compute_baseline, compute_metrics, nightly_respiratory_rate,
    nightly_skin_temp, performance_score, should_update, sleep_debt_hours, sleep_need_hours,
};
use openwhoop_db::{DatabaseHandler, SearchHistory, StageCycleUpdate};
use openwhoop_entities::{sleep_cycles, user_baselines};

#[derive(Debug, Default)]
pub struct StageResult {
    pub cycles_considered: usize,
    pub cycles_succeeded: usize,
    pub cycles_failed: usize,
    pub baseline_refreshed: bool,
}

/// Run the staging pipeline for every unstaged sleep cycle, then
/// refresh the user baseline if stale. Safe to call on every sync.
pub async fn stage_unclassified(db: &DatabaseHandler) -> anyhow::Result<StageResult> {
    let cycles = db.get_unstaged_sleep_cycles().await?;
    let mut result = stage_cycles(db, &cycles).await?;
    result.baseline_refreshed = refresh_baseline_if_stale(db).await?;
    Ok(result)
}

/// Run the staging pipeline for an explicit list of cycles. Used by
/// the reclassify CLI command.
pub async fn stage_cycles(
    db: &DatabaseHandler,
    cycles: &[sleep_cycles::Model],
) -> anyhow::Result<StageResult> {
    let baseline = load_user_baseline(db).await?;
    let mut result = StageResult {
        cycles_considered: cycles.len(),
        ..Default::default()
    };

    for cycle in cycles {
        match stage_one_cycle(db, cycle, &baseline).await {
            Ok(()) => result.cycles_succeeded += 1,
            Err(e) => {
                log::error!(
                    "sleep staging failed for cycle {} ({} → {}): {:#}",
                    cycle.id,
                    cycle.start,
                    cycle.end,
                    e
                );
                if let Err(mark_err) = db.mark_cycle_staging_failed(cycle.id).await {
                    log::error!(
                        "failed to mark cycle {} as failed: {:#}",
                        cycle.id, mark_err
                    );
                }
                result.cycles_failed += 1;
            }
        }
    }

    Ok(result)
}

/// Reclassify every cycle whose `start` falls in `[from, to]`. Wipes
/// existing epochs and resets staging columns before re-running.
pub async fn reclassify_range(
    db: &DatabaseHandler,
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> anyhow::Result<StageResult> {
    let cycles = db.get_sleep_cycles_in_range(from, to).await?;
    for cycle in &cycles {
        db.delete_sleep_epochs_for_cycle(cycle.id).await?;
        db.reset_cycle_staging_fields(cycle.id).await?;
    }
    stage_cycles(db, &cycles).await
}

async fn load_user_baseline(db: &DatabaseHandler) -> anyhow::Result<UserBaseline> {
    match db.get_latest_user_baseline().await? {
        Some(m) => Ok(baseline_from_model(m)),
        None => Ok(UserBaseline::population_default()),
    }
}

fn baseline_from_model(m: user_baselines::Model) -> UserBaseline {
    UserBaseline {
        resting_hr: m.resting_hr,
        sleep_rmssd_median: m.sleep_rmssd_median,
        sleep_rmssd_p25: m.sleep_rmssd_p25,
        sleep_rmssd_p75: m.sleep_rmssd_p75,
        hf_power_median: m.hf_power_median,
        lf_hf_ratio_median: m.lf_hf_ratio_median,
        respiratory_rate_mean: m.respiratory_rate_mean,
        respiratory_rate_std: m.respiratory_rate_std,
        window_nights: m.window_nights,
    }
}

async fn stage_one_cycle(
    db: &DatabaseHandler,
    cycle: &sleep_cycles::Model,
    baseline: &UserBaseline,
) -> anyhow::Result<()> {
    // 1. Fetch heart_rate rows inside the sleep window.
    let readings = db
        .search_history(SearchHistory {
            from: Some(cycle.start),
            to: Some(cycle.end),
            limit: None,
        })
        .await?;

    // 2. Feature extraction (30 s epochs).
    let features: Vec<EpochFeatures> = build_epochs(cycle.start, cycle.end, &readings);
    if features.is_empty() {
        anyhow::bail!("no epochs produced for cycle {}", cycle.id);
    }

    // 3. Stage classification.
    let epoch_stages: Vec<EpochStage> = classify_epochs(&features, baseline);

    // 4. Architecture metrics.
    let metrics: ArchitectureMetrics = compute_metrics(cycle.start, cycle.end, &epoch_stages);

    // 5. Respiratory rate (aggregated from per-epoch values).
    let resp: Option<RespiratoryStats> = nightly_respiratory_rate(&features);

    // 6. Skin temperature deviation vs rolling baseline.
    let skin_deviation = compute_skin_temp_deviation(db, cycle, baseline).await?;

    // 7. Sleep need / debt. prior_day_strain is not yet computed in
    // this codebase — pass None and let base_need carry it. When a
    // strain calculator is added, wire it here.
    let nap_minutes = db.sum_nap_minutes_in_prior_day(cycle.end).await.unwrap_or(0.0);
    let debt = compute_prior_sleep_debt(db, cycle.start).await?;
    let need_inputs = SleepNeedInputs {
        base_need_hours: None,
        prior_day_strain: None,
        rolling_7d_debt_hours: debt,
        nap_minutes,
    };
    let need = sleep_need_hours(&need_inputs);

    // 8. Performance score.
    let actual_sleep_hours = metrics.total_sleep_minutes / 60.0;
    let scoring = ScoringInputs {
        actual_sleep_hours,
        sleep_need_hours: need,
        sleep_efficiency: metrics.sleep_efficiency,
        deep_pct: metrics.deep_pct,
        rem_pct: metrics.rem_pct,
        // Consistency + sleep stress inputs aren't wired yet in this
        // pipeline. Neutral fallbacks keep the total well-defined
        // until they're plumbed in.
        consistency_score: None,
        avg_sleep_stress: None,
    };
    let perf: PerformanceScore = performance_score(&scoring);

    // 9. Persist.
    let update = StageCycleUpdate {
        cycle_id: cycle.id,
        epochs: &epoch_stages,
        features: &features,
        metrics: &metrics,
        respiratory: resp.as_ref(),
        skin_temp_deviation_c: skin_deviation,
        sleep_need_hours: need,
        sleep_debt_hours: debt,
        performance: &perf,
        classifier_version: CLASSIFIER_VERSION,
    };
    db.apply_staging_update(update).await?;

    Ok(())
}

async fn compute_skin_temp_deviation(
    db: &DatabaseHandler,
    cycle: &sleep_cycles::Model,
    baseline: &UserBaseline,
) -> anyhow::Result<Option<f64>> {
    // Baseline's skin_temp mean isn't currently in UserBaseline (it's
    // on the user_baselines row but we only carry HRV fields through
    // the algo-layer struct). Go straight to the DB.
    let latest_baseline_mean = db
        .get_latest_user_baseline()
        .await?
        .and_then(|m| m.skin_temp_mean_c);

    let samples = db
        .get_skin_temp_samples_in_range(cycle.start, cycle.end)
        .await?;
    let nightly = nightly_skin_temp(cycle.start, cycle.end, &samples);

    match (nightly, latest_baseline_mean) {
        (Some(n), Some(b)) => Ok(Some(n - b)),
        _ => {
            let _ = baseline; // referenced for future parameterization
            Ok(None)
        }
    }
}

async fn compute_prior_sleep_debt(
    db: &DatabaseHandler,
    current_cycle_start: NaiveDateTime,
) -> anyhow::Result<f64> {
    // Pull up to 7 prior staged cycles ending before the current
    // cycle starts. For each, use its stored sleep_need_hours and
    // infer actual sleep hours from the stage minutes.
    let seven_days_ago = current_cycle_start - TimeDelta::days(7);
    let recent = db.get_sleep_cycle_models(Some(seven_days_ago)).await?;
    let recent: Vec<&sleep_cycles::Model> = recent
        .iter()
        .filter(|c| c.end < current_cycle_start)
        .collect();

    let mut nights: Vec<NightSleep> = Vec::new();
    // Most-recent-first, up to 7 entries.
    for cycle in recent.iter().rev().take(7) {
        let need = cycle.sleep_need_hours.unwrap_or(7.5);
        let actual = cycle
            .light_minutes
            .zip(cycle.deep_minutes)
            .zip(cycle.rem_minutes)
            .map(|((l, d), r)| (l + d + r) / 60.0)
            // Fall back to the existing `score`-based sleep estimate:
            // the pre-staging sleep_score was (actual_hours / 8) × 100,
            // so actual ≈ score / 100 × 8.
            .unwrap_or_else(|| cycle.score.unwrap_or(0.0) / 100.0 * 8.0);
        nights.push(NightSleep {
            sleep_need_hours: need,
            actual_sleep_hours: actual,
        });
    }
    Ok(sleep_debt_hours(&nights))
}

/// Recompute and persist the user baseline if >24 h have elapsed
/// since the last snapshot. Returns `true` when a new baseline row
/// was written.
pub async fn refresh_baseline_if_stale(db: &DatabaseHandler) -> anyhow::Result<bool> {
    let now = Local::now().naive_local();
    let last = db.get_latest_user_baseline().await?.map(|m| m.computed_at);
    if !should_update(last, now) {
        return Ok(false);
    }
    let nights = db
        .get_recent_night_aggregates(sleep_staging::BASELINE_WINDOW_NIGHTS)
        .await?;
    if nights.is_empty() {
        return Ok(false);
    }
    let snapshot = compute_baseline(&nights);
    db.insert_user_baseline(&snapshot, now).await?;
    Ok(true)
}

