//! Thresholds and constants for sleep staging.
//!
//! Every value here carries a citation or physiological rationale. If you
//! change a number, update the rationale in the doc comment too.

/// Epoch length in seconds. 30 s is the PSG scoring standard used by every
/// published consumer-wearable sleep staging algorithm (Wulterkens 2021,
/// Fonseca 2023, MESA pipeline). Do not change without retraining every
/// downstream threshold.
pub const EPOCH_SECONDS: i64 = 30;

/// Lower physiological bound on RR interval (ms). Corresponds to a HR
/// ceiling of 200 BPM. Values below this are parsing noise or signal
/// dropouts (Task Force of the ESC/NASPE, Circulation 1996).
pub const RR_MIN_MS: u16 = 300;

/// Upper physiological bound on RR interval (ms). Corresponds to a HR
/// floor of 30 BPM. Values above this are missed-beat artifacts. Same
/// reference as RR_MIN_MS.
pub const RR_MAX_MS: u16 = 2000;

/// Ectopic detection threshold: successive RR difference > 20% of the
/// preceding interval marks an ectopic beat (Malik 1996 criterion, used
/// by NeuroKit2's `hrv_rr_intervals_cleaning`).
pub const ECTOPIC_DIFF_RATIO: f64 = 0.20;

/// If the fraction of ectopics in an epoch exceeds this, the epoch's HRV
/// features are deemed unreliable and marked invalid. 30% is the
/// conservative cutoff used by NeuroKit2 and Philips' sleep staging
/// pipeline (Fonseca 2023).
pub const ECTOPIC_REJECTION_THRESHOLD: f64 = 0.30;

/// Minimum seconds of data coverage required in an epoch to attempt
/// feature extraction. Sub-15-second epochs have too few samples for
/// stable HRV estimates (PRD §5.1).
pub const MIN_EPOCH_COVERAGE_SECS: f64 = 15.0;

/// Minimum RR intervals required for time-domain HRV. Ten is the minimum
/// practical for RMSSD; below this the RMSSD is dominated by a single
/// interval's noise (PRD §5.2.2, Task Force 1996).
pub const MIN_RR_PER_EPOCH: usize = 10;

/// Standard HRV resampling frequency (Hz). 4 Hz is the literature
/// convention for interpolating irregular RR to a uniform grid before
/// FFT-based spectral analysis (Camm et al. 1996, NeuroKit2 default).
pub const INTERP_FS_HZ: f64 = 4.0;

/// Window (seconds) centered on each epoch for frequency-domain HRV and
/// respiratory-rate analysis. A 30 s epoch alone gives ~0.033 Hz
/// frequency resolution — too coarse for VLF. A 5-min context window
/// lifts resolution to ~0.003 Hz, matching the VLF lower bound (Shaffer
/// & Ginsberg 2017, "An Overview of HRV Metrics and Norms").
pub const FREQ_CONTEXT_WINDOW_SECS: f64 = 300.0;

/// Width of the respiratory-rate window (seconds). 2 min is long enough
/// to resolve 0.1 Hz (6 breaths/min) comfortably; shorter windows make
/// low-rate breaths indistinguishable from DC offset (PRD open
/// question #3, also Pinheiro 2016).
pub const RESP_WINDOW_SECS: f64 = 120.0;

/// HF band threshold for "high respiratory rate" — if mean HR in the
/// epoch exceeds this, respiratory rate is suppressed (it becomes
/// unreliable due to low RSA amplitude; PRD §8 risk table).
pub const RESP_MAX_HR: f64 = 100.0;

// HRV frequency bands (Hz), exactly as specified in Task Force 1996
// and preserved by NeuroKit2, MESA, and every downstream benchmark.
// These MUST NOT be altered, even by a small epsilon, without a Phase 2
// feature-remapping shim.
pub const VLF_LOW_HZ: f64 = 0.003;
pub const VLF_HIGH_HZ: f64 = 0.04;
pub const LF_LOW_HZ: f64 = 0.04;
pub const LF_HIGH_HZ: f64 = 0.15;
pub const HF_LOW_HZ: f64 = 0.15;
pub const HF_HIGH_HZ: f64 = 0.4;

// Respiratory band (Hz) — 6 to 30 breaths per minute, which covers the
// full physiological range from deep sleep (6-10 bpm) to waking arousal
// (up to ~25 bpm).
pub const RESP_LOW_HZ: f64 = 0.1;
pub const RESP_HIGH_HZ: f64 = 0.5;

/// pNN50 threshold (ms). Counts successive RR differences > 50 ms, a
/// parasympathetic marker established by Bigger et al. 1988.
pub const PNN_THRESHOLD_MS: f64 = 50.0;

/// Stillness-delta threshold (g). Gravity-vector change magnitude below
/// this per 1 Hz sample is considered "still." 0.01 g matches the
/// existing gravity-stillness sleep detector in activity.rs.
pub const STILLNESS_DELTA_G: f32 = 0.01;

/// Sample entropy parameters. m=2, r=0.2·SDNN is the Richman & Moorman
/// 2000 standard used throughout the HRV literature.
pub const SAMPEN_M: usize = 2;
pub const SAMPEN_R_FRAC: f64 = 0.2;
