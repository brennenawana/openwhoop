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

// Respiratory band (Hz) — 9 to 24 breaths per minute. Matches the HF
// HRV band (0.15-0.4 Hz), which is the physiological home of
// respiratory sinus arrhythmia in adults. The PRD's original
// 0.1-0.5 Hz range overlapped with the LF band (0.04-0.15 Hz, Mayer
// waves / baroreflex ~0.1 Hz), producing spurious "8 bpm" respiratory
// estimates driven by LF contamination rather than real breathing.
// Tightened on 2026-04-17 after observing this on real user data.
pub const RESP_LOW_HZ: f64 = 0.15;
pub const RESP_HIGH_HZ: f64 = 0.40;

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

// ---------- classifier (rule-v1) thresholds ----------
//
// Each threshold here is either derived from a cited paper or is a
// conservative empirical choice. Tune with reclassify-sleep after
// observing the distribution on real user data.

/// Activity-count threshold above which an epoch is considered
/// "movement." Calibrated empirically to match WHOOP-scale activity
/// counts on wrist. Any sum-of-|accel - 1g| over 30 s exceeding this
/// is too high to be quiet sleep. Kept here (not in the classifier)
/// because it's dimensional and conceptually a staging constant.
pub const MOTION_WAKE_THRESHOLD: f64 = 20.0;

/// HR elevation (BPM above resting) required alongside motion to
/// classify Wake. Wulterkens 2021 uses a similar +15 BPM floor.
pub const WAKE_HR_OFFSET_BPM: f64 = 15.0;

/// Within-night HR percentile below which Deep is considered (rule-v2).
/// The ~25th percentile of the night's valid-epoch HR distribution
/// puts an epoch in the "quietest quarter of this night" — the
/// parasympathetic-floor region where SWS actually lives. Matches
/// the per-night normalization convention used by Walch 2019
/// (Apple Watch), Altini & Kinnunen 2021 (Oura), Roberts 2020,
/// Sridhar 2020, and the HypnosPy ECDF-quantile approach
/// (Perez-Pozuelo 2022). Tune after observing real-data distribution.
pub const DEEP_HR_PERCENTILE: f64 = 0.25;

/// Legacy absolute HR offset (rule-v1). Kept as a fallback for cases
/// where the within-night HR distribution is unavailable (fewer than
/// a handful of valid epochs). Treated as "HR < resting + offset";
/// rule-v2's primary gate is the within-night percentile above.
///
/// Historical note: this was the primary Deep-HR gate in rule-v1
/// (initially +5, bumped to +8 after early tuning). Replaced as
/// primary on 2026-04-18 after research confirmed that anchoring
/// Deep to a per-user baseline derived from nightly minimum HR is
/// structurally tautological — the nightly minimum IS a Deep HR.
/// See docs/DEEP_HR_RESEARCH_SYNTHESIS.md.
pub const DEEP_HR_OFFSET_BPM: f64 = 8.0;

/// Minimum stillness ratio for Deep. Deep sleep is the stillest stage
/// — gravity-vector barely moves across 30 s.
pub const DEEP_MOTION_STILLNESS: f64 = 0.95;

/// Minimum stillness ratio for REM. REM has occasional twitches so
/// it's slightly less still than Deep.
pub const REM_MOTION_STILLNESS: f64 = 0.85;
//
// NOTE: earlier versions gated Deep on resp_rate_std < 2.0 and REM on
// resp_rate_std > 3.0. Both were removed after real-data observation
// — the feature is computed from 3 × 40 s sub-window peak estimates,
// which at 4 Hz has ~1.5 bpm bin-quantization jitter alone, making
// the 2/3 thresholds fragile. Resp-rate-variability as a staging cue
// is cited in the literature (REM has irregular breathing) but is a
// secondary signal; HRV + motion are the primary discriminators.

/// Relative-night-position threshold below which REM is suppressed.
/// REM is back-loaded; classifying REM very early in the night is
/// physiologically unlikely. 0.2 (not 0.3) accommodates shorter
/// sleep windows where the absolute time is still long enough for
/// a REM episode (a 5 h night at rel_night=0.2 is already 1 h in).
pub const REL_NIGHT_REM_MIN: f64 = 0.2;

/// Relative-night-position ceiling for Deep. Deep is front-loaded
/// but not strictly first-half; the homeostatic drive still
/// produces occasional Deep episodes in the early-second-half
/// window, especially in short sleepers. Prior value was a strict
/// `is_first_half` (0.5 cutoff).
pub const REL_NIGHT_DEEP_MAX: f64 = 0.6;

/// Minimum stillness ratio for Light sleep. Below this, the epoch is
/// treated as Wake regardless of HRV features.
pub const LIGHT_MOTION_STILLNESS: f64 = 0.7;

