//! Sleep architecture metrics derived from a stage sequence.
//!
//! All functions are pure over a `&[EpochStage]` (or `&[SleepStage]`).
//! Each epoch represents 30 seconds (`EPOCH_SECONDS` in
//! [`super::constants`]); multiplying by 0.5 converts epoch counts to
//! minutes.

use chrono::NaiveDateTime;

use super::classifier::{EpochStage, SleepStage};

/// Epochs → minutes conversion: each epoch is 30 s = 0.5 min.
const EPOCH_MINUTES: f64 = 0.5;

/// Minimum duration of the first non-Wake block that qualifies as
/// "sleep onset" for the latency calculation. 3 minutes (6 epochs)
/// rejects brief microsleeps from counting as onset. Matches standard
/// PSG scoring rules (Iber et al. 2007 AASM manual, ch. 7).
const SLEEP_ONSET_MIN_EPOCHS: usize = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct ArchitectureMetrics {
    pub total_in_bed_minutes: f64,
    pub total_sleep_minutes: f64,
    pub sleep_latency_minutes: f64,
    pub waso_minutes: f64,
    pub sleep_efficiency: f64,
    pub awake_minutes: f64,
    pub light_minutes: f64,
    pub deep_minutes: f64,
    pub rem_minutes: f64,
    pub awake_pct: f64,
    pub light_pct: f64,
    pub deep_pct: f64,
    pub rem_pct: f64,
    pub restorative_minutes: f64,
    pub wake_event_count: u32,
    pub cycle_count: u32,
}

pub fn compute_metrics(
    sleep_start: NaiveDateTime,
    sleep_end: NaiveDateTime,
    stages: &[EpochStage],
) -> ArchitectureMetrics {
    let tib = total_in_bed_minutes(sleep_start, sleep_end);
    let stage_seq: Vec<SleepStage> = stages.iter().map(|e| e.stage).collect();

    let awake_minutes = stage_minutes(&stage_seq, SleepStage::Wake);
    let light_minutes = stage_minutes(&stage_seq, SleepStage::Light);
    let deep_minutes = stage_minutes(&stage_seq, SleepStage::Deep);
    let rem_minutes = stage_minutes(&stage_seq, SleepStage::Rem);
    let total_sleep_minutes = light_minutes + deep_minutes + rem_minutes;

    let efficiency = if tib > 0.0 {
        100.0 * total_sleep_minutes / tib
    } else {
        0.0
    };

    ArchitectureMetrics {
        total_in_bed_minutes: tib,
        total_sleep_minutes,
        sleep_latency_minutes: sleep_latency_minutes(&stage_seq),
        waso_minutes: waso_minutes(&stage_seq),
        sleep_efficiency: efficiency,
        awake_minutes,
        light_minutes,
        deep_minutes,
        rem_minutes,
        awake_pct: pct(awake_minutes, tib),
        light_pct: pct(light_minutes, tib),
        deep_pct: pct(deep_minutes, tib),
        rem_pct: pct(rem_minutes, tib),
        restorative_minutes: deep_minutes + rem_minutes,
        wake_event_count: wake_event_count(&stage_seq),
        cycle_count: cycle_count(&stage_seq),
    }
}

pub fn total_in_bed_minutes(start: NaiveDateTime, end: NaiveDateTime) -> f64 {
    let delta = end.signed_duration_since(start);
    (delta.num_seconds().max(0) as f64) / 60.0
}

pub fn stage_minutes(stages: &[SleepStage], stage: SleepStage) -> f64 {
    stages.iter().filter(|&&s| s == stage).count() as f64 * EPOCH_MINUTES
}

fn pct(minutes: f64, tib: f64) -> f64 {
    if tib <= 0.0 {
        0.0
    } else {
        100.0 * minutes / tib
    }
}

