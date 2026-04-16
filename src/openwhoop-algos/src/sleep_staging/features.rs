//! Per-epoch feature extraction for sleep staging.
//!
//! Produces a [`EpochFeatures`] record for every 30-second epoch in a sleep
//! window. Names follow NeuroKit2 / MESA conventions so the same feature
//! vector can feed a Phase-2 LightGBM model trained on MESA data without
//! a remapping layer.
//!
//! Frequency-domain HRV and respiratory rate are computed over wider
//! **context windows** centered on each epoch (5 min and 2 min
//! respectively). A 30-second epoch alone gives ~0.033 Hz frequency
//! resolution — too coarse to distinguish VLF from LF. The centered-
//! window approach is standard in HRV literature (Shaffer & Ginsberg
//! 2017) and is what MESA preprocessing uses.

use std::f64::consts::PI;

use chrono::{NaiveDateTime, TimeDelta};
use openwhoop_codec::{ImuSample, ParsedHistoryReading};
use rustfft::{FftPlanner, num_complex::Complex};

use super::constants::{
    ECTOPIC_DIFF_RATIO, ECTOPIC_REJECTION_THRESHOLD, EPOCH_SECONDS, FREQ_CONTEXT_WINDOW_SECS,
    HF_HIGH_HZ, HF_LOW_HZ, INTERP_FS_HZ, LF_HIGH_HZ, LF_LOW_HZ, MIN_EPOCH_COVERAGE_SECS,
    MIN_RR_PER_EPOCH, PNN_THRESHOLD_MS, RESP_HIGH_HZ, RESP_LOW_HZ, RESP_MAX_HR, RESP_WINDOW_SECS,
    RR_MAX_MS, RR_MIN_MS, SAMPEN_M, SAMPEN_R_FRAC, STILLNESS_DELTA_G, VLF_HIGH_HZ, VLF_LOW_HZ,
};

/// All feature values for one 30-second epoch. `None` on a feature means
/// the epoch did not have enough data to compute it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EpochFeatures {
    pub epoch_start: Option<NaiveDateTime>,
    pub epoch_end: Option<NaiveDateTime>,
    pub coverage_seconds: f64,
    pub rr_count: usize,
    pub ectopic_ratio: f64,
    /// True iff the epoch has enough data for reliable HRV features.
    /// False implies the classifier should treat the epoch as `Unknown`
    /// and fall back to neighbor interpolation post-hoc.
    pub is_valid: bool,

    // Time-domain HR / HRV
    pub hr_mean: Option<f64>,
    pub hr_std: Option<f64>,
    pub hr_min: Option<f64>,
    pub hr_max: Option<f64>,
    pub mean_nni: Option<f64>,
    pub rmssd: Option<f64>,
    pub sdnn: Option<f64>,
    pub pnn50: Option<f64>,
    pub cv_rr: Option<f64>,

    // Frequency-domain HRV (computed over a 5-min centered context window)
    pub vlf_power: Option<f64>,
    pub lf_power: Option<f64>,
    pub hf_power: Option<f64>,
    pub lf_hf_ratio: Option<f64>,
    pub total_power: Option<f64>,
    pub lf_norm: Option<f64>,
    pub hf_norm: Option<f64>,

    // Non-linear HRV
    pub sd1: Option<f64>,
    pub sd2: Option<f64>,
    pub sd1_sd2_ratio: Option<f64>,
    pub sample_entropy: Option<f64>,

    // Motion (None iff the epoch has no IMU or gravity data at all)
    pub motion_activity_count: Option<f64>,
    pub motion_stillness_ratio: Option<f64>,
    pub gyro_magnitude_mean: Option<f64>,
    pub gyro_magnitude_max: Option<f64>,
    pub posture_change: Option<f64>,

    // Respiratory (computed over a 2-min centered window)
    pub resp_rate: Option<f64>,
    pub resp_rate_std: Option<f64>,
    pub resp_amplitude: Option<f64>,

    // Circadian / temporal context
    pub minutes_since_sleep_onset: f64,
    pub relative_night_position: f64,
    pub hour_of_night: f64,
    pub is_first_half: bool,
}

/// Partition the sleep window into 30-second epochs and compute features
/// for each. `readings` is expected to be sorted ascending by `time`.
pub fn build_epochs(
    sleep_start: NaiveDateTime,
    sleep_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
) -> Vec<EpochFeatures> {
    let total_seconds = (sleep_end - sleep_start).num_seconds().max(0);
    if total_seconds < EPOCH_SECONDS {
        return Vec::new();
    }
    let epoch_count = (total_seconds / EPOCH_SECONDS) as usize;
    let total_duration_f = total_seconds as f64;

    let mut out = Vec::with_capacity(epoch_count);
    for i in 0..epoch_count {
        let epoch_start = sleep_start + TimeDelta::seconds(i as i64 * EPOCH_SECONDS);
        let epoch_end = epoch_start + TimeDelta::seconds(EPOCH_SECONDS);

        let ctx = TemporalContext::new(sleep_start, epoch_start, total_duration_f);
        let features = extract_epoch_features(epoch_start, epoch_end, readings, ctx);
        out.push(features);
    }
    out
}

