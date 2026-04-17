//! Nightly respiratory-rate aggregation.
//!
//! Per-epoch respiratory rate is computed inside
//! [`super::features::EpochFeatures::resp_rate`] via RR-interval
//! modulation (respiratory sinus arrhythmia). This module rolls those
//! per-epoch values up to one nightly number.

use super::features::EpochFeatures;

#[derive(Debug, Clone, PartialEq)]
pub struct RespiratoryStats {
    pub avg: f64,
    pub min: f64,
    pub max: f64,
    /// Number of epochs that contributed a valid resp-rate estimate.
    pub sample_count: usize,
}

/// Average / min / max respiratory rate across epochs with a valid
/// `resp_rate` estimate. Returns `None` if no epoch yielded a value.
pub fn nightly_respiratory_rate(features: &[EpochFeatures]) -> Option<RespiratoryStats> {
    let samples: Vec<f64> = features.iter().filter_map(|f| f.resp_rate).collect();
    if samples.is_empty() {
        return None;
    }
    let sum: f64 = samples.iter().sum();
    let avg = sum / samples.len() as f64;
    let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let max = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(RespiratoryStats {
        avg,
        min,
        max,
        sample_count: samples.len(),
    })
}

/// Absolute deviation of tonight's avg respiratory rate from the
/// user's 14-night baseline (breaths/min). PRD §5.5 flags the night
/// when this exceeds 2 bpm (illness signal).
pub fn resp_rate_deviation_from_baseline(nightly_avg: f64, baseline: Option<f64>) -> Option<f64> {
    baseline.map(|b| nightly_avg - b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fe(resp: Option<f64>) -> EpochFeatures {
        EpochFeatures {
            resp_rate: resp,
            is_valid: resp.is_some(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_returns_none() {
        assert!(nightly_respiratory_rate(&[]).is_none());
    }

    #[test]
    fn all_invalid_returns_none() {
        let features = vec![fe(None), fe(None), fe(None)];
        assert!(nightly_respiratory_rate(&features).is_none());
    }

    #[test]
    fn avg_min_max_computed() {
        let features = vec![fe(Some(12.0)), fe(Some(14.0)), fe(Some(16.0)), fe(None)];
        let stats = nightly_respiratory_rate(&features).unwrap();
        assert_eq!(stats.avg, 14.0);
        assert_eq!(stats.min, 12.0);
        assert_eq!(stats.max, 16.0);
        assert_eq!(stats.sample_count, 3);
    }

    #[test]
    fn deviation_none_when_no_baseline() {
        assert!(resp_rate_deviation_from_baseline(14.0, None).is_none());
    }

    #[test]
    fn deviation_signed_against_baseline() {
        assert_eq!(resp_rate_deviation_from_baseline(16.0, Some(14.0)), Some(2.0));
        assert_eq!(resp_rate_deviation_from_baseline(12.0, Some(14.0)), Some(-2.0));
    }
}
