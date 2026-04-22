//! Sleep need, debt, and composite performance score.
//!
//! Pure functions over numeric inputs; persistence + aggregation is
//! the caller's responsibility. Every coefficient lives in
//! [`super::constants`] so future config overrides only touch one
//! place.
//!
//! Formulas follow WHOOP's publicly-documented additive structure
//! (patent US20240252121A1 + developer API):
//!
//! ```text
//! need = baseline + Δ_strain + Δ_debt + Δ_nap  (Δ_nap ≤ 0)
//! ```
//!
//! Each term is grounded in primary sleep-science literature — see
//! [`super::constants`] for citations on each coefficient.

use super::constants::{
    NEED_BASELINE_MAX_STRAIN, NEED_BASELINE_MIN_EFFICIENCY_PCT,
    NEED_BASELINE_MIN_SLEEP_HOURS, NEED_BASELINE_MIN_USABLE_NIGHTS,
    NEED_BASELINE_WINDOW_NIGHTS, BASE_NEED_HOURS, DEBT_ADJ_CAP_HOURS, DEBT_ADJ_COEF,
    DEBT_DECAY_WEIGHTS, MAX_SLEEP_NEED_HOURS, MIN_SLEEP_NEED_HOURS, NAP_CREDIT_FRAC,
    NEUTRAL_CONSISTENCY, NEUTRAL_SLEEP_STRESS, RESTORATIVE_TARGET_PCT, SCORE_WEIGHTS,
    STRAIN_ADJ_CAP_MIN, STRAIN_LINEAR_COEF, STRAIN_NONLINEAR_COEF, STRAIN_NONLINEAR_POWER,
    STRAIN_NONLINEAR_THRESHOLD, SURPLUS_CREDIT_CAP_HOURS, SURPLUS_CREDIT_COEF,
};

/// Inputs for the per-night sleep-need calculation.
#[derive(Debug, Clone, Default)]
pub struct SleepNeedInputs {
    /// Personalized baseline from [`personalized_baseline_hours`], or
    /// `None` to use the population default [`BASE_NEED_HOURS`].
    pub base_need_hours: Option<f64>,
    /// Prior-day WHOOP-scale strain (0–21). `None` = unknown → 0.
    pub prior_day_strain: Option<f64>,
    /// Decay-weighted mean of qualifying naps (minutes). Only naps
    /// ending ≥ [`NAP_BEDTIME_GUARD_MINUTES`] before bedtime count.
    pub nap_minutes: f64,
    /// Pre-computed rolling sleep debt (hours). See
    /// [`sleep_debt_hours`].
    pub rolling_7d_debt_hours: f64,
    /// Pre-computed rolling sleep surplus (hours), only used when
    /// `allow_surplus_banking` is true. See [`sleep_surplus_hours`].
    pub rolling_7d_surplus_hours: f64,
    /// Enable Rupp 2009 "banking" behavior: surplus sleep across the
    /// window reduces tonight's need. WHOOP does NOT expose this.
    /// Default `false` matches WHOOP's user-facing number.
    pub allow_surplus_banking: bool,
}

/// Compute the strain-driven addition to tonight's need, in HOURS.
/// Public so callers can display the decomposition separately.
pub fn strain_addition_hours(strain: f64) -> f64 {
    let linear = STRAIN_LINEAR_COEF * strain.max(0.0);
    let over = (strain - STRAIN_NONLINEAR_THRESHOLD).max(0.0);
    let nonlinear = STRAIN_NONLINEAR_COEF * over.powf(STRAIN_NONLINEAR_POWER);
    let total_min = (linear + nonlinear).min(STRAIN_ADJ_CAP_MIN);
    total_min / 60.0
}