struct TemporalContext {
    minutes_since_sleep_onset: f64,
    relative_night_position: f64,
    hour_of_night: f64,
    is_first_half: bool,
}

impl TemporalContext {
    fn new(sleep_start: NaiveDateTime, epoch_start: NaiveDateTime, total_secs: f64) -> Self {
        let elapsed_s = (epoch_start - sleep_start).num_seconds() as f64;
        let relative = if total_secs > 0.0 {
            (elapsed_s / total_secs).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let t = epoch_start.time();
        let hour = t.hour() as f64
            + t.minute() as f64 / 60.0
            + t.second() as f64 / 3600.0;
        Self {
            minutes_since_sleep_onset: elapsed_s / 60.0,
            relative_night_position: relative,
            hour_of_night: hour,
            is_first_half: relative < 0.5,
        }
    }
}

fn extract_epoch_features(
    epoch_start: NaiveDateTime,
    epoch_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
    ctx: TemporalContext,
) -> EpochFeatures {
    let mut f = EpochFeatures {
        epoch_start: Some(epoch_start),
        epoch_end: Some(epoch_end),
        minutes_since_sleep_onset: ctx.minutes_since_sleep_onset,
        relative_night_position: ctx.relative_night_position,
        hour_of_night: ctx.hour_of_night,
        is_first_half: ctx.is_first_half,
        ..Default::default()
    };

    let in_epoch: Vec<&ParsedHistoryReading> = readings
        .iter()
        .filter(|r| r.time >= epoch_start && r.time < epoch_end)
        .collect();

    // Coverage: span of reading timestamps inside the epoch. If readings
    // are roughly 1 Hz, this approximates seconds of signal present.
    f.coverage_seconds = coverage_seconds(&in_epoch);

    // Motion features come first — they don't depend on HRV validity.
    apply_motion_features(&in_epoch, &mut f);

    // RR intervals with ectopic accounting.
    let raw_rr: Vec<u16> = in_epoch.iter().flat_map(|r| r.rr.iter().copied()).collect();
    let (clean_rr_ms, ectopic_ratio) = clean_rr_with_ectopic_ratio(&raw_rr);
    f.rr_count = raw_rr.iter().filter(|&&v| in_phys_range(v)).count();
    f.ectopic_ratio = ectopic_ratio;

    let hr_ok = f.coverage_seconds >= MIN_EPOCH_COVERAGE_SECS
        && clean_rr_ms.len() >= MIN_RR_PER_EPOCH
        && ectopic_ratio <= ECTOPIC_REJECTION_THRESHOLD;

    if hr_ok {
        f.is_valid = true;
        apply_time_domain(&in_epoch, &clean_rr_ms, &mut f);
        apply_non_linear(&clean_rr_ms, &mut f);
        apply_frequency_domain(epoch_start, epoch_end, readings, &mut f);
        apply_respiratory(epoch_start, epoch_end, readings, &mut f);
    }

    f
}

fn coverage_seconds(readings: &[&ParsedHistoryReading]) -> f64 {
    match (readings.first(), readings.last()) {
        (Some(first), Some(last)) => {
            // Add 1s for the last sample's own duration (readings are 1 Hz).
            ((last.time - first.time).num_seconds() as f64 + 1.0).max(0.0)
        }
        _ => 0.0,
    }
}

fn in_phys_range(rr: u16) -> bool {
    (RR_MIN_MS..=RR_MAX_MS).contains(&rr)
}

/// Filter RR intervals: drop values outside [RR_MIN_MS, RR_MAX_MS], then
/// flag successive differences exceeding ECTOPIC_DIFF_RATIO as ectopic
/// beats. Ectopics are excluded from HRV computation but counted in the
/// ratio for quality gating.
///
/// Returns `(clean_rr_ms, ectopic_ratio)` where ectopic_ratio =
/// ectopics / phys_valid_count. When phys_valid_count is zero the ratio
/// is 1.0 (completely unusable).
fn clean_rr_with_ectopic_ratio(rr: &[u16]) -> (Vec<f64>, f64) {
    let phys: Vec<f64> = rr
        .iter()
        .copied()
        .filter(|&v| in_phys_range(v))
        .map(f64::from)
        .collect();
    if phys.is_empty() {
        return (Vec::new(), 1.0);
    }

    let mut clean = Vec::with_capacity(phys.len());
    let mut ectopic_count = 0usize;
    // First RR has no predecessor; keep it (can't classify as ectopic
    // without a reference).
    clean.push(phys[0]);
    for i in 1..phys.len() {
        let prev = phys[i - 1];
        let diff = (phys[i] - prev).abs();
        if diff > ECTOPIC_DIFF_RATIO * prev {
            ectopic_count += 1;
        } else {
            clean.push(phys[i]);
        }
    }
    let ratio = ectopic_count as f64 / phys.len() as f64;
    (clean, ratio)
}

fn apply_time_domain(
    readings: &[&ParsedHistoryReading],
    rr_ms: &[f64],
    f: &mut EpochFeatures,
) {
    // HR features from the firmware-reported BPM; matches how existing
    // `sleep.rs` treats it. Filter out zero BPM (dropouts).
    let bpms: Vec<f64> = readings
        .iter()
        .map(|r| f64::from(r.bpm))
        .filter(|&b| b > 0.0)
        .collect();
    if !bpms.is_empty() {
        f.hr_mean = Some(mean(&bpms));
        f.hr_std = Some(stddev(&bpms));
        f.hr_min = bpms.iter().copied().fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a| a.min(v)))
        });
        f.hr_max = bpms.iter().copied().fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |a| a.max(v)))
        });
    }

    // NeuroKit2 naming: `mean_nni`, `sdnn`, `rmssd`, `pnn50`.
    f.mean_nni = Some(mean(rr_ms));
    f.sdnn = Some(stddev(rr_ms));

    let diffs: Vec<f64> = rr_ms.windows(2).map(|w| w[1] - w[0]).collect();
    if !diffs.is_empty() {
        let rmssd = (diffs.iter().map(|d| d * d).sum::<f64>() / diffs.len() as f64).sqrt();
        f.rmssd = Some(rmssd);
        let above = diffs.iter().filter(|d| d.abs() > PNN_THRESHOLD_MS).count();
        f.pnn50 = Some(100.0 * above as f64 / diffs.len() as f64);
    }

    if let (Some(m), Some(s)) = (f.mean_nni, f.sdnn)
        && m > 0.0
    {
        f.cv_rr = Some(s / m);
    }
}

