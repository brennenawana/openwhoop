//! Daytime HRV samples — 5-minute RMSSD windows across waking hours.
//!
//! Complements the sleep-only RMSSD from `SleepCycle` by producing
//! RMSSD per 5-minute aligned window throughout the day. Skips sleep
//! windows (handled by existing algo) and non-wear windows (handled
//! by Feature 4). Aggressive quality gating on RR-interval
//! plausibility and signal coverage.

use chrono::{Duration, NaiveDateTime, Timelike};
use openwhoop_codec::ParsedHistoryReading;

/// Window size in minutes. Aligned to local time :00, :05, :10, ... —
/// simpler than sliding windows and matches PRD §10.2.
pub const WINDOW_MINUTES: i64 = 5;

/// Minimum seconds of heart_rate coverage inside a window (≥3 min).
pub const MIN_COVERAGE_SECS: i64 = 3 * 60;

/// Minimum raw RR intervals before filtering.
pub const MIN_RR_COUNT: usize = 50;

/// Ectopic successive-difference threshold (20% of preceding
/// interval). Matches the existing sleep-HRV cleanup.
pub const ECTOPIC_DIFF_RATIO: f64 = 0.20;

/// Reject window if more than this fraction of RRs flag as ectopic.
pub const ECTOPIC_REJECTION: f64 = 0.30;

/// Reject window if mean HR > this and context isn't 'active' — bad
/// signal in quiet contexts.
pub const MAX_HR_RESTING_MIXED: f64 = 100.0;

/// Resting context: HR within +10 of user resting + stillness > 0.9.
pub const RESTING_HR_OFFSET: f64 = 10.0;
/// Active context: HR > +30 over resting OR stillness < 0.5.
pub const ACTIVE_HR_OFFSET: f64 = 30.0;
pub const ACTIVE_STILLNESS_MAX: f64 = 0.5;
pub const RESTING_STILLNESS_MIN: f64 = 0.9;

/// Physiological RR bounds — match the sleep staging feature bounds.
const RR_MIN_MS: u16 = 300;
const RR_MAX_MS: u16 = 2000;

/// Stillness threshold (matches activity.rs gravity-delta threshold).
const STILLNESS_DELTA_G: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrvContext {
    Resting,
    Active,
    Mixed,
}