// Within-night percentile thresholds (relative, self-normalizing).
// Starting values were 75/75/60 for Deep/REM/REM-hr-std; relaxed to
// 50/50/50 on 2026-04-17 after real-data tuning. Rationale: the
// multi-gate AND structure of the Deep and REM rules compounds
// individually-strict percentiles into near-empty intersections on
// short or feature-homogeneous nights. A 50th-percentile cut is the
// literature standard for "above-median high-parasympathetic / high-
// sympathetic" bucketing (Shaffer & Ginsberg 2017, Fonseca 2023),
// and combined with the other gates still yields selective stage
// assignment.

/// HF-power percentile above which Deep is considered. Median cut
/// captures the parasympathetic-dominant half of the night.
pub const SPECTRAL_HF_PERCENTILE: f64 = 0.50;

/// RMSSD percentile above which Deep is considered (rule-v2).
/// Within-night median: Deep candidates are the epochs with higher-
/// than-night-median vagal tone. Replaced the `rmssd > baseline.median`
/// rule for the same reason the HR gate moved — baseline medians
/// computed across multiple prior nights can sit above the current
/// night's distribution (especially for users whose overall HRV is
/// lower than the population default 47 ms), starving Deep.
pub const RMSSD_DEEP_PERCENTILE: f64 = 0.50;

/// LF/HF percentile above which REM is considered. Median cut
/// captures sympathovagal-elevated epochs relative to this night's
/// own distribution.
pub const LF_HF_PERCENTILE: f64 = 0.50;

/// HR-std percentile above which REM is considered. REM carries more
/// HR variability than Light, though less than Wake.
pub const HR_STD_PERCENTILE: f64 = 0.50;

// ---------- population fallback defaults ----------
//
// Used only when the per-user baseline has fewer than 14 nights of
// data. Replace with the user's own median once the baseline matures.

/// Population median resting HR (BPM). Rough midpoint of published
/// adult wake HR distributions (Quer et al. 2020 Nature: ~61 BPM
/// median across 92k wearable users).
pub const POPULATION_RESTING_HR: f64 = 62.0;

/// Population median sleep RMSSD (ms). Shaffer & Ginsberg 2017
/// normative data: adult sleep RMSSD median ~40 ms.
pub const POPULATION_SLEEP_RMSSD_MEDIAN: f64 = 40.0;

// ---------- sleep need / debt / scoring ----------
//
// Coefficients are grounded in public WHOOP sources (patent
// US20240252121A1, WHOOP developer API, Locker articles) and the
// canonical sleep-science literature WHOOP itself cites:
// Hirshkowitz 2015 (NSF), Watson 2015 (AASM), Van Dongen 2003,
// Belenky 2003, Rupp 2009, Mah 2011, Halson 2014, Dinges 1987,
// Lovato & Lack 2010. See docs/SLEEP_STAGING.md §8 for the full
// derivation.

/// Population-mean baseline sleep need (hours). Cold-start default
/// when the per-user baseline hasn't matured. Midpoint of Hirshkowitz
/// 2015's 7–9h NSF recommendation band for adults 18–64. WHOOP's own
/// published population mean baseline is 7.6h — close enough that
/// the midpoint is within noise.
pub const BASE_NEED_HOURS: f64 = 7.5;

/// Baseline personalization window (nights). WHOOP's Sleep Planner
/// uses 28 days ("past 28 days" per the Locker article). Match that.
pub const NEED_BASELINE_WINDOW_NIGHTS: usize = 28;

/// Minimum usable nights before the personalized baseline is
/// trusted. Below this we blend linearly toward BASE_NEED_HOURS so
/// the output isn't whipped around by one or two outlier nights.
pub const NEED_BASELINE_MIN_USABLE_NIGHTS: usize = 14;

/// Filter thresholds for "baseline-eligible" nights: the per-user
/// baseline is computed only from low-strain, well-slept,
/// nap-free nights so it reflects the user's unperturbed need.
/// Approximates WHOOP's undisclosed "typical rest day" rule.
pub const NEED_BASELINE_MAX_STRAIN: f64 = 10.0;
pub const NEED_BASELINE_MIN_EFFICIENCY_PCT: f64 = 85.0;
/// Nights under this duration are excluded from baseline. Below the
/// Watson 2015 AASM "≥7h" floor and the Van Dongen 2003 decrement
/// threshold — such nights aren't "typical rest day" samples, they
/// are already-deficit nights whose inclusion would normalize
/// undersleep as the user's target. Prevents the baseline from
/// collapsing on chronic undersleepers.
pub const NEED_BASELINE_MIN_SLEEP_HOURS: f64 = 6.0;

