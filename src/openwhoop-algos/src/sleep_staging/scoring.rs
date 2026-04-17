//! Sleep need, debt, and composite performance score.
//!
//! Pure functions over numeric inputs; persistence + aggregation is
//! the caller's responsibility. Every coefficient lives in
//! [`super::constants`] so future config overrides only touch one
//! place.

use super::constants::{
    BASE_NEED_HOURS, DEBT_ADJ_CAP_HOURS, DEBT_ADJ_COEF, DEBT_DECAY_WEIGHTS, MAX_SLEEP_NEED_HOURS,
    MIN_SLEEP_NEED_HOURS, NAP_CREDIT_FRAC, NEUTRAL_CONSISTENCY, NEUTRAL_SLEEP_STRESS,
    RESTORATIVE_TARGET_PCT, SCORE_WEIGHTS, STRAIN_ADJ_COEF,
};

/// Inputs for the per-night sleep-need calculation (PRD §5.7).
#[derive(Debug, Clone, Default)]
pub struct SleepNeedInputs {
    /// Defaults to `BASE_NEED_HOURS`. Override with the user's own
    /// mean on recovered nights once 30+ nights of data exist.
    pub base_need_hours: Option<f64>,
    /// Prior-day WHOOP-scale strain (0–21). `None` = unknown.
    pub prior_day_strain: Option<f64>,
    /// Rolling 7-day debt (already decay-weighted) in hours.
    pub rolling_7d_debt_hours: f64,
    /// Total nap minutes in the prior day. Half-credited toward
    /// tonight's need.
    pub nap_minutes: f64,
}

/// Compute personalized sleep need for tonight, in hours, clamped to
/// [`MIN_SLEEP_NEED_HOURS`, `MAX_SLEEP_NEED_HOURS`].
pub fn sleep_need_hours(inputs: &SleepNeedInputs) -> f64 {
    let base = inputs.base_need_hours.unwrap_or(BASE_NEED_HOURS);
    let strain_adj = inputs.prior_day_strain.unwrap_or(0.0) * STRAIN_ADJ_COEF;
    let debt_adj = (inputs.rolling_7d_debt_hours * DEBT_ADJ_COEF).min(DEBT_ADJ_CAP_HOURS);
    let nap_credit = inputs.nap_minutes * NAP_CREDIT_FRAC / 60.0;
    let need = base + strain_adj + debt_adj - nap_credit;
    need.clamp(MIN_SLEEP_NEED_HOURS, MAX_SLEEP_NEED_HOURS)
}

/// One historical night's sleep need and what was actually slept.
#[derive(Debug, Clone, Copy)]
pub struct NightSleep {
    pub sleep_need_hours: f64,
    pub actual_sleep_hours: f64,
}

/// Rolling 7-day sleep debt with decay weighting (PRD §5.8).
/// `recent_nights` is ordered most-recent-first; anything beyond 7
/// entries is ignored. If fewer than 7 nights are provided, the
/// available ones are used with their respective decay weights.
pub fn sleep_debt_hours(recent_nights: &[NightSleep]) -> f64 {
    recent_nights
        .iter()
        .zip(DEBT_DECAY_WEIGHTS.iter())
        .map(|(night, &w)| {
            let shortfall = (night.sleep_need_hours - night.actual_sleep_hours).max(0.0);
            shortfall * w
        })
        .sum()
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
    fn need_strain_adjustment_scales_linearly() {
        let inputs = SleepNeedInputs {
            prior_day_strain: Some(21.0),
            ..Default::default()
        };
        // 7.5 + 21 * 0.05 = 8.55
        assert!((sleep_need_hours(&inputs) - 8.55).abs() < 1e-9);
    }

    #[test]
    fn need_clamped_above_max() {
        let inputs = SleepNeedInputs {
            prior_day_strain: Some(21.0),
            rolling_7d_debt_hours: 20.0, // debt adj capped at 2.0
            ..Default::default()
        };
        // 7.5 + 1.05 + 2.0 = 10.55 -> clamped to 10.0
        assert_eq!(sleep_need_hours(&inputs), MAX_SLEEP_NEED_HOURS);
    }

    #[test]
    fn need_clamped_below_min() {
        let inputs = SleepNeedInputs {
            base_need_hours: Some(5.0),
            nap_minutes: 300.0, // 2.5h half-credit = 1.25h
            ..Default::default()
        };
        // 5 - 1.25 = 3.75 -> clamped to 6.0
        assert_eq!(sleep_need_hours(&inputs), MIN_SLEEP_NEED_HOURS);
    }

    #[test]
    fn need_nap_credit_reduces_need() {
        let inputs = SleepNeedInputs {
            nap_minutes: 60.0, // 1h nap × 0.5 credit = 0.5h
            ..Default::default()
        };
        assert!((sleep_need_hours(&inputs) - (BASE_NEED_HOURS - 0.5)).abs() < 1e-9);
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
    fn debt_decay_weights_applied() {
        // 1h shortfall each night for 7 nights -> sum of weights = 3.9
        let nights = vec![
            NightSleep {
                sleep_need_hours: 8.0,
                actual_sleep_hours: 7.0,
            };
            7
        ];
        let expected = DEBT_DECAY_WEIGHTS.iter().sum::<f64>();
        assert!((sleep_debt_hours(&nights) - expected).abs() < 1e-9);
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