/// Compute personalized sleep need for tonight, in hours, clamped to
/// [`MIN_SLEEP_NEED_HOURS`, `MAX_SLEEP_NEED_HOURS`].
pub fn sleep_need_hours(inputs: &SleepNeedInputs) -> f64 {
    let base = inputs.base_need_hours.unwrap_or(BASE_NEED_HOURS);
    let strain_adj = strain_addition_hours(inputs.prior_day_strain.unwrap_or(0.0));
    let debt_adj = (inputs.rolling_7d_debt_hours * DEBT_ADJ_COEF).min(DEBT_ADJ_CAP_HOURS);
    let nap_credit = inputs.nap_minutes * NAP_CREDIT_FRAC / 60.0;
    let surplus_credit = if inputs.allow_surplus_banking {
        (inputs.rolling_7d_surplus_hours * SURPLUS_CREDIT_COEF).min(SURPLUS_CREDIT_CAP_HOURS)
    } else {
        0.0
    };
    let need = base + strain_adj + debt_adj - nap_credit - surplus_credit;
    need.clamp(MIN_SLEEP_NEED_HOURS, MAX_SLEEP_NEED_HOURS)
}

/// One historical night's sleep need and what was actually slept.
#[derive(Debug, Clone, Copy)]
pub struct NightSleep {
    pub sleep_need_hours: f64,
    pub actual_sleep_hours: f64,
}

/// Rolling 7-day sleep debt as a decay-weighted mean shortfall
/// (hours short per night). `recent_nights` is ordered most-recent-
/// first; beyond 7 is ignored. Dividing by Σ included weights makes
/// the returned value interpretable as "average hours short per
/// night (weighted toward recency)."
pub fn sleep_debt_hours(recent_nights: &[NightSleep]) -> f64 {
    weighted_mean_deficit(recent_nights, |n| {
        (n.sleep_need_hours - n.actual_sleep_hours).max(0.0)
    })
}

/// Rolling 7-day sleep *surplus* as a decay-weighted mean excess
/// (hours slept over need). Counterpart to [`sleep_debt_hours`] for
/// the Rupp 2009 banking path. Same window + weights; one-sided.
pub fn sleep_surplus_hours(recent_nights: &[NightSleep]) -> f64 {
    weighted_mean_deficit(recent_nights, |n| {
        (n.actual_sleep_hours - n.sleep_need_hours).max(0.0)
    })
}

fn weighted_mean_deficit<F: Fn(&NightSleep) -> f64>(
    recent_nights: &[NightSleep],
    project: F,
) -> f64 {
    let (num, den) = recent_nights
        .iter()
        .zip(DEBT_DECAY_WEIGHTS.iter())
        .fold((0.0, 0.0), |(n, d), (night, &w)| {
            (n + w * project(night), d + w)
        });
    if den <= 0.0 { 0.0 } else { num / den }
}

/// A historical night with enough context to decide whether it's
/// "baseline-eligible" — i.e. low-strain, well-slept, nap-free.
#[derive(Debug, Clone, Copy)]
pub struct BaselineNight {
    pub actual_sleep_hours: f64,
    pub sleep_efficiency_pct: f64,
    pub day_strain: f64,
    pub had_nap: bool,
}

impl BaselineNight {
    fn eligible(&self) -> bool {
        self.day_strain <= NEED_BASELINE_MAX_STRAIN
            && self.sleep_efficiency_pct >= NEED_BASELINE_MIN_EFFICIENCY_PCT
            && self.actual_sleep_hours >= NEED_BASELINE_MIN_SLEEP_HOURS
            && !self.had_nap
    }
}

/// Compute the per-user baseline sleep need (hours) from the last
/// [`NEED_BASELINE_WINDOW_NIGHTS`] of data. Filters for "typical rest day"
/// nights and takes a trimmed mean (drops top + bottom 10% of samples
/// to resist outliers).
///
/// Blends linearly toward [`BASE_NEED_HOURS`] until
/// [`NEED_BASELINE_MIN_USABLE_NIGHTS`] eligible nights accumulate — so
/// early users aren't whipped around by one or two outlier nights.
///
/// `recent_nights` is ordered most-recent-first; beyond
/// [`NEED_BASELINE_WINDOW_NIGHTS`] is ignored.
pub fn personalized_baseline_hours(recent_nights: &[BaselineNight]) -> f64 {
    let eligible: Vec<f64> = recent_nights
        .iter()
        .take(NEED_BASELINE_WINDOW_NIGHTS)
        .filter(|n| n.eligible())
        .map(|n| n.actual_sleep_hours)
        .collect();

    if eligible.is_empty() {
        return BASE_NEED_HOURS;
    }

    let mut sorted = eligible.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Drop top + bottom 10% (at least 0; at most half-1 each side).
    let drop_each = (sorted.len() / 10).min(sorted.len().saturating_sub(1) / 2);
    let trimmed = &sorted[drop_each..sorted.len() - drop_each];
    let personalized = trimmed.iter().sum::<f64>() / trimmed.len() as f64;

    let alpha = (eligible.len() as f64 / NEED_BASELINE_MIN_USABLE_NIGHTS as f64).min(1.0);
    alpha * personalized + (1.0 - alpha) * BASE_NEED_HOURS
}

