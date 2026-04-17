//! Nightly skin-temperature baseline and deviation.
//!
//! PRD §5.6: nightly skin temp is the median of samples taken from
//! the *middle 50% of the sleep window*. Restricting to the middle
//! half intentionally excludes sleep-onset cooling and wake-up
//! warming — both artifacts of the transition, not steady-state sleep
//! thermoregulation. Deviation from a rolling 14-night baseline is
//! what gets surfaced; small absolute-offset errors in the sensor
//! cancel out in the deviation.

use chrono::NaiveDateTime;

/// Threshold (°C) above which a nightly deviation is flagged as
/// significant (illness / fever / stress signal). PRD §5.6.
pub const SKIN_TEMP_DEVIATION_FLAG_C: f64 = 0.5;

/// Compute tonight's median skin temp from the middle 50% of the
/// sleep window. `samples` must be pre-filtered to the sleep window
/// and ordered ascending by time. Returns `None` if fewer than 4
/// samples land in the middle half.
pub fn nightly_skin_temp(
    sleep_start: NaiveDateTime,
    sleep_end: NaiveDateTime,
    samples: &[(NaiveDateTime, f64)],
) -> Option<f64> {
    let total = sleep_end
        .signed_duration_since(sleep_start)
        .num_milliseconds();
    if total <= 0 {
        return None;
    }
    let quarter = total / 4;
    let mid_start = sleep_start + chrono::Duration::milliseconds(quarter);
    let mid_end = sleep_start + chrono::Duration::milliseconds(3 * quarter);

    let mut in_range: Vec<f64> = samples
        .iter()
        .filter(|(t, _)| *t >= mid_start && *t <= mid_end)
        .map(|(_, v)| *v)
        .collect();
    if in_range.len() < 4 {
        return None;
    }
    in_range.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let len = in_range.len();
    let median = if len.is_multiple_of(2) {
        (in_range[len / 2 - 1] + in_range[len / 2]) / 2.0
    } else {
        in_range[len / 2]
    };
    Some(median)
}

/// Signed deviation (°C) of tonight's skin temp from the rolling
/// baseline. `None` when the baseline hasn't been established yet.
pub fn deviation_from_baseline(nightly: f64, baseline: Option<f64>) -> Option<f64> {
    baseline.map(|b| nightly - b)
}

/// Whether a deviation warrants flagging (|deviation| > 0.5 °C).
pub fn is_flag_worthy(deviation: f64) -> bool {
    deviation.abs() > SKIN_TEMP_DEVIATION_FLAG_C
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeDelta};

    fn dt(m: i64) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 16)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap()
            + TimeDelta::minutes(m)
    }

    #[test]
    fn nightly_temp_uses_middle_half_only() {
        let start = dt(0);
        let end = dt(480); // 8h window, middle half = 120..360 min
        // Samples: outer quarters at 30 and 32 degrees, middle at 33.
        let mut samples: Vec<(NaiveDateTime, f64)> = Vec::new();
        // 0-120: should be excluded (low values that would pull median)
        for i in 0..120 {
            samples.push((dt(i), 30.0));
        }
        // 120-360: included, value = 33
        for i in 120..360 {
            samples.push((dt(i), 33.0));
        }
        // 360-480: excluded
        for i in 360..480 {
            samples.push((dt(i), 32.0));
        }
        let nightly = nightly_skin_temp(start, end, &samples).unwrap();
        assert!((nightly - 33.0).abs() < 1e-9);
    }

    #[test]
    fn nightly_temp_none_when_too_few_samples() {
        let start = dt(0);
        let end = dt(480);
        let samples: Vec<(NaiveDateTime, f64)> = vec![(dt(240), 33.0)];
        assert!(nightly_skin_temp(start, end, &samples).is_none());
    }

    #[test]
    fn nightly_temp_none_when_window_invalid() {
        let start = dt(0);
        let samples: Vec<(NaiveDateTime, f64)> = (0..100).map(|i| (dt(i), 33.0)).collect();
        assert!(nightly_skin_temp(start, start, &samples).is_none());
    }

    #[test]
    fn median_even_count() {
        // 4 samples inside middle half: [32, 33, 34, 35] → median = 33.5
        let start = dt(0);
        let end = dt(480);
        let samples = vec![
            (dt(150), 35.0),
            (dt(200), 33.0),
            (dt(250), 32.0),
            (dt(300), 34.0),
        ];
        let nightly = nightly_skin_temp(start, end, &samples).unwrap();
        assert!((nightly - 33.5).abs() < 1e-9);
    }

    #[test]
    fn deviation_sign_and_missing() {
        assert!((deviation_from_baseline(33.5, Some(33.0)).unwrap() - 0.5).abs() < 1e-9);
        assert!((deviation_from_baseline(32.2, Some(33.0)).unwrap() - (-0.8)).abs() < 1e-9);
        assert_eq!(deviation_from_baseline(33.0, None), None);
    }

    #[test]
    fn flag_threshold() {
        assert!(!is_flag_worthy(0.49));
        assert!(!is_flag_worthy(0.5));
        assert!(is_flag_worthy(0.51));
        assert!(is_flag_worthy(-0.6));
    }
}
