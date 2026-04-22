pub mod architecture;
pub mod baselines;
pub mod classifier;
pub mod constants;
pub mod features;
pub mod respiratory;
pub mod scoring;
pub mod skin_temp;

pub use architecture::{ArchitectureMetrics, HypnogramSegment, compute_metrics, quantized_hypnogram};
pub use baselines::{
    BASELINE_WINDOW_NIGHTS, BaselineSnapshot, NightAggregate, compute_baseline, should_update,
};
pub use classifier::{CLASSIFIER_VERSION, EpochStage, SleepStage, UserBaseline, classify_epochs};
pub use features::{EpochFeatures, build_epochs};
pub use respiratory::{RespiratoryStats, nightly_respiratory_rate};
pub use constants::NEED_BASELINE_WINDOW_NIGHTS;
pub use scoring::{
    BaselineNight, NightSleep, PerformanceComponents, PerformanceScore, ScoringInputs,
    SleepNeedInputs, baseline_eligible_nights, performance_score, personalized_baseline_hours,
    sleep_debt_hours, sleep_need_hours, sleep_surplus_hours, strain_addition_hours,
};
pub use skin_temp::nightly_skin_temp;