/// How many of the last `NEED_BASELINE_WINDOW_NIGHTS` qualify for the
/// baseline computation. Exposed for UI "calibrating" hints.
pub fn baseline_eligible_nights(recent_nights: &[BaselineNight]) -> usize {
    recent_nights
        .iter()
        .take(NEED_BASELINE_WINDOW_NIGHTS)
        .filter(|n| n.eligible())
        .count()
}


#[derive(Debug, Clone, PartialEq)]
pub struct PerformanceComponents {
    pub sufficiency: f64,
    pub efficiency: f64,
    pub restorative: f64,
    pub consistency: f64,
    pub sleep_stress: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PerformanceScore {
    pub components: PerformanceComponents,
    pub total: f64,
}

#[derive(Debug, Clone)]
pub struct ScoringInputs {
    pub actual_sleep_hours: f64,
    pub sleep_need_hours: f64,
    pub sleep_efficiency: f64,
    pub deep_pct: f64,
    pub rem_pct: f64,
    /// Existing ConsistencyScore.total_score (0–100). None falls back
    /// to `NEUTRAL_CONSISTENCY` rather than distorting the total.
    pub consistency_score: Option<f64>,
    /// Baevsky stress averaged over the sleep window (0–10). None
    /// falls back to `NEUTRAL_SLEEP_STRESS`.
    pub avg_sleep_stress: Option<f64>,
}

pub fn performance_score(inputs: &ScoringInputs) -> PerformanceScore {
    let suf = sufficiency_score(inputs.actual_sleep_hours, inputs.sleep_need_hours);
    let eff = inputs.sleep_efficiency.clamp(0.0, 100.0);
    let rest = restorative_score(inputs.deep_pct + inputs.rem_pct);
    let cons = inputs.consistency_score.unwrap_or(NEUTRAL_CONSISTENCY).clamp(0.0, 100.0);
    let stress = sleep_stress_score(inputs.avg_sleep_stress.unwrap_or(NEUTRAL_SLEEP_STRESS));

    let total = SCORE_WEIGHTS.sufficiency * suf
        + SCORE_WEIGHTS.efficiency * eff
        + SCORE_WEIGHTS.restorative * rest
        + SCORE_WEIGHTS.consistency * cons
        + SCORE_WEIGHTS.sleep_stress * stress;

    PerformanceScore {
        components: PerformanceComponents {
            sufficiency: suf,
            efficiency: eff,
            restorative: rest,
            consistency: cons,
            sleep_stress: stress,
        },
        total: total.clamp(0.0, 100.0),
    }
}

fn sufficiency_score(actual: f64, need: f64) -> f64 {
    if need <= 0.0 {
        return 0.0;
    }
    (actual / need * 100.0).clamp(0.0, 100.0)
}

fn restorative_score(restorative_pct: f64) -> f64 {
    (restorative_pct / RESTORATIVE_TARGET_PCT * 100.0).clamp(0.0, 100.0)
}

/// Invert the Baevsky sleep-stress score (0 = calm, 10 = stressed)
/// into a 0–100 "sleep stress" sub-score where 100 = ideal.
fn sleep_stress_score(avg_stress_0_10: f64) -> f64 {
    (100.0 - avg_stress_0_10.clamp(0.0, 10.0) * 10.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- sleep need -----

    #[test]
    fn need_base_only_returns_base() {
        let inputs = SleepNeedInputs::default();
        assert_eq!(sleep_need_hours(&inputs), BASE_NEED_HOURS);
    }

    #[test]
    fn strain_addition_is_zero_at_zero_and_monotonic() {
        assert_eq!(strain_addition_hours(0.0), 0.0);
        assert!(strain_addition_hours(5.0) < strain_addition_hours(10.0));
        assert!(strain_addition_hours(10.0) < strain_addition_hours(15.0));
        assert!(strain_addition_hours(15.0) < strain_addition_hours(21.0));
    }

    #[test]
    fn strain_addition_capped_at_max() {
        // Strain 21 should be ≤ 90 min = 1.5 h. Even absurd strain
        // shouldn't exceed the cap.
        assert!(strain_addition_hours(21.0) <= STRAIN_ADJ_CAP_MIN / 60.0 + 1e-9);
        assert!(strain_addition_hours(100.0) <= STRAIN_ADJ_CAP_MIN / 60.0 + 1e-9);
    }

    #[test]
    fn strain_addition_is_superlinear_past_threshold() {
        // From 10→15 (5 units past threshold) should add much more
        // than 0→5 (pure linear creep).
        let creep_0_to_5 = strain_addition_hours(5.0);
        let jump_10_to_15 = strain_addition_hours(15.0) - strain_addition_hours(10.0);
        assert!(jump_10_to_15 > creep_0_to_5 * 4.0);
    }

    #[test]
    fn need_clamped_above_max() {
        let inputs = SleepNeedInputs {
            prior_day_strain: Some(21.0),
            rolling_7d_debt_hours: 20.0, // debt adj capped at 2.0
            ..Default::default()
        };
        // 7.5 + ~1.17 + 2.0 = 10.67 → clamped to MAX (10.5).
        assert_eq!(sleep_need_hours(&inputs), MAX_SLEEP_NEED_HOURS);
    }

    #[test]
    fn need_clamped_below_min() {
        let inputs = SleepNeedInputs {
            base_need_hours: Some(5.0),
            nap_minutes: 180.0, // 3h nap × 1.0 credit = 3.0h
            ..Default::default()
        };
        // 5 - 3 = 2 → clamped to 6.0.
        assert_eq!(sleep_need_hours(&inputs), MIN_SLEEP_NEED_HOURS);
    }

    #[test]
    fn need_nap_credit_full() {
        let inputs = SleepNeedInputs {
            nap_minutes: 60.0, // 1h nap × 1.0 credit = 1.0h
            ..Default::default()
        };
        assert!((sleep_need_hours(&inputs) - (BASE_NEED_HOURS - 1.0)).abs() < 1e-9);
    }

    #[test]
    fn surplus_banking_off_by_default() {
        let inputs = SleepNeedInputs {
            rolling_7d_surplus_hours: 5.0, // ignored
            ..Default::default()
        };
        assert_eq!(sleep_need_hours(&inputs), BASE_NEED_HOURS);
    }

    #[test]
    fn surplus_banking_reduces_need_when_enabled() {
        let inputs = SleepNeedInputs {
            rolling_7d_surplus_hours: 2.0,
            allow_surplus_banking: true,
            ..Default::default()
        };
        // 7.5 - (2.0 * 0.3) = 6.9
        assert!((sleep_need_hours(&inputs) - (BASE_NEED_HOURS - 0.6)).abs() < 1e-9);
    }

    #[test]
    fn surplus_credit_capped() {
        let inputs = SleepNeedInputs {
            rolling_7d_surplus_hours: 100.0,
            allow_surplus_banking: true,
            ..Default::default()
        };
        // Credit capped at SURPLUS_CREDIT_CAP_HOURS = 1.0.
        assert!((sleep_need_hours(&inputs) - (BASE_NEED_HOURS - 1.0)).abs() < 1e-9);
    }

    // ----- sleep debt -----

    #[test]
    fn debt_zero_when_fully_rested() {
        let nights = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 8.0,
            };
            7
        ];
        assert_eq!(sleep_debt_hours(&nights), 0.0);
    }

