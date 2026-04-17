//! Rule-based activity classifier (rule-v0). 1-minute windows of IMU
//! data are bucketed into Sedentary / Light / Moderate / Vigorous.
//!
//! Thresholds are conservative starting values from general human-
//! activity-recognition literature. They WILL need tuning on real
//! WHOOP data. Version-tagged `rule-v0` so future tuning is explicit.

use chrono::{Duration, NaiveDateTime};
use openwhoop_codec::{ImuSample, ParsedHistoryReading};
use rustfft::{FftPlanner, num_complex::Complex};

/// Classifier version tag written alongside every row. Bump when
/// thresholds change materially.
pub const ACTIVITY_CLASSIFIER_VERSION: &str = "rule-v0";

pub const WINDOW_MINUTES: i64 = 1;

/// IMU sample rate (Hz). WHOOP IMU runs at ~26 Hz in the packets we
/// see. Used for FFT bin → frequency conversion.
pub const IMU_SAMPLE_RATE_HZ: f64 = 26.0;

/// Frequency band to search for dominant motion frequency.
pub const FFT_LOW_HZ: f64 = 0.5;
pub const FFT_HIGH_HZ: f64 = 10.0;

// Rule thresholds (rule-v0). Document each with its source/rationale
// in DECISIONS.md. Tune after real-user data.
const SED_ACCEL_STD: f64 = 0.05;
const SED_GYRO_MEAN: f64 = 10.0;
const LIGHT_ACCEL_STD: f64 = 0.15;
const LIGHT_FREQ: f64 = 2.0;
const MOD_FREQ_LOW: f64 = 2.0;
const MOD_FREQ_HIGH: f64 = 3.5;
const VIGOROUS_FREQ: f64 = 3.0;
const VIGOROUS_ACCEL_STD: f64 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityClass {
    Sedentary,
    Light,
    Moderate,
    Vigorous,
    Unknown,
}

impl ActivityClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ActivityClass::Sedentary => "sedentary",
            ActivityClass::Light => "light",
            ActivityClass::Moderate => "moderate",
            ActivityClass::Vigorous => "vigorous",
            ActivityClass::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivitySample {
    pub window_start: NaiveDateTime,
    pub window_end: NaiveDateTime,
    pub classification: ActivityClass,
    pub accel_magnitude_mean: f64,
    pub accel_magnitude_std: f64,
    pub gyro_magnitude_mean: f64,
    pub dominant_frequency_hz: f64,
    pub mean_hr: f64,
}

/// Classify activity in 1-minute windows across
/// `[range_start, range_end]`. Skips windows that overlap `exclude_windows`
/// (sleep cycles and/or non-wear periods computed by the caller).
/// Windows without any IMU data are skipped, not stored.
pub fn classify_activities(
    range_start: NaiveDateTime,
    range_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
    exclude_windows: &[(NaiveDateTime, NaiveDateTime)],
) -> Vec<ActivitySample> {
    let mut out = Vec::new();
    let step = Duration::minutes(WINDOW_MINUTES);
    let mut t = range_start;
    while t + step <= range_end {
        let w_start = t;
        let w_end = t + step;
        t = w_end;

        if overlaps_any(w_start, w_end, exclude_windows) {
            continue;
        }
        if let Some(sample) = classify_one(w_start, w_end, readings) {
            out.push(sample);
        }
    }
    out
}

fn classify_one(
    w_start: NaiveDateTime,
    w_end: NaiveDateTime,
    readings: &[ParsedHistoryReading],
) -> Option<ActivitySample> {
    let imu: Vec<&ImuSample> = readings
        .iter()
        .filter(|r| r.time >= w_start && r.time < w_end)
        .flat_map(|r| r.imu_data.iter().flat_map(|v| v.iter()))
        .collect();
    if imu.is_empty() {
        return None;
    }

    let accel_mags: Vec<f64> = imu
        .iter()
        .map(|s| {
            ((s.acc_x_g * s.acc_x_g + s.acc_y_g * s.acc_y_g + s.acc_z_g * s.acc_z_g) as f64).sqrt()
        })
        .collect();
    let accel_mean = mean(&accel_mags);
    let accel_std = stddev(&accel_mags);

    let gyro_mags: Vec<f64> = imu
        .iter()
        .map(|s| {
            ((s.gyr_x_dps * s.gyr_x_dps
                + s.gyr_y_dps * s.gyr_y_dps
                + s.gyr_z_dps * s.gyr_z_dps) as f64)
                .sqrt()
        })
        .collect();
    let gyro_mean = mean(&gyro_mags);

    // FFT of detrended accel magnitude series. Dominant frequency in
    // the 0.5-10 Hz band.
    let dominant_hz = dominant_freq(&accel_mags, IMU_SAMPLE_RATE_HZ, FFT_LOW_HZ, FFT_HIGH_HZ);

    let bpms: Vec<f64> = readings
        .iter()
        .filter(|r| r.time >= w_start && r.time < w_end)
        .map(|r| f64::from(r.bpm))
        .filter(|&b| b > 0.0)
        .collect();
    let mean_hr = if bpms.is_empty() { 0.0 } else { mean(&bpms) };

    let classification = classify(accel_std, gyro_mean, dominant_hz);

    Some(ActivitySample {
        window_start: w_start,
        window_end: w_end,
        classification,
        accel_magnitude_mean: accel_mean,
        accel_magnitude_std: accel_std,
        gyro_magnitude_mean: gyro_mean,
        dominant_frequency_hz: dominant_hz,
        mean_hr,
    })
}

