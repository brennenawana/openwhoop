pub mod architecture;
pub mod classifier;
pub mod constants;
pub mod features;
pub mod scoring;

pub use architecture::{ArchitectureMetrics, HypnogramSegment, compute_metrics, quantized_hypnogram};
pub use classifier::{CLASSIFIER_VERSION, EpochStage, SleepStage, UserBaseline, classify_epochs};
pub use features::{EpochFeatures, build_epochs};
pub use scoring::{
    NightSleep, PerformanceComponents, PerformanceScore, ScoringInputs, SleepNeedInputs,
    performance_score, sleep_debt_hours, sleep_need_hours,
};
