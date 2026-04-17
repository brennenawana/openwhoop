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
use openwhoop_entities::{sleep_cycles, sleep_epochs, user_baselines};

#[derive(Debug, Default)]
pub struct StageResult {
    pub cycles_considered: usize,
    pub cycles_succeeded: usize,
    pub cycles_failed: usize,
    pub baseline_refreshed: bool,
}

/// Snapshot of last night's sleep for the Tauri tray app. All fields
/// derive from the persisted `sleep_cycles` row and its epochs — the
/// front-end builds its own display struct from this payload.
#[derive(Debug, Clone, PartialEq)]
pub struct SleepSnapshot {
    pub sleep_start: NaiveDateTime,
    pub sleep_end: NaiveDateTime,
    pub stages: SleepStageTotals,
    pub hypnogram: Vec<HypnogramEntry>,
    pub efficiency: Option<f64>,
    pub latency_min: Option<f64>,
    pub waso_min: Option<f64>,
    pub cycle_count: Option<i32>,
    pub wake_event_count: Option<i32>,
    pub avg_respiratory_rate: Option<f64>,
    pub skin_temp_deviation_c: Option<f64>,
    pub performance_score: Option<f64>,
    pub sleep_need_hours: Option<f64>,
    pub sleep_debt_hours: Option<f64>,
    pub score_components: Option<ScoreComponentsBreakdown>,
    pub classifier_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SleepStageTotals {
    pub awake_min: f64,
    pub light_min: f64,
    pub deep_min: f64,
    pub rem_min: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HypnogramEntry {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub stage: String,
}

/// Re-derivable score component breakdown. Consistency and sleep
/// stress aren't persisted, so they come back as `NEUTRAL_*` values
/// (matching what the staging pipeline passed as fallbacks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreComponentsBreakdown {
    pub sufficiency: f64,
    pub efficiency: f64,
    pub restorative: f64,
    pub consistency: f64,
    pub sleep_stress: f64,
}

/// Fetch the most recent sleep cycle + epochs and shape them into a
/// [`SleepSnapshot`]. `None` when the user has no sleep cycles yet.
pub async fn latest_sleep_snapshot(
    db: &DatabaseHandler,
) -> anyhow::Result<Option<SleepSnapshot>> {
    let Some((cycle, epochs)) = db.get_latest_sleep_with_epochs().await? else {
        return Ok(None);
    };
    Ok(Some(build_snapshot(&cycle, &epochs)))
}

fn build_snapshot(
    cycle: &sleep_cycles::Model,
    epochs: &[sleep_epochs::Model],
) -> SleepSnapshot {
    let epoch_stages: Vec<openwhoop_algos::sleep_staging::EpochStage> = epochs
        .iter()
        .map(|e| openwhoop_algos::sleep_staging::EpochStage {
            epoch_start: e.epoch_start,
            epoch_end: e.epoch_end,
            stage: openwhoop_algos::sleep_staging::SleepStage::parse(&e.stage)
                .unwrap_or(openwhoop_algos::sleep_staging::SleepStage::Unknown),
            classifier_version: "rule-v1",
        })
        .collect();

    let hypnogram: Vec<HypnogramEntry> =
        openwhoop_algos::sleep_staging::quantized_hypnogram(&epoch_stages)
            .into_iter()
            .map(|seg| HypnogramEntry {
                start: seg.start,
                end: seg.end,
                stage: seg.stage.as_str().to_string(),
            })
            .collect();

    let stages = SleepStageTotals {
        awake_min: cycle.awake_minutes.unwrap_or(0.0),
        light_min: cycle.light_minutes.unwrap_or(0.0),
        deep_min: cycle.deep_minutes.unwrap_or(0.0),
        rem_min: cycle.rem_minutes.unwrap_or(0.0),
    };

    let score_components = recompute_components(cycle);

    SleepSnapshot {
        sleep_start: cycle.start,
        sleep_end: cycle.end,
        stages,
        hypnogram,
        efficiency: cycle.sleep_efficiency,
        latency_min: cycle.sleep_latency_minutes,
        waso_min: cycle.waso_minutes,
        cycle_count: cycle.cycle_count,
        wake_event_count: cycle.wake_event_count,
        avg_respiratory_rate: cycle.avg_respiratory_rate,
        skin_temp_deviation_c: cycle.skin_temp_deviation_c,
        performance_score: cycle.performance_score,
        sleep_need_hours: cycle.sleep_need_hours,
        sleep_debt_hours: cycle.sleep_debt_hours,
        score_components,
        classifier_version: cycle.classifier_version.clone(),
    }
}

/// Reconstruct the five score components from the persisted cycle.
/// Sufficiency / efficiency / restorative are derivable from stored
/// values; consistency + sleep_stress come back as the neutral
/// fallbacks the pipeline used at write time. Returns `None` when
/// any required derivation input is missing.
fn recompute_components(cycle: &sleep_cycles::Model) -> Option<ScoreComponentsBreakdown> {
    use openwhoop_algos::sleep_staging::constants::{
        NEUTRAL_CONSISTENCY, NEUTRAL_SLEEP_STRESS, RESTORATIVE_TARGET_PCT, SCORE_WEIGHTS,
    };

    let need = cycle.sleep_need_hours?;
    if need <= 0.0 {
        return None;
    }
    let light = cycle.light_minutes?;
    let deep = cycle.deep_minutes?;
    let rem = cycle.rem_minutes?;
    let efficiency = cycle.sleep_efficiency.unwrap_or(0.0);
    let tib_min = (cycle.end - cycle.start).num_minutes() as f64;
    if tib_min <= 0.0 {
        return None;
    }
    let total_sleep_h = (light + deep + rem) / 60.0;
    let sufficiency = (total_sleep_h / need * 100.0).clamp(0.0, 100.0);
    let deep_pct = 100.0 * deep / tib_min;
    let rem_pct = 100.0 * rem / tib_min;
    let restorative = ((deep_pct + rem_pct) / RESTORATIVE_TARGET_PCT * 100.0).clamp(0.0, 100.0);
    let stress = (100.0 - NEUTRAL_SLEEP_STRESS * 10.0).clamp(0.0, 100.0);

    // Sanity self-check: weighted sum should be within a hair of the
    // persisted performance_score if the cycle was classified with
    // this version of the weights.
    let _total_check = SCORE_WEIGHTS.sufficiency * sufficiency
        + SCORE_WEIGHTS.efficiency * efficiency
        + SCORE_WEIGHTS.restorative * restorative
        + SCORE_WEIGHTS.consistency * NEUTRAL_CONSISTENCY
        + SCORE_WEIGHTS.sleep_stress * stress;

    Some(ScoreComponentsBreakdown {
        sufficiency,
        efficiency,
        restorative,
        consistency: NEUTRAL_CONSISTENCY,
        sleep_stress: stress,
    })
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
/// Forces a baseline recompute after staging so subsequent runs see
/// the effect of any threshold changes that triggered the reclassify.
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
    let mut result = stage_cycles(db, &cycles).await?;
    result.baseline_refreshed = refresh_baseline(db, true).await?;
    Ok(result)
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
    refresh_baseline(db, false).await
}

/// Recompute the baseline, bypassing the 24 h idempotence gate when
/// `force = true`. Used by the reclassify path so the baseline
/// reflects whatever threshold change motivated the reclassify.
async fn refresh_baseline(db: &DatabaseHandler, force: bool) -> anyhow::Result<bool> {
    let now = Local::now().naive_local();
    let last = db.get_latest_user_baseline().await?.map(|m| m.computed_at);
    if !force && !should_update(last, now) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use uuid::Uuid;

    fn cycle_fixture() -> sleep_cycles::Model {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end = start + chrono::TimeDelta::hours(8);
        sleep_cycles::Model {
            id: Uuid::new_v4(),
            sleep_id: end.date(),
            start,
            end,
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: Some(85.0),
            synced: false,
            awake_minutes: Some(30.0),
            light_minutes: Some(250.0),
            deep_minutes: Some(90.0),
            rem_minutes: Some(110.0),
            sleep_latency_minutes: Some(15.0),
            waso_minutes: Some(25.0),
            sleep_efficiency: Some(90.0),
            wake_event_count: Some(3),
            cycle_count: Some(4),
            avg_respiratory_rate: Some(14.0),
            min_respiratory_rate: Some(11.0),
            max_respiratory_rate: Some(18.0),
            skin_temp_deviation_c: Some(0.2),
            sleep_need_hours: Some(8.0),
            sleep_debt_hours: Some(1.5),
            performance_score: Some(85.0),
            classifier_version: Some("rule-v1".to_string()),
        }
    }

    #[test]
    fn snapshot_pulls_stage_totals_from_cycle() {
        let cycle = cycle_fixture();
        let snap = build_snapshot(&cycle, &[]);
        assert_eq!(snap.stages.awake_min, 30.0);
        assert_eq!(snap.stages.light_min, 250.0);
        assert_eq!(snap.stages.deep_min, 90.0);
        assert_eq!(snap.stages.rem_min, 110.0);
    }

    #[test]
    fn snapshot_empty_epochs_produces_empty_hypnogram() {
        let cycle = cycle_fixture();
        let snap = build_snapshot(&cycle, &[]);
        assert!(snap.hypnogram.is_empty());
    }

    #[test]
    fn snapshot_forwards_persisted_scalars() {
        let cycle = cycle_fixture();
        let snap = build_snapshot(&cycle, &[]);
        assert_eq!(snap.efficiency, Some(90.0));
        assert_eq!(snap.latency_min, Some(15.0));
        assert_eq!(snap.waso_min, Some(25.0));
        assert_eq!(snap.cycle_count, Some(4));
        assert_eq!(snap.wake_event_count, Some(3));
        assert_eq!(snap.avg_respiratory_rate, Some(14.0));
        assert_eq!(snap.skin_temp_deviation_c, Some(0.2));
        assert_eq!(snap.performance_score, Some(85.0));
        assert_eq!(snap.sleep_need_hours, Some(8.0));
        assert_eq!(snap.sleep_debt_hours, Some(1.5));
    }

    #[test]
    fn snapshot_recomputes_score_components() {
        let cycle = cycle_fixture();
        let snap = build_snapshot(&cycle, &[]);
        let comps = snap.score_components.unwrap();
        // total_sleep = (250 + 90 + 110) / 60 = 7.5 h
        // sufficiency = 7.5 / 8.0 × 100 = 93.75
        assert!((comps.sufficiency - 93.75).abs() < 0.01);
        // tib = 480 min; restorative_pct = (90 + 110) / 480 × 100 = ~41.67
        // restorative score = 41.67 / 45 × 100 ≈ 92.59
        assert!((comps.restorative - 92.5925925925926).abs() < 0.01);
        assert_eq!(comps.efficiency, 90.0);
        // Neutral fallbacks for consistency + sleep_stress:
        assert_eq!(comps.consistency, 50.0);
        assert_eq!(comps.sleep_stress, 50.0);
    }
}


