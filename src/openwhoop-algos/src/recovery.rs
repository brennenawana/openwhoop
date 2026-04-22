//! Recovery score — a readiness metric modeled on Whoop's recovery.
//!
//! Inputs are pulled from the most recent completed sleep cycle:
//!   - HRV (RMSSD ms)
//!   - Resting HR (min bpm during sleep is a good proxy)
//!   - Avg respiratory rate
//!   - Sleep performance score
//!   - Skin temp deviation
//!
//! Each input is converted to a z-score against a 14-night personal
//! baseline, oriented so "positive = better for recovery." The z-scores
//! are combined with HRV-dominant weights, then mapped to 0–100 with
//! red/yellow/green bands at <34 / 34–66 / 67+.
//!
//! Whoop's exact formula is proprietary; this is a directionally-correct
//! approximation based on Whoop's publicly-stated inputs + weighting
//! hierarchy. See tests below for calibration assumptions.

/// Weights sum to 1.0; HRV dominant, per Whoop's published ordering
/// (HRV > RHR > sleep > respiratory rate > skin temp).
const WEIGHT_HRV: f64 = 0.40;
const WEIGHT_RHR: f64 = 0.25;
const WEIGHT_SLEEP: f64 = 0.20;
const WEIGHT_RR: f64 = 0.10;
const WEIGHT_SKIN_TEMP: f64 = 0.05;

/// Output scaling: a weighted z-score of 0 → 50, ±3 SD → pegged at 5/95.
const SCORE_MID: f64 = 50.0;
const SCORE_SCALE: f64 = 15.0;

pub const BASELINE_WINDOW_NIGHTS: usize = 14;
pub const MIN_BASELINE_NIGHTS: usize = 3;

