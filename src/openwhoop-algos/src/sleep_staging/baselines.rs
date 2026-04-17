//! Rolling 14-night per-user baseline (PRD §5.10).
//!
//! Aggregates a window of [`NightAggregate`]s into a
//! [`BaselineSnapshot`] that the classifier and scoring modules use
//! as their "per-user normal." Pure over inputs; the DB layer is
//! responsible for fetching the most recent nights and persisting
//! the result.

use chrono::{NaiveDateTime, TimeDelta};

/// Nights to roll over. 14 is long enough to smooth weekday/weekend
/// variance while short enough to track genuine fitness changes
/// (PRD §5.10).
pub const BASELINE_WINDOW_NIGHTS: usize = 14;

/// Minimum elapsed time before recomputing the baseline. Prevents
/// per-sync-run thrash when the user opens the app frequently.
pub const BASELINE_MIN_UPDATE_INTERVAL_HOURS: i64 = 24;

/// Input: one night's summary, as extracted from `sleep_cycles` +
/// `sleep_epochs`.
#[derive(Debug, Clone, Default)]
pub struct NightAggregate {
    pub sleep_start: NaiveDateTime,
    pub sleep_end: NaiveDateTime,
    /// Per-night resting HR proxy — typically the minimum sleep HR,
    /// computed by the pipeline and passed through.
    pub resting_hr: Option<f64>,
    /// Per-epoch RMSSD values from this night's `sleep_epochs`.
    pub rmssd_samples: Vec<f64>,
    pub hf_power_samples: Vec<f64>,
    pub lf_hf_ratio_samples: Vec<f64>,
    /// Actual sleep time in hours (TST).
    pub duration_hours: Option<f64>,
    pub respiratory_rate_avg: Option<f64>,
    pub skin_temp_nightly_c: Option<f64>,
}

/// Output: values written to the `user_baselines` row. All fields are
/// `Option` so the first few nights of data yield a partial (but
/// still useful) baseline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BaselineSnapshot {
    pub window_nights: i32,
    pub resting_hr: Option<f64>,
    pub sleep_rmssd_median: Option<f64>,
    pub sleep_rmssd_p25: Option<f64>,
    pub sleep_rmssd_p75: Option<f64>,
    pub hf_power_median: Option<f64>,
    pub lf_hf_ratio_median: Option<f64>,
    pub sleep_duration_mean_hours: Option<f64>,
    pub respiratory_rate_mean: Option<f64>,
    pub respiratory_rate_std: Option<f64>,
    pub skin_temp_mean_c: Option<f64>,
    pub skin_temp_std_c: Option<f64>,
}

pub fn compute_baseline(nights: &[NightAggregate]) -> BaselineSnapshot {
    let window = nights.len().min(BASELINE_WINDOW_NIGHTS);
    let nights = &nights[..window];

    // Pooled per-epoch HRV samples across all nights. Using all
    // epochs (rather than per-night medians of epochs) matches the
    // classifier's threshold scale, which is a per-epoch percentile
    // look-up.
    let rmssd_pool: Vec<f64> = nights.iter().flat_map(|n| n.rmssd_samples.iter().copied()).collect();
    let hf_pool: Vec<f64> = nights.iter().flat_map(|n| n.hf_power_samples.iter().copied()).collect();
    let lfhf_pool: Vec<f64> = nights
        .iter()
        .flat_map(|n| n.lf_hf_ratio_samples.iter().copied())
        .collect();

    let resting_hr_values: Vec<f64> = nights.iter().filter_map(|n| n.resting_hr).collect();
    let duration_values: Vec<f64> = nights.iter().filter_map(|n| n.duration_hours).collect();
    let resp_values: Vec<f64> = nights.iter().filter_map(|n| n.respiratory_rate_avg).collect();
    let temp_values: Vec<f64> = nights.iter().filter_map(|n| n.skin_temp_nightly_c).collect();

    BaselineSnapshot {
        window_nights: window as i32,
        resting_hr: mean_opt(&resting_hr_values),
        sleep_rmssd_median: percentile(&rmssd_pool, 0.5),
        sleep_rmssd_p25: percentile(&rmssd_pool, 0.25),
        sleep_rmssd_p75: percentile(&rmssd_pool, 0.75),
        hf_power_median: percentile(&hf_pool, 0.5),
        lf_hf_ratio_median: percentile(&lfhf_pool, 0.5),
        sleep_duration_mean_hours: mean_opt(&duration_values),
        respiratory_rate_mean: mean_opt(&resp_values),
        respiratory_rate_std: std_opt(&resp_values),
        skin_temp_mean_c: mean_opt(&temp_values),
        skin_temp_std_c: std_opt(&temp_values),
    }
}