/// Strain-contribution coefficients. Formula (rule-v0):
///   Δ_strain_min = STRAIN_LINEAR_COEF · strain
///                + STRAIN_NONLINEAR_COEF · max(0, strain − STRAIN_NONLINEAR_THRESHOLD)^STRAIN_NONLINEAR_POWER
///                clamped to STRAIN_ADJ_CAP_MIN
///
/// Shape: small linear creep across the whole 0–21 range (so any
/// strain adds a tiny amount — matches WHOOP's "normal daily
/// activities count" copy) plus a super-linear term above strain 10
/// that dominates at high exertion (Borg scale is logarithmic;
/// Halson 2014 shows training-load effects are super-linear).
///
/// With these values:
///   strain  5 → +1.5 min
///   strain 10 → +3.0 min
///   strain 15 → +17.5 min
///   strain 18 → +34 min
///   strain 21 → +71 min (capped)
pub const STRAIN_LINEAR_COEF: f64 = 0.3;
pub const STRAIN_NONLINEAR_COEF: f64 = 6.0;
pub const STRAIN_NONLINEAR_THRESHOLD: f64 = 10.0;
pub const STRAIN_NONLINEAR_POWER: f64 = 1.35;
pub const STRAIN_ADJ_CAP_MIN: f64 = 90.0;

/// Sleep-debt coefficient. Applied to the decay-weighted mean
/// deficit (hours). Van Dongen 2003 shows 1 h/night × 14 nights
/// produces PVT decrements equivalent to 2 nights of total
/// deprivation — supports converting ~10 h of accumulated deficit
/// into 1–2 h of extra need. Coefficient of 0.5 on the weighted
/// mean puts us in that zone.
pub const DEBT_ADJ_COEF: f64 = 0.5;

/// Cap on the debt adjustment (hours). WHOOP's patent explicitly
/// says the debt term is "capped at a maximum."
pub const DEBT_ADJ_CAP_HOURS: f64 = 2.0;

/// Fraction of nap minutes credited toward sleep need. WHOOP's
/// own Locker copy ("sleep need that night is reduced by the amount
/// of time you napped for") and developer API (`need_from_recent_nap_milli`
/// can be negative) support near-1.0 credit. Dinges 1987 / Lovato
/// & Lack 2010 back the physiological basis.
pub const NAP_CREDIT_FRAC: f64 = 1.0;

/// Minimum gap between a qualifying nap's end and bedtime (minutes).
/// Guards against crediting naps taken just before sleep (they don't
/// subtract from drive; they pre-empt it).
pub const NAP_BEDTIME_GUARD_MINUTES: f64 = 120.0;

/// Sleep need is clamped to this range so a sensible target always
/// exists regardless of inputs. Floor from Watson 2015 AASM
/// ("<6h inadequate"); ceiling from Mah 2011 (Stanford basketball
/// intervention used 10h TIB target).
pub const MIN_SLEEP_NEED_HOURS: f64 = 6.0;
pub const MAX_SLEEP_NEED_HOURS: f64 = 10.5;

/// Sleep-debt decay weights across the last 7 nights (most recent
/// weighted highest). Belenky 2003 showed 3 nights recovery doesn't
/// fully clear 7 nights of restriction — supports a multi-day tail.
/// Weighted mean (Σ weights = 3.9) so coefficient is interpretable.
pub const DEBT_DECAY_WEIGHTS: [f64; 7] = [1.0, 0.85, 0.70, 0.55, 0.40, 0.25, 0.15];

/// When surplus-sleep banking is enabled, this coefficient applies
/// to the weighted-mean *excess* (actual − need, only positive)
/// across the debt window. Subtracts from tonight's need. Rupp 2009
/// showed surplus sleep is physiologically bankable on a ~7-day
/// timescale; WHOOP does NOT expose this behavior. Default is off.
pub const SURPLUS_CREDIT_COEF: f64 = 0.3;
pub const SURPLUS_CREDIT_CAP_HOURS: f64 = 1.0;

/// Target restorative (Deep + REM) percentage of time in bed.
/// 45% is the healthy adult benchmark from Hirshkowitz et al. 2015
/// National Sleep Foundation consensus (25% REM + 20% Deep target).
pub const RESTORATIVE_TARGET_PCT: f64 = 45.0;

/// Neutral fallback for missing consistency score (0–100). 50 is the
/// midpoint — assumes "we don't know" rather than penalizing or
/// rewarding.
pub const NEUTRAL_CONSISTENCY: f64 = 50.0;

/// Neutral fallback for missing sleep-stress score (0–10 Baevsky).
/// 5 corresponds to a 50 sleep_stress sub-score after inversion.
pub const NEUTRAL_SLEEP_STRESS: f64 = 5.0;

pub struct ScoreWeights {
    pub sufficiency: f64,
    pub efficiency: f64,
    pub restorative: f64,
    pub consistency: f64,
    pub sleep_stress: f64,
}

/// Component weights for the composite Sleep Performance Score. Sum
/// to 1.0. PRD §5.9. Prioritizes duration (sufficiency) like WHOOP.
pub const SCORE_WEIGHTS: ScoreWeights = ScoreWeights {
    sufficiency: 0.35,
    efficiency: 0.20,
    restorative: 0.20,
    consistency: 0.15,
    sleep_stress: 0.10,
};