fn classify(accel_std: f64, gyro_mean: f64, dom_hz: f64) -> ActivityClass {
    if accel_std < SED_ACCEL_STD && gyro_mean < SED_GYRO_MEAN {
        return ActivityClass::Sedentary;
    }
    if dom_hz > VIGOROUS_FREQ || accel_std > VIGOROUS_ACCEL_STD {
        return ActivityClass::Vigorous;
    }
    if (MOD_FREQ_LOW..MOD_FREQ_HIGH).contains(&dom_hz) && accel_std > LIGHT_ACCEL_STD {
        return ActivityClass::Moderate;
    }
    if accel_std < LIGHT_ACCEL_STD && dom_hz < LIGHT_FREQ {
        return ActivityClass::Light;
    }
    ActivityClass::Unknown
}

fn overlaps_any(
    start: NaiveDateTime,
    end: NaiveDateTime,
    windows: &[(NaiveDateTime, NaiveDateTime)],
) -> bool {
    windows.iter().any(|(s, e)| *s < end && *e > start)
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

fn stddev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64).sqrt()
}

fn dominant_freq(samples: &[f64], fs: f64, low: f64, high: f64) -> f64 {
    if samples.len() < 8 {
        return 0.0;
    }
    let n = samples.len();
    let m = mean(samples);
    let mut buf: Vec<Complex<f64>> = samples
        .iter()
        .map(|s| Complex { re: s - m, im: 0.0 })
        .collect();
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    fft.process(&mut buf);

    let df = fs / n as f64;
    let mut best_bin = 0usize;
    let mut best_mag = 0.0;
    for (i, bin) in buf.iter().enumerate().take(n / 2 + 1).skip(1) {
        let f = i as f64 * df;
        if !(low..high).contains(&f) {
            continue;
        }
        let mag2 = bin.re * bin.re + bin.im * bin.im;
        if mag2 > best_mag {
            best_mag = mag2;
            best_bin = i;
        }
    }
    best_bin as f64 * df
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(secs: i64) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 17)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            + Duration::seconds(secs)
    }

    fn still_sample() -> ImuSample {
        ImuSample {
            acc_x_g: 0.0,
            acc_y_g: 0.0,
            acc_z_g: 1.0,
            gyr_x_dps: 0.5,
            gyr_y_dps: 0.5,
            gyr_z_dps: 0.5,
        }
    }

    fn reading_with_imu(
        time: NaiveDateTime,
        bpm: u8,
        samples: Vec<ImuSample>,
    ) -> ParsedHistoryReading {
        ParsedHistoryReading {
            time,
            bpm,
            rr: vec![],
            imu_data: Some(samples),
            gravity: None,
        }
    }

    #[test]
    fn still_imu_classifies_sedentary() {
        let readings: Vec<ParsedHistoryReading> = (0..60)
            .map(|i| {
                reading_with_imu(dt(i), 60, (0..26).map(|_| still_sample()).collect())
            })
            .collect();
        let samples = classify_activities(dt(0), dt(60), &readings, &[]);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].classification, ActivityClass::Sedentary);
    }

    #[test]
    fn high_motion_imu_classifies_vigorous() {
        // Alternate between small-accel and large-accel samples so the
        // *magnitude* varies (signs cancel out via sqrt). Running-like
        // ~2.5 Hz signal at 26 Hz sample rate = 10-11 samples per cycle.
        let readings: Vec<ParsedHistoryReading> = (0..60)
            .map(|i| {
                let samples: Vec<ImuSample> = (0..26)
                    .map(|j| {
                        // 2.5 Hz oscillation in magnitude
                        let phase = (i as f64 * 26.0 + j as f64) / 26.0 * 2.5 * 2.0 * std::f64::consts::PI;
                        let mag = 1.0 + 0.8 * phase.sin();
                        ImuSample {
                            acc_x_g: mag as f32,
                            acc_y_g: 0.0,
                            acc_z_g: 1.0,
                            gyr_x_dps: 100.0,
                            gyr_y_dps: 0.0,
                            gyr_z_dps: 0.0,
                        }
                    })
                    .collect();
                reading_with_imu(dt(i), 160, samples)
            })
            .collect();
        let samples = classify_activities(dt(0), dt(60), &readings, &[]);
        assert_eq!(samples.len(), 1);
        assert!(
            matches!(
                samples[0].classification,
                ActivityClass::Vigorous | ActivityClass::Moderate
            ),
            "got {:?} (accel_std={:.3}, dom_hz={:.2})",
            samples[0].classification,
            samples[0].accel_magnitude_std,
            samples[0].dominant_frequency_hz,
        );
    }

    #[test]
    fn window_without_imu_produces_no_sample() {
        let readings: Vec<ParsedHistoryReading> = (0..60)
            .map(|i| ParsedHistoryReading {
                time: dt(i),
                bpm: 60,
                rr: vec![],
                imu_data: None,
                gravity: None,
            })
            .collect();
        let samples = classify_activities(dt(0), dt(60), &readings, &[]);
        assert!(samples.is_empty());
    }

    #[test]
    fn window_overlapping_exclude_is_skipped() {
        let readings: Vec<ParsedHistoryReading> = (0..120)
            .map(|i| reading_with_imu(dt(i), 60, vec![still_sample()]))
            .collect();
        let exclude = vec![(dt(30), dt(90))];
        let samples = classify_activities(dt(0), dt(120), &readings, &exclude);
        // Both 0-60 and 60-120 overlap [30,90] ⇒ 0 samples.
        assert_eq!(samples.len(), 0);
    }
}