fn apply_non_linear(rr_ms: &[f64], f: &mut EpochFeatures) {
    if rr_ms.len() < 2 {
        return;
    }
    let sdnn = f.sdnn.unwrap_or_else(|| stddev(rr_ms));

    // Poincaré SD1 / SD2. SD1 = RMSSD/√2, SD2 = sqrt(2·SDNN² - SD1²)
    // (Brennan, Palaniswami, Kamen 2001). Computed directly from the
    // RR series rather than from RMSSD to be robust to rounding.
    let diffs: Vec<f64> = rr_ms.windows(2).map(|w| w[1] - w[0]).collect();
    let sd1 = (variance(&diffs) / 2.0).sqrt();
    let sd2_sq = 2.0 * sdnn * sdnn - sd1 * sd1;
    let sd2 = if sd2_sq > 0.0 { sd2_sq.sqrt() } else { 0.0 };
    f.sd1 = Some(sd1);
    f.sd2 = Some(sd2);
    if sd2 > 0.0 {
        f.sd1_sd2_ratio = Some(sd1 / sd2);
    }

    // Sample entropy (m=2, r=0.2·SDNN), Richman & Moorman 2000.
    if sdnn > 0.0 && rr_ms.len() > SAMPEN_M + 1 {
        f.sample_entropy = sample_entropy(rr_ms, SAMPEN_M, SAMPEN_R_FRAC * sdnn);
    }
}