impl HrvContext {
    pub fn as_str(self) -> &'static str {
        match self {
            HrvContext::Resting => "resting",
            HrvContext::Active => "active",
            HrvContext::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HrvSample {
    pub window_start: NaiveDateTime,
    pub window_end: NaiveDateTime,
    pub rmssd: f64,
    pub sdnn: Option<f64>,
    pub mean_hr: f64,
    pub rr_count: usize,
    pub stillness_ratio: f64,
    pub context: HrvContext,
}

/// Compute HRV samples for every 5-minute aligned window fully inside
/// `[window_start, window_end]`, skipping windows that overlap any
/// of `sleep_windows`.
///
/// `resting_hr` — user's resting HR proxy; PRD says "default to 60".
///
/// Returns one sample per window that passes gating; windows that
/// fail quality gates produce no sample.
pub fn compute_daytime_hrv(
    window_start: NaiveDateTime,
    window_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
    sleep_windows: &[(NaiveDateTime, NaiveDateTime)],
    resting_hr: f64,
) -> Vec<HrvSample> {
    let mut out = Vec::new();
    let mut t = align_down_5min(window_start);
    let step = Duration::minutes(WINDOW_MINUTES);
    while t + step <= window_end {
        let w_start = t;
        let w_end = t + step;
        t = w_end;

        if overlaps_any(w_start, w_end, sleep_windows) {
            continue;
        }
        if let Some(sample) = compute_one_window(w_start, w_end, readings, resting_hr) {
            out.push(sample);
        }
    }
    out
}

fn compute_one_window(
    w_start: NaiveDateTime,
    w_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
    resting_hr: f64,
) -> Option<HrvSample> {
    let in_window: Vec<&ParsedHistoryReading> = readings
        .iter()
        .filter(|r| r.time >= w_start && r.time < w_end)
        .collect();

    let coverage = coverage_secs(&in_window);
    if coverage < MIN_COVERAGE_SECS {
        return None;
    }

    let raw_rr: Vec<u16> = in_window.iter().flat_map(|r| r.rr.iter().copied()).collect();
    if raw_rr.len() < MIN_RR_COUNT {
        return None;
    }

    let (clean_rr, ectopic_ratio) = clean_rr_with_ectopic_ratio(&raw_rr);
    if ectopic_ratio > ECTOPIC_REJECTION {
        return None;
    }
    if clean_rr.len() < MIN_RR_COUNT {
        return None;
    }

    let rmssd = rmssd_ms(&clean_rr);
    let sdnn = sdnn_ms(&clean_rr);
    let mean_hr = mean_hr_bpm(&in_window);
    let stillness = stillness_ratio(&in_window);
    let context = classify_context(mean_hr, stillness, resting_hr);

    if mean_hr > MAX_HR_RESTING_MIXED && context != HrvContext::Active {
        return None;
    }

    Some(HrvSample {
        window_start: w_start,
        window_end: w_end,
        rmssd,
        sdnn: Some(sdnn),
        mean_hr,
        rr_count: clean_rr.len(),
        stillness_ratio: stillness,
        context,
    })
}

fn align_down_5min(t: NaiveDateTime) -> NaiveDateTime {
    let m = t.minute();
    let aligned_m = (m / 5) * 5;
    t.with_minute(aligned_m)
        .and_then(|t| t.with_second(0))
        .unwrap_or(t)
}

fn overlaps_any(
    start: NaiveDateTime,
    end: NaiveDateTime,
    windows: &[(NaiveDateTime, NaiveDateTime)],
) -> bool {
    windows.iter().any(|(s, e)| *s < end && *e > start)
}

fn coverage_secs(readings: &[&ParsedHistoryReading]) -> i64 {
    match (readings.first(), readings.last()) {
        (Some(f), Some(l)) => (l.time - f.time).num_seconds().max(0) + 1,
        _ => 0,
    }
}

fn clean_rr_with_ectopic_ratio(rr: &[u16]) -> (Vec<f64>, f64) {
    let phys: Vec<f64> = rr
        .iter()
        .copied()
        .filter(|&v| (RR_MIN_MS..=RR_MAX_MS).contains(&v))
        .map(f64::from)
        .collect();
    if phys.is_empty() {
        return (Vec::new(), 1.0);
    }
    let mut clean = Vec::with_capacity(phys.len());
    let mut ectopic = 0usize;
    clean.push(phys[0]);
    for i in 1..phys.len() {
        let prev = phys[i - 1];
        if (phys[i] - prev).abs() > ECTOPIC_DIFF_RATIO * prev {
            ectopic += 1;
        } else {
            clean.push(phys[i]);
        }
    }
    (clean, ectopic as f64 / phys.len() as f64)
}

fn rmssd_ms(rr: &[f64]) -> f64 {
    if rr.len() < 2 {
        return 0.0;
    }
    let diffs: Vec<f64> = rr.windows(2).map(|w| (w[1] - w[0]).powi(2)).collect();
    (diffs.iter().sum::<f64>() / diffs.len() as f64).sqrt()
}

fn sdnn_ms(rr: &[f64]) -> f64 {
    if rr.len() < 2 {
        return 0.0;
    }
    let mean = rr.iter().sum::<f64>() / rr.len() as f64;
    let var = rr.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rr.len() as f64;
    var.sqrt()
}

fn mean_hr_bpm(readings: &[&ParsedHistoryReading]) -> f64 {
    let bpms: Vec<f64> = readings
        .iter()
        .map(|r| f64::from(r.bpm))
        .filter(|&b| b > 0.0)
        .collect();
    if bpms.is_empty() {
        0.0
    } else {
        bpms.iter().sum::<f64>() / bpms.len() as f64
    }
}

fn stillness_ratio(readings: &[&ParsedHistoryReading]) -> f64 {
    let gravities: Vec<[f32; 3]> = readings.iter().filter_map(|r| r.gravity).collect();
    if gravities.len() < 2 {
        return 0.0;
    }
    let pairs = gravities.len() - 1;
    let still: usize = gravities
        .windows(2)
        .filter(|w| grav_delta(w[0], w[1]) < STILLNESS_DELTA_G)
        .count();
    still as f64 / pairs as f64
}

fn grav_delta(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn classify_context(mean_hr: f64, stillness: f64, resting_hr: f64) -> HrvContext {
    if mean_hr < resting_hr + RESTING_HR_OFFSET && stillness > RESTING_STILLNESS_MIN {
        HrvContext::Resting
    } else if mean_hr > resting_hr + ACTIVE_HR_OFFSET || stillness < ACTIVE_STILLNESS_MAX {
        HrvContext::Active
    } else {
        HrvContext::Mixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(mins: i64, secs: i64) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 17)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            + Duration::minutes(mins)
            + Duration::seconds(secs)
    }

    fn reading(
        time: NaiveDateTime,
        bpm: u8,
        rr: Vec<u16>,
        gravity: Option<[f32; 3]>,
    ) -> ParsedHistoryReading {
        ParsedHistoryReading {
            time,
            bpm,
            rr,
            imu_data: None,
            gravity,
        }
    }

    #[test]
    fn align_down_5min_rounds_correctly() {
        assert_eq!(align_down_5min(dt(0, 0)), dt(0, 0));
        assert_eq!(align_down_5min(dt(3, 45)), dt(0, 0));
        assert_eq!(align_down_5min(dt(5, 0)), dt(5, 0));
        assert_eq!(align_down_5min(dt(7, 30)), dt(5, 0));
    }

    #[test]
    fn constant_rr_window_produces_rmssd_zero() {
        // HR 62, resting 60, stillness pegged — classifies as Resting
        // (62 - 60 = 2 < RESTING_HR_OFFSET).
        let gravity = Some([0.0_f32, 0.0, 1.0]);
        let readings: Vec<ParsedHistoryReading> = (0..300)
            .map(|i| reading(dt(0, i), 62, vec![968], gravity))
            .collect();
        let samples = compute_daytime_hrv(dt(0, 0), dt(5, 0), &readings, &[], 60.0);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].rmssd, 0.0);
        assert_eq!(samples[0].context, HrvContext::Resting);
    }

