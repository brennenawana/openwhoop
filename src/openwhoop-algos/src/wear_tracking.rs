//! Wear-period derivation from two signals:
//!
//! 1. Primary: WristOn/WristOff events (from the `events` table). These
//!    are authoritative — the strap's capacitive sensor raises these
//!    on genuine on/off-wrist transitions.
//! 2. Fallback: contiguous runs of `skin_contact = 1` in
//!    `heart_rate.sensor_data`. Used when events aren't available for a
//!    time range.
//!
//! The algorithm in [`derive_wear_periods`] is pure over its inputs;
//! DB reads live in `openwhoop-db`. Pipeline integration bolts this on
//! to the sync loop.

use chrono::NaiveDateTime;
#[cfg(test)]
use chrono::TimeDelta;

/// Minimum duration for a wear period to count. Short blips
/// (<5 min) are usually artifacts — "picked up + put down" — per
/// PRD §10.4.
pub const MIN_WEAR_PERIOD_MINUTES: f64 = 5.0;

/// Maximum gap between contiguous `skin_contact = 1` samples that
/// still counts as one period (we merge across it). PRD §4-4.
pub const SKIN_CONTACT_MERGE_GAP_SECS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WearSource {
    Events,
    SkinContact,
    Fused,
}

impl WearSource {
    pub fn as_str(self) -> &'static str {
        match self {
            WearSource::Events => "events",
            WearSource::SkinContact => "skin_contact",
            WearSource::Fused => "fused",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WearPeriod {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub source: WearSource,
}

impl WearPeriod {
    pub fn duration_minutes(&self) -> f64 {
        (self.end - self.start).num_seconds().max(0) as f64 / 60.0
    }
}

/// One row from the `events` table projected down to what wear
/// tracking cares about.
#[derive(Debug, Clone)]
pub struct WearEvent {
    pub timestamp: NaiveDateTime,
    /// True = WristOn, False = WristOff. Other events should be
    /// filtered out by the caller.
    pub on: bool,
}

/// A contiguous run of `skin_contact = 1` samples. Caller derives
/// these from `heart_rate.sensor_data` — we only need the start/end
/// range for the merge.
#[derive(Debug, Clone)]
pub struct SkinContactRun {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

/// Derive wear periods from both input signals. Events are
/// authoritative where they exist; skin_contact fills in gaps.
///
/// `window_start`/`window_end` bound the time range we're considering.
/// Periods returned are always inside `[window_start, window_end]`
/// (we clip).
pub fn derive_wear_periods(
    events: &[WearEvent],
    skin_runs: &[SkinContactRun],
    window_start: NaiveDateTime,
    window_end: NaiveDateTime,
) -> Vec<WearPeriod> {
    let mut periods: Vec<WearPeriod> = Vec::new();

    // Step 1: events-derived periods.
    let mut open_on: Option<NaiveDateTime> = None;
    let last_sample_time = skin_runs.iter().map(|r| r.end).max().unwrap_or(window_end);
    for ev in events {
        if ev.timestamp < window_start || ev.timestamp > window_end {
            continue;
        }
        match (ev.on, open_on) {
            (true, None) => open_on = Some(ev.timestamp),
            (true, Some(_)) => {
                // Two WristOn in a row — ignore the second. Strap shouldn't
                // do this but be defensive.
            }
            (false, Some(start)) => {
                periods.push(WearPeriod {
                    start,
                    end: ev.timestamp,
                    source: WearSource::Events,
                });
                open_on = None;
            }
            (false, None) => {
                // WristOff with no matching WristOn — per PRD, start from
                // earliest skin_contact run in the vicinity. Conservative
                // fallback: use window_start if no runs.
                let inferred_start = skin_runs
                    .iter()
                    .rev()
                    .find(|r| r.end <= ev.timestamp && r.start >= window_start)
                    .map(|r| r.start)
                    .unwrap_or(window_start);
                periods.push(WearPeriod {
                    start: inferred_start,
                    end: ev.timestamp,
                    source: WearSource::Events,
                });
            }
        }
    }
    // Dangling open WristOn at window end — close at the last observed
    // heart_rate sample with skin_contact=1 (per PRD), or window_end.
    if let Some(start) = open_on {
        periods.push(WearPeriod {
            start,
            end: last_sample_time.min(window_end),
            source: WearSource::Events,
        });
    }

    // Step 2: merge skin_contact runs to fill events-gaps.
    let merged_skin = merge_skin_runs(skin_runs);
    let mut skin_periods: Vec<WearPeriod> = Vec::new();
    for run in merged_skin {
        if run.end < window_start || run.start > window_end {
            continue;
        }
        let start = run.start.max(window_start);
        let end = run.end.min(window_end);
        let dur_min = (end - start).num_seconds() as f64 / 60.0;
        if dur_min >= MIN_WEAR_PERIOD_MINUTES {
            skin_periods.push(WearPeriod {
                start,
                end,
                source: WearSource::SkinContact,
            });
        }
    }

    // Step 3: merge the two lists. If a skin_contact period is
    // entirely inside an events period, drop it (events are
    // authoritative). If it overlaps, the overlapping portion stays
    // events; the non-overlapping tail stays skin_contact (tagged
    // 'fused' for any period that was stitched).
    let mut out: Vec<WearPeriod> = Vec::new();
    out.extend(periods);
    for sp in skin_periods {
        let mut remaining = vec![(sp.start, sp.end)];
        let mut new_remaining: Vec<(NaiveDateTime, NaiveDateTime)> = Vec::new();
        let mut fused = false;
        for ep in &out {
            if ep.source != WearSource::Events && ep.source != WearSource::Fused {
                continue;
            }
            for (r_start, r_end) in &remaining {
                // Subtract ep's range from this remaining chunk.
                if ep.end <= *r_start || ep.start >= *r_end {
                    // no overlap
                    new_remaining.push((*r_start, *r_end));
                } else {
                    fused = true;
                    // left part
                    if *r_start < ep.start {
                        new_remaining.push((*r_start, ep.start));
                    }
                    // right part
                    if *r_end > ep.end {
                        new_remaining.push((ep.end, *r_end));
                    }
                }
            }
            remaining = std::mem::take(&mut new_remaining);
        }
        for (s, e) in remaining {
            let dur_min = (e - s).num_seconds() as f64 / 60.0;
            if dur_min < MIN_WEAR_PERIOD_MINUTES {
                continue;
            }
            out.push(WearPeriod {
                start: s,
                end: e,
                source: if fused {
                    WearSource::Fused
                } else {
                    WearSource::SkinContact
                },
            });
        }
    }

    // Sort and drop anything shorter than the minimum.
    out.sort_by_key(|p| p.start);
    out.retain(|p| p.duration_minutes() >= MIN_WEAR_PERIOD_MINUTES);
    out
}

/// Merge skin_contact runs separated by less than
/// `SKIN_CONTACT_MERGE_GAP_SECS`.
fn merge_skin_runs(runs: &[SkinContactRun]) -> Vec<SkinContactRun> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<SkinContactRun> = runs.to_vec();
    sorted.sort_by_key(|r| r.start);
    let mut out: Vec<SkinContactRun> = Vec::with_capacity(sorted.len());
    for run in sorted {
        if let Some(last) = out.last_mut() {
            let gap = (run.start - last.end).num_seconds();
            if gap <= SKIN_CONTACT_MERGE_GAP_SECS {
                if run.end > last.end {
                    last.end = run.end;
                }
                continue;
            }
        }
        out.push(run);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(mins: i64) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 4, 17)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            + TimeDelta::minutes(mins)
    }

    #[test]
    fn events_derive_brackets_wriston_wristoff_pairs() {
        let events = vec![
            WearEvent { timestamp: dt(0), on: true },
            WearEvent { timestamp: dt(120), on: false },
        ];
        let periods = derive_wear_periods(&events, &[], dt(-60), dt(300));
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].source, WearSource::Events);
        assert_eq!(periods[0].start, dt(0));
        assert_eq!(periods[0].end, dt(120));
    }

    #[test]
    fn skin_contact_fallback_runs_when_no_events() {
        let runs = vec![SkinContactRun {
            start: dt(0),
            end: dt(30),
        }];
        let periods = derive_wear_periods(&[], &runs, dt(-60), dt(300));
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].source, WearSource::SkinContact);
        assert_eq!(periods[0].duration_minutes(), 30.0);
    }

    #[test]
    fn short_skin_contact_blip_is_dropped() {
        let runs = vec![SkinContactRun {
            start: dt(0),
            end: dt(3), // 3 min < 5 min threshold
        }];
        let periods = derive_wear_periods(&[], &runs, dt(-60), dt(300));
        assert!(periods.is_empty());
    }

    #[test]
    fn skin_runs_close_together_merge() {
        let runs = vec![
            SkinContactRun { start: dt(0), end: dt(10) },
            SkinContactRun { start: dt(10) + TimeDelta::seconds(30), end: dt(30) },
        ];
        let merged = merge_skin_runs(&runs);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start, dt(0));
        assert_eq!(merged[0].end, dt(30));
    }

    #[test]
    fn skin_runs_with_large_gap_do_not_merge() {
        let runs = vec![
            SkinContactRun { start: dt(0), end: dt(10) },
            SkinContactRun { start: dt(20), end: dt(40) },
        ];
        let merged = merge_skin_runs(&runs);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn events_inside_skin_contact_produces_fused_period() {
        // Events cover 10-60. Skin contact covers 0-120. Result:
        // one events period (10-60) and one fused (60-120).
        let events = vec![
            WearEvent { timestamp: dt(10), on: true },
            WearEvent { timestamp: dt(60), on: false },
        ];
        let runs = vec![SkinContactRun { start: dt(0), end: dt(120) }];
        let periods = derive_wear_periods(&events, &runs, dt(-60), dt(300));
        assert!(periods.iter().any(|p| p.source == WearSource::Events));
        assert!(periods.iter().any(|p| p.source == WearSource::Fused));
    }

    #[test]
    fn dangling_wriston_closes_at_last_sample() {
        let events = vec![WearEvent { timestamp: dt(0), on: true }];
        let runs = vec![SkinContactRun { start: dt(0), end: dt(90) }];
        let periods = derive_wear_periods(&events, &runs, dt(-60), dt(300));
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].source, WearSource::Events);
        assert_eq!(periods[0].end, dt(90));
    }

    #[test]
    fn orphan_wristoff_uses_prior_skin_contact_start() {
        // WristOff at t=100 with no prior WristOn. A skin_contact run
        // from t=30 to t=50. The WristOff period should start at t=30.
        let events = vec![WearEvent { timestamp: dt(100), on: false }];
        let runs = vec![SkinContactRun { start: dt(30), end: dt(50) }];
        let periods = derive_wear_periods(&events, &runs, dt(-60), dt(300));
        assert!(periods.iter().any(|p| p.source == WearSource::Events && p.start == dt(30)));
    }
}