/// Time from the start of the sleep window to the first contiguous
/// non-Wake block lasting ≥ `SLEEP_ONSET_MIN_EPOCHS`. If sleep never
/// consolidates, returns the total window duration (i.e. the user
/// never fell asleep).
pub fn sleep_latency_minutes(stages: &[SleepStage]) -> f64 {
    let mut run_start: Option<usize> = None;
    for (i, &s) in stages.iter().enumerate() {
        let is_sleep = matches!(s, SleepStage::Light | SleepStage::Deep | SleepStage::Rem);
        match (is_sleep, run_start) {
            (true, None) => run_start = Some(i),
            (true, Some(start)) => {
                if i - start + 1 >= SLEEP_ONSET_MIN_EPOCHS {
                    return start as f64 * EPOCH_MINUTES;
                }
            }
            (false, _) => run_start = None,
        }
    }
    stages.len() as f64 * EPOCH_MINUTES
}

/// Wake After Sleep Onset: total Wake minutes between the first and
/// last sleep epoch. Excludes pre-onset Wake (that's "latency") and
/// post-wake wake (that's trailing).
pub fn waso_minutes(stages: &[SleepStage]) -> f64 {
    let first = stages
        .iter()
        .position(|s| matches!(s, SleepStage::Light | SleepStage::Deep | SleepStage::Rem));
    let last = stages
        .iter()
        .rposition(|s| matches!(s, SleepStage::Light | SleepStage::Deep | SleepStage::Rem));
    match (first, last) {
        (Some(f), Some(l)) if l > f => stages[f..=l]
            .iter()
            .filter(|&&s| s == SleepStage::Wake)
            .count() as f64
            * EPOCH_MINUTES,
        _ => 0.0,
    }
}

/// Count of contiguous Wake runs *after* sleep onset. Each run of one
/// or more Wake epochs between two sleep epochs counts as a single
/// wake event.
pub fn wake_event_count(stages: &[SleepStage]) -> u32 {
    let first = stages
        .iter()
        .position(|s| matches!(s, SleepStage::Light | SleepStage::Deep | SleepStage::Rem));
    let last = stages
        .iter()
        .rposition(|s| matches!(s, SleepStage::Light | SleepStage::Deep | SleepStage::Rem));
    let (start, end) = match (first, last) {
        (Some(f), Some(l)) => (f, l),
        _ => return 0,
    };
    let mut count = 0u32;
    let mut in_wake_run = false;
    for &s in &stages[start..=end] {
        if s == SleepStage::Wake {
            if !in_wake_run {
                count += 1;
                in_wake_run = true;
            }
        } else {
            in_wake_run = false;
        }
    }
    count
}

/// Number of NREM→REM sleep cycles. A "cycle" is counted by the number
/// of distinct REM episodes during the night — the clinical
/// convention (Carskadon & Dement). PRD §5.4 describes this as
/// "Light→Deep→Light→REM→Wake progressions"; the REM-episode count
/// is the practical realization of that (each REM episode caps one
/// cycle). Two REM episodes separated by only `Unknown` epochs count
/// as one continuous episode.
pub fn cycle_count(stages: &[SleepStage]) -> u32 {
    let mut count = 0u32;
    let mut in_rem = false;
    for &s in stages {
        match s {
            SleepStage::Rem => {
                if !in_rem {
                    count += 1;
                    in_rem = true;
                }
            }
            SleepStage::Unknown => {}
            _ => in_rem = false,
        }
    }
    count
}

/// Collapse a per-epoch stage sequence into a run-length-encoded
/// hypnogram at 1-minute resolution (2 epochs per minute). Each entry
/// is `(start, end, stage)`. Used by the Tauri snapshot.
pub fn quantized_hypnogram(stages: &[EpochStage]) -> Vec<HypnogramSegment> {
    if stages.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seg_start = stages[0].epoch_start;
    let mut seg_stage = stages[0].stage;
    let mut seg_end = stages[0].epoch_end;
    for e in &stages[1..] {
        if e.stage == seg_stage {
            seg_end = e.epoch_end;
        } else {
            out.push(HypnogramSegment {
                start: seg_start,
                end: seg_end,
                stage: seg_stage,
            });
            seg_start = e.epoch_start;
            seg_stage = e.stage;
            seg_end = e.epoch_end;
        }
    }
    out.push(HypnogramSegment {
        start: seg_start,
        end: seg_end,
        stage: seg_stage,
    });

    // Round segment boundaries to the nearest minute so the UI can
    // render at 1-minute resolution. Keep microsecond-exact internal.
    for seg in &mut out {
        seg.start = round_to_minute(seg.start);
        seg.end = round_to_minute(seg.end);
    }
    out
}