/// Idempotence gate. Returns `true` only when enough time has
/// elapsed since the last baseline write to justify recomputing.
pub fn should_update(last_update: Option<NaiveDateTime>, now: NaiveDateTime) -> bool {
    match last_update {
        None => true,
        Some(last) => {
            let elapsed = now.signed_duration_since(last);
            elapsed >= TimeDelta::hours(BASELINE_MIN_UPDATE_INTERVAL_HOURS)
        }
    }
}

fn mean_opt(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    Some(xs.iter().sum::<f64>() / xs.len() as f64)
}

fn std_opt(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let m = xs.iter().sum::<f64>() / xs.len() as f64;
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64;
    Some(var.sqrt())
}

fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((p * (v.len() - 1) as f64).round() as usize).min(v.len() - 1);
    Some(v[idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, d)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap()
    }

    fn night(day: u32, rmssds: &[f64], resting: f64, duration: f64) -> NightAggregate {
        NightAggregate {
            sleep_start: dt(day),
            sleep_end: dt(day + 1),
            resting_hr: Some(resting),
            rmssd_samples: rmssds.to_vec(),
            hf_power_samples: rmssds.to_vec(), // cheat: same values for test
            lf_hf_ratio_samples: vec![1.0],
            duration_hours: Some(duration),
            respiratory_rate_avg: Some(14.0),
            skin_temp_nightly_c: Some(33.0),
        }
    }

    #[test]
    fn empty_input_gives_empty_baseline() {
        let b = compute_baseline(&[]);
        assert_eq!(b.window_nights, 0);
        assert!(b.resting_hr.is_none());
        assert!(b.sleep_rmssd_median.is_none());
    }

    #[test]
    fn single_night_window_is_one() {
        let nights = vec![night(1, &[30.0, 40.0, 50.0], 60.0, 8.0)];
        let b = compute_baseline(&nights);
        assert_eq!(b.window_nights, 1);
        assert_eq!(b.sleep_rmssd_median, Some(40.0));
        assert_eq!(b.resting_hr, Some(60.0));
    }

    #[test]
    fn truncates_at_14_nights() {
        let nights: Vec<NightAggregate> = (1..=20)
            .map(|d| night(d, &[40.0], 60.0, 8.0))
            .collect();
        let b = compute_baseline(&nights);
        assert_eq!(b.window_nights, BASELINE_WINDOW_NIGHTS as i32);
    }

    #[test]
    fn percentile_pooled_across_nights() {
        let nights = vec![
            night(1, &[10.0, 20.0], 60.0, 8.0),
            night(2, &[30.0, 40.0], 60.0, 8.0),
            night(3, &[50.0, 60.0, 70.0], 60.0, 8.0),
        ];
        let b = compute_baseline(&nights);
        // Pool: [10, 20, 30, 40, 50, 60, 70] (7 values) - median idx 3 = 40
        assert_eq!(b.sleep_rmssd_median, Some(40.0));
    }

    #[test]
    fn resting_hr_mean() {
        let nights = vec![
            night(1, &[40.0], 58.0, 8.0),
            night(2, &[40.0], 62.0, 8.0),
            night(3, &[40.0], 60.0, 8.0),
        ];
        let b = compute_baseline(&nights);
        assert_eq!(b.resting_hr, Some(60.0));
    }

    #[test]
    fn respiratory_std_zero_when_constant() {
        let nights = vec![
            night(1, &[40.0], 60.0, 8.0),
            night(2, &[40.0], 60.0, 8.0),
        ];
        let b = compute_baseline(&nights);
        assert_eq!(b.respiratory_rate_std, Some(0.0));
    }

    #[test]
    fn should_update_true_when_no_prior() {
        let now = dt(10);
        assert!(should_update(None, now));
    }

    #[test]
    fn should_update_false_under_24h() {
        let now = dt(10);
        let last = now - TimeDelta::hours(23);
        assert!(!should_update(Some(last), now));
    }

    #[test]
    fn should_update_true_at_exactly_24h() {
        let now = dt(10);
        let last = now - TimeDelta::hours(24);
        assert!(should_update(Some(last), now));
    }
}