    #[test]
    fn debt_weighted_mean_is_one_hour_for_sustained_one_hour_deficit() {
        // 1h shortfall each night for 7 nights → weighted mean = 1.0h.
        let nights = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 7.0,
            };
            7
        ];
        assert!((sleep_debt_hours(&nights) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn debt_ignores_extra_nights() {
        // Only first 7 decay weights exist; anything beyond is dropped.
        let mut nights = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 7.0,
            };
            10
        ];
        let with_extras = sleep_debt_hours(&nights);
        nights.truncate(7);
        let without_extras = sleep_debt_hours(&nights);
        assert_eq!(with_extras, without_extras);
    }

    // ----- surplus banking -----

    #[test]
    fn surplus_zero_when_fully_rested() {
        let nights = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 8.0,
            };
            7
        ];
        assert_eq!(sleep_surplus_hours(&nights), 0.0);
    }

    #[test]
    fn surplus_weighted_mean_is_one_hour_for_sustained_one_hour_excess() {
        let nights = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 9.0,
            };
            7
        ];
        assert!((sleep_surplus_hours(&nights) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn surplus_is_one_sided() {
        // Mixing shortfall and excess doesn't cross-subtract: each
        // fn only sees its own sign.
        let nights = vec![
            NightSleep { sleep_need_hours: 8.0, actual_sleep_hours: 10.0 }, // +2h
            NightSleep { sleep_need_hours: 8.0, actual_sleep_hours: 6.0 },  // −2h
        ];
        assert!(sleep_surplus_hours(&nights) > 0.0);
        assert!(sleep_debt_hours(&nights) > 0.0);
    }

    // ----- personalized baseline -----

    #[test]
    fn baseline_empty_returns_population_default() {
        assert_eq!(personalized_baseline_hours(&[]), BASE_NEED_HOURS);
    }

    #[test]
    fn baseline_ineligible_nights_return_population_default() {
        // High strain + low efficiency + had-nap → filtered out.
        let nights = vec![
            BaselineNight {
                actual_sleep_hours: 9.0,
                sleep_efficiency_pct: 60.0,
                day_strain: 15.0,
                had_nap: true,
            };
            20
        ];
        assert_eq!(personalized_baseline_hours(&nights), BASE_NEED_HOURS);
    }

    #[test]
    fn baseline_converges_to_personal_mean_with_enough_eligible() {
        // 14 eligible nights of 8.2h → alpha=1.0, trimmed mean = 8.2.
        let nights = vec![
            BaselineNight {
                actual_sleep_hours: 8.2,
                sleep_efficiency_pct: 92.0,
                day_strain: 5.0,
                had_nap: false,
            };
            14
        ];
        assert!((personalized_baseline_hours(&nights) - 8.2).abs() < 1e-9);
    }

    #[test]
    fn baseline_blends_toward_default_with_few_nights() {
        // 7 eligible nights of 9.0h → alpha = 7/14 = 0.5.
        // Blended = 0.5 * 9.0 + 0.5 * 7.5 = 8.25.
        let nights = vec![
            BaselineNight {
                actual_sleep_hours: 9.0,
                sleep_efficiency_pct: 92.0,
                day_strain: 5.0,
                had_nap: false,
            };
            7
        ];
        assert!((personalized_baseline_hours(&nights) - 8.25).abs() < 1e-9);
    }

    #[test]
    fn baseline_eligible_nights_counts_correctly() {
        let mut nights = vec![
            BaselineNight {
                actual_sleep_hours: 8.0,
                sleep_efficiency_pct: 92.0,
                day_strain: 5.0,
                had_nap: false,
            };
            5
        ];
        nights.push(BaselineNight {
            actual_sleep_hours: 8.0,
            sleep_efficiency_pct: 92.0,
            day_strain: 18.0, // high strain — filtered
            had_nap: false,
        });
        assert_eq!(baseline_eligible_nights(&nights), 5);
    }

    // ----- original decay-over-time sanity -----

    #[test]
    fn debt_decays_over_time() {
        // A single bad night 7 days ago weighted at 0.15 contributes
        // less than the same night yesterday at 1.0.
        let old_first = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 8.0,
            };
            6
        ]
        .into_iter()
        .chain(std::iter::once(NightSleep {
            sleep_need_hours: 8.0,
            actual_sleep_hours: 6.0,
        }))
        .collect::<Vec<_>>();
        let recent_first = std::iter::once(NightSleep {
            sleep_need_hours: 8.0,
            actual_sleep_hours: 6.0,
        })
        .chain(vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 8.0,
            };
            6
        ])
        .collect::<Vec<_>>();
        assert!(sleep_debt_hours(&recent_first) > sleep_debt_hours(&old_first));
    }

    // ----- performance score -----

    #[test]
    fn sufficiency_exact_need_gives_100() {
        let inputs = ScoringInputs {
            actual_sleep_hours: 8.0,
            sleep_need_hours: 8.0,
            sleep_efficiency: 90.0,
            deep_pct: 20.0,
            rem_pct: 25.0,
            consistency_score: Some(80.0),
            avg_sleep_stress: Some(2.0),
        };
        let score = performance_score(&inputs);
        assert_eq!(score.components.sufficiency, 100.0);
    }

    #[test]
    fn sufficiency_half_need_gives_50() {
        let inputs = ScoringInputs {
            actual_sleep_hours: 4.0,
            sleep_need_hours: 8.0,
            sleep_efficiency: 0.0,
            deep_pct: 0.0,
            rem_pct: 0.0,
            consistency_score: None,
            avg_sleep_stress: None,
        };
        assert_eq!(performance_score(&inputs).components.sufficiency, 50.0);
    }

    #[test]
    fn restorative_target_gives_100() {
        // Deep + REM = 45% → restorative = 100
        let inputs = ScoringInputs {
            actual_sleep_hours: 8.0,
            sleep_need_hours: 8.0,
            sleep_efficiency: 0.0,
            deep_pct: 20.0,
            rem_pct: 25.0,
            consistency_score: None,
            avg_sleep_stress: None,
        };
        assert!((performance_score(&inputs).components.restorative - 100.0).abs() < 1e-9);
    }

    #[test]
    fn performance_total_matches_weighted_sum() {
        let inputs = ScoringInputs {
            actual_sleep_hours: 8.0,
            sleep_need_hours: 8.0,
            sleep_efficiency: 80.0,
            deep_pct: 18.0,  // restorative = (18+18)/45 * 100 = 80
            rem_pct: 18.0,
            consistency_score: Some(70.0),
            avg_sleep_stress: Some(3.0), // stress score = 100 - 30 = 70
        };
        let score = performance_score(&inputs);
        let expected = SCORE_WEIGHTS.sufficiency * 100.0
            + SCORE_WEIGHTS.efficiency * 80.0
            + SCORE_WEIGHTS.restorative * 80.0
            + SCORE_WEIGHTS.consistency * 70.0
            + SCORE_WEIGHTS.sleep_stress * 70.0;
        assert!((score.total - expected).abs() < 1e-9);
    }

    #[test]
    fn stress_max_gives_zero() {
        let inputs = ScoringInputs {
            actual_sleep_hours: 8.0,
            sleep_need_hours: 8.0,
            sleep_efficiency: 100.0,
            deep_pct: 0.0,
            rem_pct: 0.0,
            consistency_score: None,
            avg_sleep_stress: Some(10.0),
        };
        assert_eq!(performance_score(&inputs).components.sleep_stress, 0.0);
    }

    #[test]
    fn missing_consistency_uses_neutral() {
        let inputs = ScoringInputs {
            actual_sleep_hours: 8.0,
            sleep_need_hours: 8.0,
            sleep_efficiency: 50.0,
            deep_pct: 0.0,
            rem_pct: 0.0,
            consistency_score: None,
            avg_sleep_stress: None,
        };
        assert_eq!(
            performance_score(&inputs).components.consistency,
            NEUTRAL_CONSISTENCY
        );
    }
}