fn round_to_minute(t: NaiveDateTime) -> NaiveDateTime {
    let secs = t.and_utc().timestamp();
    let rounded = (secs + 30) / 60 * 60;
    chrono::DateTime::<chrono::Utc>::from_timestamp(rounded, 0)
        .map(|d| d.naive_utc())
        .unwrap_or(t)
}

#[derive(Debug, Clone, PartialEq)]
pub struct HypnogramSegment {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub stage: SleepStage,
}

impl ArchitectureMetrics {
    /// Convenience constructor for a cycle with no sleep at all
    /// (e.g. an aborted detection). Keeps downstream code from having
    /// to handle an Option<ArchitectureMetrics>.
    pub fn empty(tib: f64) -> Self {
        Self {
            total_in_bed_minutes: tib,
            total_sleep_minutes: 0.0,
            sleep_latency_minutes: tib,
            waso_minutes: 0.0,
            sleep_efficiency: 0.0,
            awake_minutes: tib,
            light_minutes: 0.0,
            deep_minutes: 0.0,
            rem_minutes: 0.0,
            awake_pct: 100.0,
            light_pct: 0.0,
            deep_pct: 0.0,
            rem_pct: 0.0,
            restorative_minutes: 0.0,
            wake_event_count: 0,
            cycle_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeDelta};
    use SleepStage::*;

    fn dt(m: i64) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 16)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap()
            + TimeDelta::minutes(m)
    }

    fn to_epochs(stages: &[SleepStage]) -> Vec<EpochStage> {
        stages
            .iter()
            .enumerate()
            .map(|(i, &s)| EpochStage {
                epoch_start: dt(0) + TimeDelta::seconds((i * 30) as i64),
                epoch_end: dt(0) + TimeDelta::seconds(((i + 1) * 30) as i64),
                stage: s,
                classifier_version: "rule-v1",
            })
            .collect()
    }

    #[test]
    fn tib_8_hours() {
        let tib = total_in_bed_minutes(dt(0), dt(480));
        assert_eq!(tib, 480.0);
    }

    #[test]
    fn stage_minutes_counts_epochs_times_half() {
        let stages = [Light, Light, Deep, Deep, Deep, Rem, Wake];
        assert_eq!(stage_minutes(&stages, Light), 1.0);
        assert_eq!(stage_minutes(&stages, Deep), 1.5);
        assert_eq!(stage_minutes(&stages, Rem), 0.5);
        assert_eq!(stage_minutes(&stages, Wake), 0.5);
    }

    #[test]
    fn latency_first_run_of_6_epochs() {
        // 4 Wake, then 6 Light: onset at epoch 4 = 2 min
        let mut stages = vec![Wake; 4];
        stages.extend(std::iter::repeat_n(Light, 6));
        assert_eq!(sleep_latency_minutes(&stages), 2.0);
    }

    #[test]
    fn latency_ignores_short_sleep_runs() {
        // 2 Wake, 3 Light, 2 Wake, 6 Light: first onset that sticks is at epoch 7 = 3.5 min
        let mut stages = vec![Wake; 2];
        stages.extend(std::iter::repeat_n(Light, 3));
        stages.extend(std::iter::repeat_n(Wake, 2));
        stages.extend(std::iter::repeat_n(Light, 6));
        assert_eq!(sleep_latency_minutes(&stages), 3.5);
    }

    #[test]
    fn waso_only_counts_between_first_and_last_sleep() {
        // Wake, Wake, Light, Wake, Wake, Light, Wake, Wake
        // WASO = 2 Wake epochs between first Light and last Light = 1.0 min
        let stages = vec![Wake, Wake, Light, Wake, Wake, Light, Wake, Wake];
        assert_eq!(waso_minutes(&stages), 1.0);
    }

    #[test]
    fn waso_zero_when_no_sleep() {
        let stages = vec![Wake, Wake, Wake];
        assert_eq!(waso_minutes(&stages), 0.0);
    }

    #[test]
    fn wake_event_count_counts_contiguous_runs() {
        // Light, Wake, Wake, Light, Wake, Light → 2 wake events
        let stages = vec![Light, Wake, Wake, Light, Wake, Light];
        assert_eq!(wake_event_count(&stages), 2);
    }

    #[test]
    fn wake_event_count_ignores_trailing_wake() {
        let stages = vec![Light, Light, Wake, Wake, Wake];
        assert_eq!(wake_event_count(&stages), 0);
    }

    #[test]
    fn cycle_count_counts_distinct_rem_episodes() {
        // 3 REM episodes separated by NREM
        let stages = vec![
            Light, Deep, Rem, Rem, Light, Deep, Rem, Light, Rem,
        ];
        assert_eq!(cycle_count(&stages), 3);
    }

    #[test]
    fn cycle_count_unknown_does_not_break_run() {
        let stages = vec![Light, Rem, Unknown, Rem, Light];
        assert_eq!(cycle_count(&stages), 1);
    }

    #[test]
    fn cycle_count_no_rem_gives_zero() {
        let stages = vec![Light, Deep, Light, Wake, Light];
        assert_eq!(cycle_count(&stages), 0);
    }

    #[test]
    fn compute_metrics_8h_typical() {
        // 8 h × 120 epochs = 960 epochs
        // 15 min Wake latency (30 eps) + 360 min mixed sleep (720 eps) + 105 min trailing.
        let mut seq = Vec::new();
        seq.extend(std::iter::repeat_n(Wake, 30));
        // Simplified cycle: 30 Light + 20 Deep + 30 Light + 20 REM, repeat 4x = 400 epochs
        for _ in 0..4 {
            seq.extend(std::iter::repeat_n(Light, 30));
            seq.extend(std::iter::repeat_n(Deep, 20));
            seq.extend(std::iter::repeat_n(Light, 30));
            seq.extend(std::iter::repeat_n(Rem, 20));
        }
        // Pad with Wake to 960 epochs.
        while seq.len() < 960 {
            seq.push(Wake);
        }

        let epochs = to_epochs(&seq);
        let start = dt(0);
        let end = dt(0) + TimeDelta::minutes(480);
        let m = compute_metrics(start, end, &epochs);

        assert_eq!(m.total_in_bed_minutes, 480.0);
        // 4 cycles × (30+20+30+20 = 100 sleep epochs) = 400 sleep epochs = 200 min
        assert_eq!(m.total_sleep_minutes, 200.0);
        assert_eq!(m.cycle_count, 4);
        assert_eq!(m.sleep_latency_minutes, 15.0);
        // WASO: Wake epochs between first and last sleep = 0 here (all wake is pre/post)
        assert_eq!(m.waso_minutes, 0.0);
        assert!((m.sleep_efficiency - (200.0 / 480.0 * 100.0)).abs() < 1e-9);
    }

    #[test]
    fn quantized_hypnogram_collapses_runs() {
        let stages = to_epochs(&[Light, Light, Light, Deep, Deep, Rem]);
        let hypno = quantized_hypnogram(&stages);
        assert_eq!(hypno.len(), 3);
        assert_eq!(hypno[0].stage, Light);
        assert_eq!(hypno[1].stage, Deep);
        assert_eq!(hypno[2].stage, Rem);
    }
}