fn apply_frequency_domain(
    epoch_start: NaiveDateTime,
    epoch_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
    f: &mut EpochFeatures,
) {
    let center = epoch_start + (epoch_end - epoch_start) / 2;
    let half = TimeDelta::milliseconds((FREQ_CONTEXT_WINDOW_SECS * 1000.0 / 2.0) as i64);
    let win_start = center - half;
    let win_end = center + half;

    let rr: Vec<u16> = readings
        .iter()
        .filter(|r| r.time >= win_start && r.time < win_end)
        .flat_map(|r| r.rr.iter().copied())
        .collect();
    let (clean, _) = clean_rr_with_ectopic_ratio(&rr);
    // Require at least 2 minutes of RR data for VLF resolution; below
    // that the low-frequency bins alias into DC.
    let min_rr = (2.0 * 60.0) as usize; // ~120 beats minimum
    if clean.len() < min_rr {
        return;
    }

    let interp = interpolate_rr_4hz(&clean);
    if interp.len() < 16 {
        return;
    }

    let spectrum = power_spectrum(&interp, INTERP_FS_HZ);
    let df = INTERP_FS_HZ / interp.len() as f64;

    let vlf = band_power(&spectrum, df, VLF_LOW_HZ, VLF_HIGH_HZ);
    let lf = band_power(&spectrum, df, LF_LOW_HZ, LF_HIGH_HZ);
    let hf = band_power(&spectrum, df, HF_LOW_HZ, HF_HIGH_HZ);
    let total = vlf + lf + hf;

    f.vlf_power = Some(vlf);
    f.lf_power = Some(lf);
    f.hf_power = Some(hf);
    f.total_power = Some(total);
    if hf > 0.0 {
        f.lf_hf_ratio = Some(lf / hf);
    }
    let lf_hf = lf + hf;
    if lf_hf > 0.0 {
        f.lf_norm = Some(lf / lf_hf);
        f.hf_norm = Some(hf / lf_hf);
    }
}

fn apply_respiratory(
    epoch_start: NaiveDateTime,
    epoch_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
    f: &mut EpochFeatures,
) {
    // Suppress when HR is too high — respiratory sinus arrhythmia
    // amplitude collapses above ~100 BPM (Pinheiro 2016).
    if let Some(hr) = f.hr_mean
        && hr > RESP_MAX_HR
    {
        return;
    }

    let center = epoch_start + (epoch_end - epoch_start) / 2;
    let half = TimeDelta::milliseconds((RESP_WINDOW_SECS * 1000.0 / 2.0) as i64);
    let win_start = center - half;
    let win_end = center + half;

    let rr: Vec<u16> = readings
        .iter()
        .filter(|r| r.time >= win_start && r.time < win_end)
        .flat_map(|r| r.rr.iter().copied())
        .collect();
    let (clean, _) = clean_rr_with_ectopic_ratio(&rr);
    if clean.len() < 30 {
        return;
    }

    let interp = interpolate_rr_4hz(&clean);
    if interp.len() < 16 {
        return;
    }

    let (peak_freq, peak_power) = dominant_frequency(&interp, INTERP_FS_HZ, RESP_LOW_HZ, RESP_HIGH_HZ);
    if let Some(freq) = peak_freq {
        f.resp_rate = Some(freq * 60.0);
        f.resp_amplitude = Some(peak_power);

        // Split window into thirds; estimate dominant frequency in each
        // sub-window and take stdev as within-epoch respiratory
        // variability. Three sub-windows keeps each long enough to
        // resolve 0.1 Hz (40 s × 4 Hz = 160 samples → ~0.025 Hz bin).
        let sub_len = interp.len() / 3;
        if sub_len >= 16 {
            let mut rates: Vec<f64> = Vec::with_capacity(3);
            for k in 0..3 {
                let slice = &interp[k * sub_len..(k + 1) * sub_len];
                let (sub_peak, _) =
                    dominant_frequency(slice, INTERP_FS_HZ, RESP_LOW_HZ, RESP_HIGH_HZ);
                if let Some(p) = sub_peak {
                    rates.push(p * 60.0);
                }
            }
            if rates.len() >= 2 {
                f.resp_rate_std = Some(stddev(&rates));
            }
        }
    }
}

fn apply_motion_features(readings: &[&ParsedHistoryReading], f: &mut EpochFeatures) {
    // Gravity-based stillness matches the existing sleep detector in
    // activity.rs: a Δgravity magnitude < 0.01 g per 1 Hz sample is
    // treated as "still."
    let gravities: Vec<[f32; 3]> = readings.iter().filter_map(|r| r.gravity).collect();
    if gravities.len() >= 2 {
        let total_pairs = gravities.len() - 1;
        let still_count = gravities
            .windows(2)
            .filter(|w| grav_delta(w[0], w[1]) < STILLNESS_DELTA_G)
            .count();
        f.motion_stillness_ratio = Some(still_count as f64 / total_pairs as f64);
    }
    if gravities.len() >= 2 {
        let first = gravities[0];
        let last = gravities[gravities.len() - 1];
        f.posture_change = Some(angle_degrees(first, last));
    }

    // Activity count (actigraphy proxy): sum of |accel_magnitude - 1.0|
    // across IMU samples in the epoch. One-g is subtracted so constant
    // gravity does not register as activity (actigraphy convention,
    // pyActigraphy/Cole-Kripke formulation).
    let imu_samples: Vec<&ImuSample> = readings
        .iter()
        .flat_map(|r| r.imu_data.iter().flat_map(|v| v.iter()))
        .collect();
    if !imu_samples.is_empty() {
        let activity: f64 = imu_samples
            .iter()
            .map(|s| {
                let mag = (s.acc_x_g * s.acc_x_g + s.acc_y_g * s.acc_y_g + s.acc_z_g * s.acc_z_g)
                    .sqrt();
                ((mag - 1.0_f32).abs()) as f64
            })
            .sum();
        f.motion_activity_count = Some(activity);

        let gyro_mags: Vec<f64> = imu_samples
            .iter()
            .map(|s| {
                let g2 = s.gyr_x_dps * s.gyr_x_dps
                    + s.gyr_y_dps * s.gyr_y_dps
                    + s.gyr_z_dps * s.gyr_z_dps;
                (g2 as f64).sqrt()
            })
            .collect();
        f.gyro_magnitude_mean = Some(mean(&gyro_mags));
        f.gyro_magnitude_max = gyro_mags
            .iter()
            .copied()
            .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))));
    }
}

