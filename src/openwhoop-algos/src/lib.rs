pub(crate) mod activity;
pub use activity::{
    ActivityPeriod, MAX_SLEEP_PAUSE, MAX_WORKOUT_DURATION, MIN_WORKOUT_DURATION,
};

pub(crate) mod sleep;
pub use sleep::SleepCycle;

pub(crate) mod sleep_consistency;
pub use sleep_consistency::SleepConsistencyAnalyzer;

pub(crate) mod stress;
pub use stress::{StressCalculator, StressScore};

pub(crate) mod exercise;
pub use exercise::ExerciseMetrics;

pub(crate) mod strain;
pub use strain::{StrainCalculator, StrainScore};

pub(crate) mod spo2;
pub use spo2::{SpO2Calculator, SpO2Reading, SpO2Score};

pub(crate) mod temperature;
pub use temperature::{SkinTempCalculator, SkinTempScore};

pub mod helpers;

pub mod sleep_staging;

pub(crate) mod wear_tracking;
pub use wear_tracking::{WearEvent, WearPeriod, WearSource, SkinContactRun, derive_wear_periods};

pub(crate) mod daytime_hrv;
pub use daytime_hrv::{HrvContext, HrvSample, compute_daytime_hrv};

pub(crate) mod activity_classifier;
pub use activity_classifier::{
    ACTIVITY_CLASSIFIER_VERSION, ActivityClass, ActivitySample, classify_activities,
};