/// Per-night inputs. `hrv_rmssd_ms` and `rhr_bpm` are required (recovery
/// is meaningless without them); the rest are optional because they may
/// be absent on older cycles or when the strap was off-wrist at the
/// relevant part of the night.
#[derive(Clone, Copy, Debug)]
pub struct RecoveryNight {
    pub hrv_rmssd_ms: f64,
    pub rhr_bpm: f64,
    pub avg_resp_rate: Option<f64>,
    pub sleep_performance_score: Option<f64>,
    pub skin_temp_deviation_c: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct BaselineStat {
    mean: f64,
    sd: f64,
    n: usize,
}

impl BaselineStat {
    fn from_iter<I: IntoIterator<Item = f64>>(values: I) -> Self {
        let v: Vec<f64> = values.into_iter().collect();
        let n = v.len();
        if n == 0 {
            return Self::default();
        }
        let mean = v.iter().copied().sum::<f64>() / n as f64;
        let var = if n > 1 {
            v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64
        } else {
            0.0
        };
        Self {
            mean,
            sd: var.sqrt(),
            n,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RecoveryZ {
    pub hrv: Option<f64>,
    pub rhr: Option<f64>,
    pub sleep: Option<f64>,
    pub rr: Option<f64>,
    pub skin_temp: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecoveryBand {
    Red,
    Yellow,
    Green,
}

impl RecoveryBand {
    fn from_score(score: f64) -> Self {
        if score < 34.0 {
            Self::Red
        } else if score < 67.0 {
            Self::Yellow
        } else {
            Self::Green
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecoveryDriver {
    Hrv,
    Rhr,
    Sleep,
    Rr,
    SkinTemp,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryScore {
    /// 0–100, higher = better readiness.
    pub score: f64,
    pub band: RecoveryBand,
    /// The metric pulling the score down the most (None when no metric's
    /// weighted contribution is materially negative).
    pub dominant_driver: RecoveryDriver,
    pub z_scores: RecoveryZ,
    /// How many nights were actually used for the baseline (≤
    /// [`BASELINE_WINDOW_NIGHTS`]).
    pub baseline_window_nights: usize,
    /// True while baseline_window_nights < [`BASELINE_WINDOW_NIGHTS`].
    pub calibrating: bool,
}

/// Compute a recovery score for `night` using `baseline` (other recent
/// nights, not including `night`). Returns `None` if baseline is too
/// short to produce a meaningful score.
///
/// `baseline` may be longer than the window; only the first
/// [`BASELINE_WINDOW_NIGHTS`] entries are used, so callers should pass
/// most-recent-first.
pub fn compute_recovery(
    night: &RecoveryNight,
    baseline: &[RecoveryNight],
) -> Option<RecoveryScore> {
    if baseline.len() < MIN_BASELINE_NIGHTS {
        return None;
    }
    let window: Vec<RecoveryNight> = baseline
        .iter()
        .take(BASELINE_WINDOW_NIGHTS)
        .copied()
        .collect();

    let b_hrv = BaselineStat::from_iter(window.iter().map(|n| n.hrv_rmssd_ms));
    let b_rhr = BaselineStat::from_iter(window.iter().map(|n| n.rhr_bpm));
    let b_rr = BaselineStat::from_iter(window.iter().filter_map(|n| n.avg_resp_rate));
    let b_sleep =
        BaselineStat::from_iter(window.iter().filter_map(|n| n.sleep_performance_score));
    // Skin temp: magnitude of deviation matters regardless of sign.
    let b_skin = BaselineStat::from_iter(
        window
            .iter()
            .filter_map(|n| n.skin_temp_deviation_c.map(f64::abs)),
    );

    // Z-scores: sign chosen so positive = better recovery.
    let z_hrv = safe_z(night.hrv_rmssd_ms, &b_hrv, 1.0);
    let z_rhr = safe_z(night.rhr_bpm, &b_rhr, -1.0);
    let z_rr = night.avg_resp_rate.and_then(|x| safe_z(x, &b_rr, -1.0));
    let z_sleep = night
        .sleep_performance_score
        .and_then(|x| safe_z(x, &b_sleep, 1.0));
    let z_skin = night
        .skin_temp_deviation_c
        .and_then(|x| safe_z(x.abs(), &b_skin, -1.0));

    let contributions = [
        (RecoveryDriver::Hrv, z_hrv, WEIGHT_HRV),
        (RecoveryDriver::Rhr, z_rhr, WEIGHT_RHR),
        (RecoveryDriver::Sleep, z_sleep, WEIGHT_SLEEP),
        (RecoveryDriver::Rr, z_rr, WEIGHT_RR),
        (RecoveryDriver::SkinTemp, z_skin, WEIGHT_SKIN_TEMP),
    ];

    // Missing inputs simply don't contribute. Weights are NOT renormalized
    // so that a night with only HRV+RHR sits closer to the neutral 50 than
    // it would with full data — reflecting less certainty, not false
    // confidence.
    let sum: f64 = contributions
        .iter()
        .filter_map(|(_, z, w)| z.map(|z| z * w))
        .sum();
    let score = (SCORE_MID + sum * SCORE_SCALE).clamp(0.0, 100.0);

    let dominant_driver = contributions
        .iter()
        .filter_map(|(d, z, w)| z.map(|z| (*d, z * w)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(d, contrib)| {
            if contrib < -0.05 {
                d
            } else {
                RecoveryDriver::None
            }
        })
        .unwrap_or(RecoveryDriver::None);

    Some(RecoveryScore {
        score,
        band: RecoveryBand::from_score(score),
        dominant_driver,
        z_scores: RecoveryZ {
            hrv: z_hrv,
            rhr: z_rhr,
            sleep: z_sleep,
            rr: z_rr,
            skin_temp: z_skin,
        },
        baseline_window_nights: window.len(),
        calibrating: window.len() < BASELINE_WINDOW_NIGHTS,
    })
}

fn safe_z(x: f64, b: &BaselineStat, sign: f64) -> Option<f64> {
    if b.n < MIN_BASELINE_NIGHTS {
        return None;
    }
    if b.sd < 1e-6 {
        return Some(0.0);
    }
    Some(sign * (x - b.mean) / b.sd)
}

/// Age-specific RMSSD percentiles (ms) drawn from published resting-HRV
/// normative data: Nunan et al. 2010 (systematic review of healthy
/// short-term HRV), Voss et al. 2015 (short-term resting HRV in ~1900
/// subjects), Umetani et al. 1998 (age + sex norms).
///
/// These are **daytime-resting** reference points. Sleep RMSSD typically
/// runs 20–40 % higher than daytime resting, so scores computed against
/// these will tend to sit above 50 for a healthy sleep window — which
/// accurately reflects sleep's physiological advantage rather than
/// being a calibration error.
#[derive(Clone, Copy, Debug)]
struct AgeHrvNorm {
    p10: f64,
    p50: f64,
    p90: f64,
}

fn age_norm(age_years: u32) -> AgeHrvNorm {
    match age_years {
        0..=19 => AgeHrvNorm {
            p10: 25.0,
            p50: 55.0,
            p90: 90.0,
        },
        20..=29 => AgeHrvNorm {
            p10: 20.0,
            p50: 42.0,
            p90: 75.0,
        },
        30..=39 => AgeHrvNorm {
            p10: 16.0,
            p50: 35.0,
            p90: 60.0,
        },
        40..=49 => AgeHrvNorm {
            p10: 12.0,
            p50: 28.0,
            p90: 50.0,
        },
        50..=59 => AgeHrvNorm {
            p10: 10.0,
            p50: 23.0,
            p90: 42.0,
        },
        60..=69 => AgeHrvNorm {
            p10: 8.0,
            p50: 18.0,
            p90: 35.0,
        },
        _ => AgeHrvNorm {
            p10: 6.0,
            p50: 15.0,
            p90: 30.0,
        },
    }
}

/// Map an RMSSD measurement (ms) to a 0–100 score against age-matched
/// population norms. Anchor points: 0 ms → 0, p10 → 25, p50 → 50,
/// p90 → 85, and above p90 the score extends slowly toward 100 (every
/// extra 50 % of p90 ≈ +15 points).
///
/// Unlike the baseline z-score used by [`compute_recovery`], this is a
/// static reference — it answers "how does my HRV compare to other
/// healthy people my age" rather than "how am I trending today vs my
/// own recent history." Callers typically want both: this score for a
/// health-facing readout, and the z-score for trend surfacing.
pub fn age_normed_hrv_score(hrv_rmssd_ms: f64, age_years: u32) -> f64 {
    let n = age_norm(age_years);
    let score = if hrv_rmssd_ms <= 0.0 {
        0.0
    } else if hrv_rmssd_ms <= n.p10 {
        (hrv_rmssd_ms / n.p10) * 25.0
    } else if hrv_rmssd_ms <= n.p50 {
        25.0 + ((hrv_rmssd_ms - n.p10) / (n.p50 - n.p10)) * 25.0
    } else if hrv_rmssd_ms <= n.p90 {
        50.0 + ((hrv_rmssd_ms - n.p50) / (n.p90 - n.p50)) * 35.0
    } else {
        let over = hrv_rmssd_ms - n.p90;
        let extra = (over / (n.p90 * 0.5)) * 15.0;
        85.0 + extra
    };
    score.clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn night(hrv: f64, rhr: f64) -> RecoveryNight {
        RecoveryNight {
            hrv_rmssd_ms: hrv,
            rhr_bpm: rhr,
            avg_resp_rate: None,
            sleep_performance_score: None,
            skin_temp_deviation_c: None,
        }
    }

    #[test]
    fn returns_none_when_baseline_too_short() {
        let today = night(50.0, 60.0);
        let baseline = vec![night(45.0, 62.0), night(48.0, 61.0)];
        assert!(compute_recovery(&today, &baseline).is_none());
    }

    #[test]
    fn score_near_mid_when_at_baseline_with_zero_variance() {
        let baseline: Vec<_> = (0..10).map(|_| night(50.0, 60.0)).collect();
        let today = night(50.0, 60.0);
        let r = compute_recovery(&today, &baseline).unwrap();
        assert!((r.score - SCORE_MID).abs() < 1.0);
        assert!(matches!(r.band, RecoveryBand::Yellow));
        assert!(matches!(r.dominant_driver, RecoveryDriver::None));
    }

    #[test]
    fn high_hrv_and_low_rhr_increases_score() {
        let baseline: Vec<_> = (0..10)
            .map(|i| night(40.0 + (i as f64) * 0.5, 60.0 + (i as f64) * 0.3))
            .collect();
        let today = night(100.0, 48.0);
        let r = compute_recovery(&today, &baseline).unwrap();
        assert!(r.score > 70.0, "expected high recovery, got {}", r.score);
        assert!(matches!(r.band, RecoveryBand::Green));
    }

    #[test]
    fn low_hrv_and_high_rhr_decreases_score() {
        let baseline: Vec<_> = (0..10)
            .map(|i| night(50.0 + (i as f64) * 0.5, 58.0 + (i as f64) * 0.3))
            .collect();
        let today = night(20.0, 75.0);
        let r = compute_recovery(&today, &baseline).unwrap();
        assert!(r.score < 30.0, "expected low recovery, got {}", r.score);
        assert!(matches!(r.band, RecoveryBand::Red));
        assert!(matches!(
            r.dominant_driver,
            RecoveryDriver::Hrv | RecoveryDriver::Rhr
        ));
    }

    #[test]
    fn calibrating_flag_set_for_fewer_than_14_nights() {
        let baseline: Vec<_> = (0..5).map(|_| night(50.0, 60.0)).collect();
        let today = night(55.0, 58.0);
        let r = compute_recovery(&today, &baseline).unwrap();
        assert!(r.calibrating);
        assert_eq!(r.baseline_window_nights, 5);
    }

    #[test]
    fn band_from_score_boundaries() {
        assert!(matches!(RecoveryBand::from_score(0.0), RecoveryBand::Red));
        assert!(matches!(RecoveryBand::from_score(33.9), RecoveryBand::Red));
        assert!(matches!(
            RecoveryBand::from_score(34.0),
            RecoveryBand::Yellow
        ));
        assert!(matches!(
            RecoveryBand::from_score(66.9),
            RecoveryBand::Yellow
        ));
        assert!(matches!(
            RecoveryBand::from_score(67.0),
            RecoveryBand::Green
        ));
        assert!(matches!(
            RecoveryBand::from_score(100.0),
            RecoveryBand::Green
        ));
    }

    #[test]
    fn window_is_capped_at_baseline_window() {
        let baseline: Vec<_> = (0..30).map(|_| night(50.0, 60.0)).collect();
        let today = night(50.0, 60.0);
        let r = compute_recovery(&today, &baseline).unwrap();
        assert_eq!(r.baseline_window_nights, BASELINE_WINDOW_NIGHTS);
        assert!(!r.calibrating);
    }

    // ---- age-normed HRV ----

    #[test]
    fn age_normed_hrv_score_hits_anchor_points() {
        // Age 36 → 30-39 bucket: p10=16, p50=35, p90=60.
        assert!((age_normed_hrv_score(0.0, 36) - 0.0).abs() < 1e-6);
        assert!((age_normed_hrv_score(16.0, 36) - 25.0).abs() < 1e-6);
        assert!((age_normed_hrv_score(35.0, 36) - 50.0).abs() < 1e-6);
        assert!((age_normed_hrv_score(60.0, 36) - 85.0).abs() < 1e-6);
    }

    #[test]
    fn age_normed_hrv_score_extends_past_p90_but_clamps_at_100() {
        // p90 = 60; +50% of p90 (= 90 ms total) should land near 100.
        let s = age_normed_hrv_score(90.0, 36);
        assert!(s > 99.0 && s <= 100.0, "got {s}");
        // Well beyond p90 pegs at 100.
        assert!((age_normed_hrv_score(200.0, 36) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn age_normed_hrv_score_age_shifts_the_scale() {
        // Same 35 ms reading: typical for 30s, well above typical for 60s.
        let thirties = age_normed_hrv_score(35.0, 36);
        let sixties = age_normed_hrv_score(35.0, 65);
        assert!((thirties - 50.0).abs() < 0.1);
        assert!(sixties > 75.0, "expected elder cohort to score higher, got {sixties}");
    }

    #[test]
    fn age_normed_hrv_score_below_p10_scales_linearly_to_zero() {
        // Half of p10 → half of the p10 score (25/2 = 12.5).
        let s = age_normed_hrv_score(8.0, 36);
        assert!((s - 12.5).abs() < 0.1, "got {s}");
    }

    #[test]
    fn missing_metrics_dampen_toward_mid() {
        // Baseline with realistic spread: HRV ~50 ± 10 ms, RHR ~60 ± 5 bpm.
        // Only HRV + RHR carry signal (the rest are missing on `today`),
        // so the weighted contribution maxes at 0.65 — a signal of
        // ~1 SD on each should pull the score noticeably, but not peg.
        let hrv_values = [38.0, 42.0, 45.0, 48.0, 50.0, 52.0, 55.0, 58.0, 62.0, 65.0];
        let rhr_values = [52.0, 55.0, 57.0, 59.0, 60.0, 61.0, 62.0, 63.0, 65.0, 68.0];
        let baseline: Vec<_> = (0..10)
            .map(|i| night(hrv_values[i], rhr_values[i]))
            .collect();
        // +1 SD better HRV (~60 + 10 = 70 is too much; pick ~60), -1 SD RHR
        let today = night(60.0, 56.0);
        let r = compute_recovery(&today, &baseline).unwrap();
        // With ~65% of weight contributing at ~1 SD each, expect the score
        // to land comfortably above the mid but nowhere near pegged.
        assert!(
            r.score > 55.0 && r.score < 75.0,
            "expected dampened-positive recovery, got {}",
            r.score
        );
    }
}
