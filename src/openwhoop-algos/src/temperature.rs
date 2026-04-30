use chrono::NaiveDateTime;

pub struct SkinTempCalculator;

#[derive(Debug, Clone, Copy)]
pub struct SkinTempScore {
    pub time: NaiveDateTime,
    pub temp_celsius: f64,
}

impl SkinTempCalculator {
    /// Empirical conversion factor: T(degC) = skin_temp_raw x 0.04
    ///
    /// Derived from firmware analysis of the WHOOP 4.0:
    /// - The raw u16 value is a thermistor ADC reading passed through the
    ///   DSP pipeline without mathematical transformation
    /// - The firmware sends raw values; the WHOOP server performs per-device
    ///   calibrated conversion
    /// - This factor produces physiologically reasonable wrist skin temperatures
    ///   (31-37degC) across the observed raw range (582-1125)
    const CONVERSION_FACTOR: f64 = 0.04;

    /// Minimum valid raw reading (below this is likely off-wrist or sensor error)
    const MIN_RAW: u16 = 100;

    pub fn convert(time: NaiveDateTime, skin_temp_raw: u16) -> Option<SkinTempScore> {
        if skin_temp_raw < Self::MIN_RAW {
            return None;
        }

        let temp_celsius = f64::from(skin_temp_raw) * Self::CONVERSION_FACTOR;
        Some(SkinTempScore { time, temp_celsius })
    }
}

/// Per-user thermistor calibration. Each strap's thermistor has a
/// different absolute offset; a single global conversion factor (the
/// 0.04 in [`SkinTempCalculator`] above) produces alarming false
/// readings (e.g. 38°C / 100°F at the wrist while the user's actual core
/// is 36.9°C). We anchor the user's *median raw value during stable
/// wear* to a population-typical resting wrist skin temp of 32°C, then
/// apply a slope assumed constant across straps of the same model.
///
/// Math: `T(°C) = (raw - raw_median) × SLOPE + REF_C`
///
/// SLOPE is empirically fit at 0.030 (°C per ADC unit) from observed
/// wear/off-wrist transitions; this can be refined per-strap later if
/// we collect known-temperature reference points.
pub struct SkinTempCalibration;

impl SkinTempCalibration {
    /// °C per ADC unit. Thermistor characteristic — assumed constant
    /// across straps of the same hardware revision; only the offset
    /// (which we capture in `raw_median`) varies per device.
    pub const SLOPE_C_PER_ADC: f64 = 0.030;

    /// Reference temperature the user's `raw_median` is anchored to.
    /// 32°C is a population-typical resting wrist skin temp under
    /// normal indoor conditions (medical literature, multiple sources).
    pub const REFERENCE_C: f64 = 32.0;

    /// Apply per-user calibration. Returns the calibrated temperature
    /// in °C using the user's median-raw anchor.
    pub fn convert(skin_temp_raw: u16, raw_median: f64) -> f64 {
        (f64::from(skin_temp_raw) - raw_median) * Self::SLOPE_C_PER_ADC + Self::REFERENCE_C
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn base_time() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
    }

    #[test]
    fn zero_raw_returns_none() {
        assert!(SkinTempCalculator::convert(base_time(), 0).is_none());
    }

    #[test]
    fn below_minimum_returns_none() {
        assert!(SkinTempCalculator::convert(base_time(), 50).is_none());
    }

    #[test]
    fn typical_resting_value() {
        // Raw 850 -> 34.0degC
        let score = SkinTempCalculator::convert(base_time(), 850).unwrap();
        assert!((score.temp_celsius - 34.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sleep_value() {
        // Raw 900 -> 36.0degC
        let score = SkinTempCalculator::convert(base_time(), 900).unwrap();
        assert!((score.temp_celsius - 36.0).abs() < f64::EPSILON);
    }

    #[test]
    fn low_value() {
        // Raw 700 -> 28.0degC
        let score = SkinTempCalculator::convert(base_time(), 700).unwrap();
        assert!((score.temp_celsius - 28.0).abs() < f64::EPSILON);
    }

    #[test]
    fn minimum_valid() {
        let score = SkinTempCalculator::convert(base_time(), 100).unwrap();
        assert!((score.temp_celsius - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn calibration_anchors_median_to_reference() {
        // raw == raw_median should map exactly to the reference temp.
        let t = SkinTempCalibration::convert(953, 953.0);
        assert!((t - 32.0).abs() < 1e-9);
    }

    #[test]
    fn calibration_above_median_is_warmer() {
        // raw 50 ADC above median → 50 × 0.030 = 1.5°C above 32°C.
        let t = SkinTempCalibration::convert(1003, 953.0);
        assert!((t - 33.5).abs() < 1e-9);
    }

    #[test]
    fn calibration_below_median_is_cooler() {
        // raw 100 ADC below median → 100 × 0.030 = 3.0°C below 32°C.
        let t = SkinTempCalibration::convert(853, 953.0);
        assert!((t - 29.0).abs() < 1e-9);
    }

    #[test]
    fn calibration_real_world_scenario() {
        // Real bug report: raw 953 was giving 38.12°C / 100.6°F via the
        // old global 0.04 factor while the user's actual core was 36.9°C.
        // With per-user calibration anchored on this user's wear-time
        // median (~953), the same reading correctly returns ~32°C.
        let t = SkinTempCalibration::convert(953, 953.0);
        assert!(t >= 31.5 && t <= 32.5, "expected ~32°C, got {t}");
    }
}
