pub mod classifier;
pub mod constants;
pub mod features;

pub use classifier::{CLASSIFIER_VERSION, EpochStage, SleepStage, UserBaseline, classify_epochs};
pub use features::{EpochFeatures, build_epochs};