    #[test]
    fn alternating_rr_produces_known_rmssd() {
        // 800/900 alternating ⇒ successive diff = 100 ⇒ RMSSD = 100
        let gravity = Some([0.0_f32, 0.0, 1.0]);
        let readings: Vec<ParsedHistoryReading> = (0..300)
            .map(|i| {
                let rr = if i % 2 == 0 { 800 } else { 900 };
                reading(dt(0, i), 70, vec![rr], gravity)
            })
            .collect();
        let samples = compute_daytime_hrv(dt(0, 0), dt(5, 0), &readings, &[], 60.0);
        assert_eq!(samples.len(), 1);
        assert!((samples[0].rmssd - 100.0).abs() < 0.1);
    }

    #[test]
    fn window_overlapping_sleep_is_skipped() {
        let gravity = Some([0.0_f32, 0.0, 1.0]);
        let readings: Vec<ParsedHistoryReading> = (0..600)
            .map(|i| reading(dt(0, i), 60, vec![1000], gravity))
            .collect();
        let sleep = vec![(dt(2, 0), dt(7, 0))];
        let samples = compute_daytime_hrv(dt(0, 0), dt(10, 0), &readings, &sleep, 60.0);
        // Windows 0-5 and 5-10 both overlap sleep [2,7] → zero samples
        assert_eq!(samples.len(), 0);
    }

    #[test]
    fn window_with_insufficient_coverage_is_skipped() {
        // Only 60s of data in a 5-min window
        let gravity = Some([0.0_f32, 0.0, 1.0]);
        let readings: Vec<ParsedHistoryReading> = (0..60)
            .map(|i| reading(dt(0, i), 70, vec![800], gravity))
            .collect();
        let samples = compute_daytime_hrv(dt(0, 0), dt(5, 0), &readings, &[], 60.0);
        assert_eq!(samples.len(), 0);
    }

    #[test]
    fn window_with_insufficient_rrs_is_skipped() {
        // 3 min of data, 1 RR each ⇒ 180 RRs. Below not skipped; try with empty rr.
        let gravity = Some([0.0_f32, 0.0, 1.0]);
        let readings: Vec<ParsedHistoryReading> = (0..300)
            .map(|i| reading(dt(0, i), 70, vec![], gravity))
            .collect();
        let samples = compute_daytime_hrv(dt(0, 0), dt(5, 0), &readings, &[], 60.0);
        assert!(samples.is_empty());
    }

    #[test]
    fn active_window_classified_active() {
        let readings: Vec<ParsedHistoryReading> = (0..300)
            .map(|i| reading(dt(0, i), 140, vec![450], None))
            .collect();
        let samples = compute_daytime_hrv(dt(0, 0), dt(5, 0), &readings, &[], 60.0);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].context, HrvContext::Active);
    }
}