fn grav_delta(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn angle_degrees(a: [f32; 3], b: [f32; 3]) -> f64 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) as f64;
    let na = ((a[0] * a[0] + a[1] * a[1] + a[2] * a[2]) as f64).sqrt();
    let nb = ((b[0] * b[0] + b[1] * b[1] + b[2] * b[2]) as f64).sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    let cos = (dot / (na * nb)).clamp(-1.0, 1.0);
    cos.acos().to_degrees()
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn variance(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64
}

fn stddev(xs: &[f64]) -> f64 {
    variance(xs).sqrt()
}

/// Linear interpolation of an RR tachogram onto a uniform INTERP_FS_HZ
/// (4 Hz) grid. The tachogram's time axis is the cumulative sum of RR
/// intervals — the classical HRV formulation (Camm et al. 1996).
fn interpolate_rr_4hz(rr_ms: &[f64]) -> Vec<f64> {
    if rr_ms.len() < 2 {
        return Vec::new();
    }
    // Cumulative time in seconds for each beat. t[0] = 0.
    let mut t = Vec::with_capacity(rr_ms.len());
    let mut acc = 0.0;
    for (i, &r) in rr_ms.iter().enumerate() {
        if i == 0 {
            t.push(0.0);
        } else {
            acc += r / 1000.0;
            t.push(acc);
        }
    }
    let total = *t.last().unwrap();
    let step = 1.0 / INTERP_FS_HZ;
    let n = (total / step).floor() as usize;
    if n < 2 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let tu = i as f64 * step;
        while j + 1 < t.len() && t[j + 1] < tu {
            j += 1;
        }
        if j + 1 >= t.len() {
            out.push(rr_ms[rr_ms.len() - 1]);
            continue;
        }
        let t0 = t[j];
        let t1 = t[j + 1];
        let y0 = rr_ms[j];
        let y1 = rr_ms[j + 1];
        let v = if t1 > t0 {
            y0 + (y1 - y0) * (tu - t0) / (t1 - t0)
        } else {
            y0
        };
        out.push(v);
    }
    out
}

/// Single-sided power spectrum of a uniformly-sampled series. Returns
/// the magnitude-squared / N at each frequency bin (0 → fs/2). Applies
/// a Hann window and detrending; standard HRV practice.
fn power_spectrum(samples: &[f64], _fs: f64) -> Vec<f64> {
    let n = samples.len();
    let m = mean(samples);
    // Hann window; compensate for window energy loss by 1/sum(w²).
    let mut windowed: Vec<Complex<f64>> = Vec::with_capacity(n);
    let mut w_sq_sum = 0.0;
    for (i, &s) in samples.iter().enumerate() {
        let w = 0.5 - 0.5 * ((2.0 * PI * i as f64) / (n as f64 - 1.0)).cos();
        w_sq_sum += w * w;
        windowed.push(Complex {
            re: (s - m) * w,
            im: 0.0,
        });
    }
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    fft.process(&mut windowed);

    // Return single-sided PSD from bin 0 to n/2.
    let half = n / 2 + 1;
    let norm = if w_sq_sum > 0.0 { w_sq_sum } else { 1.0 };
    windowed
        .iter()
        .take(half)
        .enumerate()
        .map(|(i, c)| {
            let mag2 = c.re * c.re + c.im * c.im;
            let scale = if i == 0 || i == n / 2 { 1.0 } else { 2.0 };
            scale * mag2 / norm
        })
        .collect()
}

fn band_power(psd: &[f64], df: f64, low: f64, high: f64) -> f64 {
    let mut total = 0.0;
    for (i, &p) in psd.iter().enumerate() {
        let f = i as f64 * df;
        if f >= low && f < high {
            total += p * df;
        }
    }
    total
}

fn dominant_frequency(samples: &[f64], fs: f64, low: f64, high: f64) -> (Option<f64>, f64) {
    let psd = power_spectrum(samples, fs);
    let df = fs / samples.len() as f64;
    let mut best_idx: Option<usize> = None;
    let mut best_power = 0.0;
    for (i, &p) in psd.iter().enumerate() {
        let f = i as f64 * df;
        if f >= low && f < high && p > best_power {
            best_power = p;
            best_idx = Some(i);
        }
    }
    match best_idx {
        Some(i) => (Some(i as f64 * df), best_power),
        None => (None, 0.0),
    }
}

/// Sample entropy (Richman & Moorman 2000). For a series of length N,
/// count the number of m- and (m+1)-length template matches within
/// tolerance r, and take -ln(A/B). Used for HRV complexity. O(N²) but
/// N<50 per epoch so well under the perf budget.
fn sample_entropy(xs: &[f64], m: usize, r: f64) -> Option<f64> {
    let n = xs.len();
    if n <= m + 1 || r <= 0.0 {
        return None;
    }
    let count_matches = |len: usize| -> u64 {
        let mut c = 0u64;
        for i in 0..=(n - len - 1) {
            for j in (i + 1)..=(n - len) {
                let mut max_d = 0.0_f64;
                for k in 0..len {
                    let d = (xs[i + k] - xs[j + k]).abs();
                    if d > max_d {
                        max_d = d;
                    }
                }
                if max_d < r {
                    c += 1;
                }
            }
        }
        c
    };
    let b = count_matches(m);
    let a = count_matches(m + 1);
    if a == 0 || b == 0 {
        return None;
    }
    Some(-((a as f64) / (b as f64)).ln())
}

trait NaiveTimeExt {
    fn hour(&self) -> u32;
    fn minute(&self) -> u32;
    fn second(&self) -> u32;
}
impl NaiveTimeExt for chrono::NaiveTime {
    fn hour(&self) -> u32 {
        chrono::Timelike::hour(self)
    }
    fn minute(&self) -> u32 {
        chrono::Timelike::minute(self)
    }
    fn second(&self) -> u32 {
        chrono::Timelike::second(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use openwhoop_codec::ImuSample;

    fn dt(h: u32, m: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 16)
            .unwrap()
            .and_hms_opt(h, m, s)
            .unwrap()
    }

    fn reading(t: NaiveDateTime, bpm: u8, rr: Vec<u16>) -> ParsedHistoryReading {
        ParsedHistoryReading {
            time: t,
            bpm,
            rr,
            imu_data: None,
            gravity: None,
        }
    }

    fn reading_with_gravity(
        t: NaiveDateTime,
        bpm: u8,
        rr: Vec<u16>,
        gravity: [f32; 3],
    ) -> ParsedHistoryReading {
        ParsedHistoryReading {
            time: t,
            bpm,
            rr,
            imu_data: None,
            gravity: Some(gravity),
        }
    }

    // ----- construction / coverage -----

    #[test]
    fn build_epochs_partitions_into_30s_chunks() {
        let start = dt(22, 0, 0);
        let end = dt(22, 2, 0); // 2 min → 4 epochs
        let readings: Vec<_> = (0..120)
            .map(|i| reading(start + TimeDelta::seconds(i), 60, vec![1000]))
            .collect();
        let epochs = build_epochs(start, end, &readings);
        assert_eq!(epochs.len(), 4);
        assert_eq!(epochs[0].epoch_start.unwrap(), start);
        assert_eq!(epochs[1].epoch_start.unwrap(), start + TimeDelta::seconds(30));
    }

    #[test]
    fn build_epochs_empty_when_window_too_short() {
        let start = dt(22, 0, 0);
        let epochs = build_epochs(start, start + TimeDelta::seconds(15), &[]);
        assert!(epochs.is_empty());
    }

    // ----- validity / edge cases -----

    #[test]
    fn epoch_with_no_rr_is_invalid_and_returns_none_hrv() {
        let start = dt(22, 0, 0);
        let readings: Vec<_> = (0..30).map(|i| reading(start + TimeDelta::seconds(i), 60, vec![])).collect();
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        assert_eq!(epochs.len(), 1);
        let e = &epochs[0];
        assert!(!e.is_valid);
        assert!(e.rmssd.is_none());
        assert!(e.sdnn.is_none());
        assert!(e.hf_power.is_none());
    }

    #[test]
    fn epoch_with_insufficient_rr_is_invalid() {
        let start = dt(22, 0, 0);
        // 5 readings with 1 RR each = 5 RRs total, below MIN_RR_PER_EPOCH
        let readings: Vec<_> = (0..5)
            .map(|i| reading(start + TimeDelta::seconds(i), 60, vec![1000]))
            .collect();
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        assert!(!epochs[0].is_valid);
    }

    // ----- RR cleaning -----

    #[test]
    fn clean_rr_drops_out_of_range() {
        let (clean, ratio) = clean_rr_with_ectopic_ratio(&[200, 800, 3000, 900, 1000]);
        assert!(ratio < ECTOPIC_REJECTION_THRESHOLD);
        assert_eq!(clean.len(), 3); // 800, 900, 1000 survive
    }

    #[test]
    fn clean_rr_flags_ectopic() {
        // 800 → 1200 is a 50% jump, should be flagged ectopic
        let (clean, ratio) = clean_rr_with_ectopic_ratio(&[800, 810, 800, 1200, 810, 800]);
        assert!(ratio > 0.0);
        assert!(!clean.contains(&1200.0));
    }

    // ----- time-domain HRV -----

    #[test]
    fn rmssd_alternating_rr_gives_expected_value() {
        // Alternating 800/900: every diff = 100 → RMSSD = 100
        let start = dt(22, 0, 0);
        let rrs: Vec<u16> = (0..60).map(|i| if i % 2 == 0 { 800 } else { 900 }).collect();
        let readings: Vec<_> = rrs
            .chunks(2)
            .enumerate()
            .map(|(i, chunk)| reading(start + TimeDelta::seconds(i as i64), 70, chunk.to_vec()))
            .collect();
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        let rmssd = epochs[0].rmssd.unwrap();
        assert!((rmssd - 100.0).abs() < 0.01, "got rmssd={rmssd}");
    }

    #[test]
    fn constant_rr_gives_zero_variability() {
        let start = dt(22, 0, 0);
        let readings: Vec<_> = (0..30)
            .map(|i| reading(start + TimeDelta::seconds(i), 75, vec![800]))
            .collect();
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        let e = &epochs[0];
        assert_eq!(e.rmssd, Some(0.0));
        assert_eq!(e.sdnn, Some(0.0));
        assert_eq!(e.pnn50, Some(0.0));
    }

    // ----- frequency-domain HRV -----

    #[test]
    fn synthetic_hf_oscillation_lands_in_hf_band() {
        // Build 5 min of RR data with a known 0.25 Hz (15 bpm) sinusoid.
        // That's squarely in the HF band (0.15-0.4 Hz). After FFT the
        // peak should land there, so HF power dominates LF.
        let start = dt(22, 0, 0);
        let mean_rr = 1000.0_f64;
        let amplitude = 50.0_f64;
        let freq_hz = 0.25_f64;
        // Generate ~5 min of RR intervals at mean 1000 ms (= 60 bpm).
        // 300 beats at 1 s each.
        let mut readings = Vec::new();
        let mut t_ms = 0.0_f64;
        let mut current_time = start;
        let mut bucket_rr: Vec<u16> = Vec::new();
        let mut bucket_second = 0_i64;
        for _i in 0..300 {
            let rr = mean_rr + amplitude * (2.0 * PI * freq_hz * t_ms / 1000.0).sin();
            let rr_u = rr.round().clamp(300.0, 2000.0) as u16;
            t_ms += rr;
            let this_second = (t_ms / 1000.0) as i64;
            if this_second != bucket_second && !bucket_rr.is_empty() {
                readings.push(reading(current_time, 60, bucket_rr.clone()));
                bucket_rr.clear();
                current_time = start + TimeDelta::seconds(this_second);
                bucket_second = this_second;
            }
            bucket_rr.push(rr_u);
        }
        if !bucket_rr.is_empty() {
            readings.push(reading(current_time, 60, bucket_rr));
        }

        // Pick the middle epoch so the 5-min context window is fully inside.
        let epochs = build_epochs(start, start + TimeDelta::seconds(300), &readings);
        let mid = &epochs[epochs.len() / 2];
        let lf = mid.lf_power.unwrap_or(0.0);
        let hf = mid.hf_power.unwrap_or(0.0);
        assert!(hf > lf, "HF {hf} should dominate LF {lf} for 0.25 Hz input");
        assert!(hf > 0.0);
    }

    // ----- motion -----

    #[test]
    fn zero_motion_gives_stillness_ratio_one() {
        let start = dt(22, 0, 0);
        let gravity = [0.0_f32, 0.0, 1.0];
        let readings: Vec<_> = (0..30)
            .map(|i| reading_with_gravity(start + TimeDelta::seconds(i), 60, vec![1000], gravity))
            .collect();
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        let e = &epochs[0];
        assert_eq!(e.motion_stillness_ratio, Some(1.0));
        assert_eq!(e.posture_change, Some(0.0));
    }

    #[test]
    fn posture_change_90_degrees() {
        let start = dt(22, 0, 0);
        let mut readings: Vec<_> = Vec::new();
        // First half: supine (gravity = +Z)
        for i in 0..15 {
            readings.push(reading_with_gravity(
                start + TimeDelta::seconds(i),
                60,
                vec![1000],
                [0.0, 0.0, 1.0],
            ));
        }
        // Second half: on side (gravity = +X)
        for i in 15..30 {
            readings.push(reading_with_gravity(
                start + TimeDelta::seconds(i),
                60,
                vec![1000],
                [1.0, 0.0, 0.0],
            ));
        }
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        let angle = epochs[0].posture_change.unwrap();
        assert!((angle - 90.0).abs() < 1e-6, "got angle={angle}");
    }

    #[test]
    fn activity_count_sums_from_imu() {
        let start = dt(22, 0, 0);
        let imu = ImuSample {
            acc_x_g: 0.0,
            acc_y_g: 0.0,
            acc_z_g: 2.0, // magnitude 2 g → |2 - 1| = 1
            gyr_x_dps: 10.0,
            gyr_y_dps: 0.0,
            gyr_z_dps: 0.0,
        };
        let mut readings: Vec<_> = (0..30)
            .map(|i| reading(start + TimeDelta::seconds(i), 60, vec![1000]))
            .collect();
        for r in &mut readings {
            r.imu_data = Some(vec![imu.clone()]);
        }
        let epochs = build_epochs(start, start + TimeDelta::seconds(30), &readings);
        let ac = epochs[0].motion_activity_count.unwrap();
        assert!((ac - 30.0).abs() < 1e-6);
        let gy = epochs[0].gyro_magnitude_mean.unwrap();
        assert!((gy - 10.0).abs() < 1e-6);
    }

    // ----- temporal context -----

    #[test]
    fn temporal_context_places_first_epoch_in_first_half() {
        let start = dt(22, 0, 0);
        let end = start + TimeDelta::hours(8);
        let readings: Vec<_> = (0..30)
            .map(|i| reading(start + TimeDelta::seconds(i), 60, vec![1000]))
            .collect();
        let epochs = build_epochs(start, end, &readings);
        assert!(epochs[0].is_first_half);
        assert!(epochs[0].relative_night_position < 0.01);
        assert_eq!(epochs[0].minutes_since_sleep_onset, 0.0);
    }

    #[test]
    fn temporal_context_last_epoch_in_second_half() {
        let start = dt(22, 0, 0);
        let end = start + TimeDelta::hours(8);
        let readings: Vec<_> = (0..30)
            .map(|i| reading(start + TimeDelta::seconds(i), 60, vec![1000]))
            .collect();
        let epochs = build_epochs(start, end, &readings);
        let last = &epochs[epochs.len() - 1];
        assert!(!last.is_first_half);
        assert!(last.relative_night_position > 0.99);
    }

    // ----- sample entropy -----

    #[test]
    fn sample_entropy_constant_is_near_zero() {
        // A constant series is maximally regular; SampEn should be small
        // (not strictly zero because of the boundary effect: counts of
        // length-m and length-m+1 templates differ by N - 2m + 1 terms).
        let xs = vec![100.0; 20];
        let se = sample_entropy(&xs, 2, 1.0).unwrap();
        assert!(se.abs() < 0.2, "got se={se}");
    }

    #[test]
    fn sample_entropy_returns_none_when_no_matches() {
        // Monotonic ramp wider than r → no m+1 templates match → None.
        let xs: Vec<f64> = (0..20).map(|i| i as f64 * 100.0).collect();
        assert!(sample_entropy(&xs, 2, 1.0).is_none());
    }

    #[test]
    fn sample_entropy_random_gives_positive_value() {
        // A mildly irregular sequence has sample entropy > 0.
        let xs: Vec<f64> = (0..40)
            .map(|i| 800.0 + ((i as f64 * 1.7).sin() * 20.0))
            .collect();
        let se = sample_entropy(&xs, 2, 10.0).unwrap();
        assert!(se > 0.0, "got se={se}");
    }

    // ----- interpolation -----

    #[test]
    fn interpolate_preserves_constant_rr() {
        let rr = vec![800.0; 50];
        let interp = interpolate_rr_4hz(&rr);
        assert!(!interp.is_empty());
        for v in &interp {
            assert!((v - 800.0).abs() < 1e-6);
        }
    }
}
