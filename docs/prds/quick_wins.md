# PRD: OpenWhoop Quick Wins — Overnight Batch

**Status:** Draft v1.0
**Target:** OpenWhoop core library + Tauri tray app
**Scope:** Features designed for ≤8 hours of autonomous builder-agent work, no human oversight
**Last updated:** 2026-04-16

---

## 1. Objective

Close a set of audit-identified gaps in the OpenWhoop data pipeline and add new derivations from already-stored data, without modifying any existing working algorithm or live-hardware-dependent code path. Each feature in this PRD is independently valuable, schema-additive, and testable against existing SQLite data without a WHOOP strap present.

## 2. Design constraints (overnight autonomy)

These constraints apply to the whole batch:

- **Additive only.** New tables, new columns, new modules. No modification of existing algorithms (stress, SpO₂, skin_temp, sleep detection, RMSSD-in-sleep, strain, consistency). Exactly one allowed exception: adding `EVENTS_FROM_STRAP` UUID to `handle_packet` routing — that's a one-line addition, not a rewrite.
- **No new external dependencies** that require native compilation (C/C++ libs). Pure-Rust crates only.
- **Testable without hardware.** Every feature must be verifiable by (a) running unit tests, (b) running the feature against existing `heart_rate`/`sleep_cycles`/`packets` data, or (c) feeding synthetic packets through the parser in tests. No feature in this PRD requires a live strap connection to validate.
- **Independent.** Features 2–7 depend on Feature 1 (events ingestion) only for *future richness*, not for correctness. Each must work standalone even if its upstream dependency is buggy or skipped.
- **Fail-safe.** If a feature can't be completed, the builder skips it, documents the skip, and moves on. No feature's failure blocks any other.

## 3. Non-goals

- Modifying the existing sync pipeline's core ordering (download → detect → calculate). New computations append to the pipeline, they don't reorder it.
- Real-time BLE stream parsing (RealtimeData 0x28, RealtimeRawData 0x2B, RealtimeImuDataStream 0x33). These need live-hardware validation. Separate PRD.
- UI/frontend changes. This PRD only extends the backend snapshot data contract. Frontend work is a follow-up.
- Improvements to existing SpO₂, stress, skin-temp, or sleep algorithms. Those are non-trivial and warrant their own PRDs with validation plans.
- Sleep staging — covered by `PRD_sleep_staging.md`.

## 4. Features

### Feature 1: Events ingestion

**Problem:** The BLE audit identified that `EVENTS_FROM_STRAP` notifications are received but silently dropped due to a UUID routing gap in `handle_packet`. The parser *can* decode `Event`, `UnknownEvent`, and `RunAlarm` variants but matches them to empty `{}` handlers. Nine+ real-time event types are being discarded: WristOn/Off (9/10), ChargingOn/Off (7/8), BatteryLevel (3), DoubleTap (14), ExtendedBatteryInformation (63), External5vOn/Off (5/6), HighFreqSyncPrompt (96).

**Solution:**
1. Add `EVENTS_FROM_STRAP` UUID to the `handle_packet` match arm (alongside `DATA_FROM_STRAP` and `CMD_FROM_STRAP`).
2. Create an `events` table.
3. Replace the `WhoopData::Event { .. } => {}` and `WhoopData::UnknownEvent { .. } => {}` handlers with row inserts.
4. Replace the `WhoopData::RunAlarm { unix } => {}` handler with a row insert (also feeds Feature 3).

**Schema:**
```sql
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL,
    event_id INTEGER NOT NULL,
    event_name TEXT NOT NULL,
    raw_data JSON,
    synced BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX idx_events_timestamp ON events(timestamp);
CREATE INDEX idx_events_type ON events(event_id);
```

**Known event ID → name mapping** (from BLE audit, hard-code these):
```
3  -> "BatteryLevel"
5  -> "External5vOn"
6  -> "External5vOff"
7  -> "ChargingOn"
8  -> "ChargingOff"
9  -> "WristOn"
10 -> "WristOff"
14 -> "DoubleTap"
63 -> "ExtendedBatteryInformation"
96 -> "HighFreqSyncPrompt"
```
Unknown event IDs get `event_name = format!("Unknown({})", id)`.

**Acceptance:**
- `EVENTS_FROM_STRAP` UUID appears in `handle_packet`'s match statement.
- Unit test: synthetic Event packet fed through the parser produces one row in `events`.
- Unit test: unknown event ID is gracefully captured with `event_name = "Unknown(N)"` rather than panicking.
- Existing behavior unchanged: historical data download still works, all existing tests still pass.

### Feature 2: Device info persistence

**Problem:** `VersionInfo { harvard, boylston }` is info-logged but not stored. Every connect sees the firmware version, then forgets it. No way to correlate data quality to firmware version.

**Solution:** New `device_info` table. Persist one row on every successful `initialize()`. Also capture device name if the `CommandResponse` handler can be extended to parse `get_name` responses (if not, leave `device_name` as NULL — don't block on it).

**Schema:**
```sql
CREATE TABLE device_info (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at DATETIME NOT NULL,
    harvard_version TEXT,
    boylston_version TEXT,
    device_name TEXT
);
CREATE INDEX idx_device_info_recorded ON device_info(recorded_at);
```

**Acceptance:**
- After a successful initialize, exactly one new row exists in `device_info`.
- Unit test: calling the persistence function with `VersionInfo` inputs creates a row with matching version strings.

### Feature 3: Alarm history

**Problem:** `AlarmInfo` is used transiently in `get_alarm()` but never persisted. `RunAlarm { unix }` timestamps are dropped. No history of alarm sets, fires, or clears.

**Solution:** New `alarm_history` table. Write a row on `SetAlarmTime`, on `RunAlarm` event, and on `DisableAlarm`. Lifecycle tracking via separate columns.

**Schema:**
```sql
CREATE TABLE alarm_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    action TEXT NOT NULL CHECK(action IN ('set', 'fired', 'cleared', 'queried')),
    action_at DATETIME NOT NULL,
    scheduled_for DATETIME,  -- when the alarm was set to fire
    enabled BOOLEAN
);
CREATE INDEX idx_alarm_history_at ON alarm_history(action_at);
```

**Acceptance:**
- Calling `set_alarm` (existing command) produces an `action='set'` row.
- Receiving a `RunAlarm` event (via Feature 1's ingestion) produces an `action='fired'` row.
- Calling `disable_alarm` produces an `action='cleared'` row.
- Unit tests cover each of the three write paths.

### Feature 4: Wear-time tracking

**Problem:** The `skin_contact` field is stored per-sample but unused. Without wear-time, every metric's denominator is wrong. Users can't distinguish "my HR data has gaps because I took the strap off" from "my HR data has gaps because of a bug."

**Solution:** New `wear_periods` table populated by a new algorithm that fuses two signals:
- **Primary:** WristOn/WristOff events from the `events` table (once Feature 1 is live). These are authoritative.
- **Fallback:** Per-sample `skin_contact` runs from `heart_rate.sensor_data`. Less precise but always available.

Algorithm runs as a new pipeline step after `detect_sleeps` and before the new sleep-staging (if present).

**Schema:**
```sql
CREATE TABLE wear_periods (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    start DATETIME NOT NULL,
    end DATETIME NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('events', 'skin_contact', 'fused')),
    duration_minutes REAL NOT NULL,
    CHECK(end > start)
);
CREATE INDEX idx_wear_periods_start ON wear_periods(start);
```

**Algorithm:**
```
1. Query events table for WristOn/WristOff in [sync_start, sync_end]
   → primary periods: bracket WristOn→WristOff pairs
   → source='events'

2. For gaps ≥5 min between primary periods, fall back to skin_contact=1 runs
   in heart_rate.sensor_data. Merge runs separated by <1 min.
   Require ≥5 min minimum duration to count.
   → source='skin_contact'

3. Merge adjacent/overlapping periods across sources.
   If an events-derived and a skin_contact-derived period overlap,
   prefer events boundaries, tag source='fused'.

4. Handle edge cases:
   - If a WristOn has no matching WristOff before sync_end: end = last heart_rate
     row timestamp while skin_contact was 1.
   - If a WristOff has no preceding WristOn: start = earliest heart_rate row
     timestamp while skin_contact was 1 in that vicinity.
```

**Acceptance:**
- For a synthetic day with alternating WristOn/Off events, `wear_periods` contains the correct bracketed intervals.
- For a synthetic day with only `skin_contact` transitions (no events), the fallback produces reasonable periods.
- Total wear minutes in a day equals the count of on-wrist heart_rate samples ÷ 60 (±10% tolerance for edge effects).
- Existing sleep detection and other pipeline steps unaffected.

### Feature 5: Daytime HRV samples

**Problem:** HRV is only computed during sleep, embedded in sleep cycle creation. But RR intervals exist 24/7. No way to track pre/post workout recovery, stress response, or waking HRV trends.

**Solution:** New `hrv_samples` table. Post-sync algorithm computes RMSSD in 5-minute non-overlapping windows across waking hours, filtering aggressively for quality.

**Schema:**
```sql
CREATE TABLE hrv_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    window_start DATETIME NOT NULL,
    window_end DATETIME NOT NULL,
    rmssd REAL NOT NULL,
    sdnn REAL,
    mean_hr REAL NOT NULL,
    rr_count INTEGER NOT NULL,
    stillness_ratio REAL NOT NULL,
    context TEXT NOT NULL CHECK(context IN ('resting', 'active', 'mixed'))
);
CREATE INDEX idx_hrv_samples_start ON hrv_samples(window_start);
```

**Algorithm:**
```
For each 5-minute window [t, t+5min) during waking hours (exclude sleep windows
from sleep_cycles):

1. Gate (all must pass, else skip window):
   - window has ≥3 minutes of heart_rate coverage
   - combined RR interval count ≥ 50
   - signal_quality mean ≥ <threshold>  (use median value observed in user's
     data as the threshold; if no baseline, use a conservative fixed default)
   - fewer than 30% of RRs flagged as ectopic (successive-difference >20% of
     preceding interval)

2. Compute:
   - rmssd = sqrt(mean((RR[i+1] - RR[i])^2))
   - sdnn = stddev(RR)
   - mean_hr = mean(bpm across rows in window)
   - stillness_ratio = fraction of rows where ||delta accel_gravity|| < 0.01g

3. Classify context:
   - mean_hr < user_resting_hr + 10 AND stillness_ratio > 0.9 → 'resting'
   - mean_hr > user_resting_hr + 30 OR stillness_ratio < 0.5  → 'active'
   - else                                                      → 'mixed'

4. Reject windows where mean_hr > 100 and context != 'active' (bad signal).

5. Insert row.
```

`user_resting_hr` is already available in the pipeline (from latest sleep's min_bpm per the TECHNICAL_REFERENCE). Default to 60 if not yet computed.

**Acceptance:**
- For a synthetic 24-hour RR series with known RMSSD values per 5-min window, the computed RMSSDs match to 3 decimal places.
- A resting window (low HR, high stillness) is classified as `'resting'`.
- An active window (high HR, high motion) is classified as `'active'`.
- Windows overlapping with `sleep_cycles` ranges are NOT included (those are handled by existing sleep-HRV).
- Pipeline runtime for one day of data (≈86400 seconds, 288 five-minute windows) completes in <3 seconds.

### Feature 6: Sync log

**Problem:** The snapshot tracks `last_sync_at` and `last_sync_attempt_at` but no history. When sync fails, users and developers have no trail.

**Solution:** New `sync_log` table. One row per sync attempt, capturing outcome and metrics.

**Schema:**
```sql
CREATE TABLE sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_started_at DATETIME NOT NULL,
    attempt_ended_at DATETIME,
    outcome TEXT NOT NULL CHECK(outcome IN ('success', 'error', 'cancelled', 'timeout', 'in_progress')),
    error_message TEXT,
    heart_rate_rows_added INTEGER DEFAULT 0,
    packets_downloaded INTEGER DEFAULT 0,
    sleep_cycles_created INTEGER DEFAULT 0,
    trigger TEXT  -- 'manual', 'scheduler', 'presence', 'charging_off', etc.
);
CREATE INDEX idx_sync_log_started ON sync_log(attempt_started_at);
```

**Integration:** Wrap the existing sync entry point. Write an `'in_progress'` row at start; update to final outcome at end. On error, capture the error message. Don't break existing sync behavior if the logging itself fails (catch errors in the logging path, log-warn, continue).

**Acceptance:**
- After a successful sync, exactly one new `outcome='success'` row exists with accurate counts.
- A sync that errors produces exactly one `outcome='error'` row with the error message.
- Sync logging failure does NOT cause sync failure (unit test: inject a write error into the logger, confirm sync still completes).

### Feature 7: Basic activity classification

**Problem:** Gyroscope data (`gyr_x/y/z_dps`) and high-rate accelerometer data in `imu_data` are completely unused. The `heart_rate.activity` column is firmware-assigned and coarse.

**Solution:** New `activity_samples` table with rule-based classification in 1-minute windows during waking, on-wrist periods. Phase 1 targets four coarse classes — this is deliberately simple.

**Schema:**
```sql
CREATE TABLE activity_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    window_start DATETIME NOT NULL,
    window_end DATETIME NOT NULL,
    classification TEXT NOT NULL CHECK(classification IN ('sedentary', 'light', 'moderate', 'vigorous', 'unknown')),
    accel_magnitude_mean REAL,
    accel_magnitude_std REAL,
    gyro_magnitude_mean REAL,
    dominant_frequency_hz REAL,
    mean_hr REAL
);
CREATE INDEX idx_activity_samples_start ON activity_samples(window_start);
```

**Algorithm (1-minute windows, waking + on-wrist only):**
```
For each window:

1. Extract from imu_data arrays in the window's heart_rate rows:
   - accel_magnitude_mean = mean(sqrt(ax² + ay² + az²))
   - accel_magnitude_std  = stddev of same
   - gyro_magnitude_mean  = mean(sqrt(gx² + gy² + gz²))
   - dominant_frequency_hz = peak frequency of accel magnitude FFT in 0.5-10 Hz band

2. Classify:
   - accel_magnitude_std < 0.05 AND gyro_magnitude_mean < 10:
       → 'sedentary'  (desk work, reading, driving)
   - accel_magnitude_std < 0.15 AND dominant_frequency_hz < 2.0:
       → 'light'      (walking around, standing chores)
   - dominant_frequency_hz between 2.0-3.5 AND accel_magnitude_std > 0.15:
       → 'moderate'   (brisk walking, most cardio)
   - dominant_frequency_hz > 3.0 OR accel_magnitude_std > 0.4:
       → 'vigorous'   (running, HIIT, cycling at intensity)
   - else:
       → 'unknown'

3. Skip windows where no IMU data is present (mark as NULL row? No — skip).
```

**Note on thresholds:** These are conservative initial values from general human-activity-recognition literature, not validated against WHOOP-specific data. They will almost certainly need tuning after real-user data is observed. Mark this feature's classifier as `classifier_version = 'rule-v0'` in a code constant, so future tuning can be explicit.

**Acceptance:**
- For 60 seconds of synthetic still IMU data (accel magnitude ≈ 1.0, low variance), classification = `'sedentary'`.
- For 60 seconds of synthetic running-like IMU data (accel magnitude varies at 2.5 Hz with high amplitude), classification = `'moderate'` or `'vigorous'`.
- Windows overlapping with `sleep_cycles` or outside `wear_periods` are skipped.
- Classification runs in <2 seconds per day of data.

## 5. Schema summary

Five new tables total:
- `events`
- `device_info`
- `alarm_history`
- `wear_periods`
- `hrv_samples`
- `sync_log`
- `activity_samples`

No new columns on existing tables. No modifications to existing tables. Zero schema changes are destructive.

## 6. Pipeline integration

The existing sync pipeline (per `TECHNICAL_REFERENCE.md`: `download history → detect sleeps → detect events → calculate stress/spo2/skin_temp`) gets these new steps appended:

```
existing: download history
existing: detect sleeps
existing: detect events (firmware-assigned activity classification)
existing: calculate stress / spo2 / skin_temp
NEW:      update wear_periods      [Feature 4]
NEW:      compute daytime hrv      [Feature 5]
NEW:      classify activities      [Feature 7]
```

Feature 1 (events ingestion) happens in the BLE packet handler, not the sync pipeline — it's always-on.
Features 2, 3, 6 are cross-cutting (connect/alarm/sync hooks respectively) and not part of the sync pipeline flow.

Each new step is wrapped in isolating error handling: any step that panics or errors is logged but does not prevent subsequent steps from running.

## 7. Snapshot extensions (backend only)

Extend the `Snapshot` struct returned by `get_snapshot` with the following, but do not break existing fields:

```rust
// Add to Snapshot
pub struct Snapshot {
    // ... existing fields ...
    pub recent_events: Vec<EventSummary>,           // last 50 events
    pub device_info: Option<DeviceInfoSummary>,     // latest device_info row
    pub alarm_history: Vec<AlarmSummary>,           // last 10 alarms
    pub today_wear_minutes: f64,                    // sum over today
    pub today_hrv_samples: Vec<HrvSampleSummary>,   // today's windows
    pub recent_sync_log: Vec<SyncLogEntry>,         // last 10 attempts
    pub today_activity_breakdown: ActivityBreakdown, // minutes per class
}
```

No UI work. Frontend is a separate PR.

## 8. Acceptance criteria (batch-level)

For the batch to be considered shippable:

1. All 7 features land as independent commits. Any single feature skipped must be documented in a `SKIPPED.md` file with reason.
2. All existing tests still pass (no regressions).
3. New unit tests for each feature pass.
4. `cargo clippy` clean (no new warnings introduced).
5. Running the full sync pipeline on an existing dev database produces no panics and populates all applicable new tables.
6. A `MIGRATION_SUMMARY.md` file at the repo root lists: schema changes, new dependencies (if any), behavioral changes, and any known limitations.

## 9. Risks and mitigations

| Risk | Mitigation |
|---|---|
| A feature has a bug that breaks existing sync | Each new pipeline step is wrapped in its own try/catch; failure logs and continues rather than halts |
| Builder agent can't figure out the ORM (sea-orm, etc.) for a new migration | Agent is instructed to inspect existing migrations first and match the style; if it can't, skip the feature and document |
| Event ingestion duplicates rows on sync retry | Events table's uniqueness is (timestamp, event_id) — add a UNIQUE constraint; use `INSERT OR IGNORE` |
| Daytime HRV thresholds produce too-sparse or too-dense output | Acceptance criteria specifies expected count range; if wildly off, threshold is wrong and feature gets flagged for review |
| Activity classifier thresholds are wrong for the specific user | Documented as `rule-v0`; flagged in `MIGRATION_SUMMARY.md` as "needs tuning after first week of observation" |
| Wear-time fallback double-counts with events-derived periods | The merge step in the algorithm handles overlaps explicitly; unit test covers this case |
| The UUID routing one-line fix accidentally regresses something | The change is purely additive (adding a match arm); existing arms are untouched |

## 10. Open questions (best-guess defaults provided for autonomy)

1. **Events uniqueness key:** Use `(timestamp, event_id)` as UNIQUE. If a real event repeats at the exact same millisecond (unlikely), we lose it — acceptable tradeoff for idempotency on sync retries.
2. **HRV window overlap handling:** Non-overlapping 5-minute windows aligned to `:00, :05, :10, ...` local time. Simpler than sliding windows.
3. **Activity window size:** 1 minute. Shorter windows are noisier, longer windows blur activity boundaries.
4. **Wear-time minimum duration:** 5 minutes. Shorter on-wrist blips are usually "picked up and put down" artifacts, not real wear.
5. **Sync log retention:** No auto-cleanup in Phase 1. Table will grow indefinitely. Add TODO for retention policy later.
6. **Device info deduplication:** Write a new row on every initialize, even if version strings are identical to the last row. Makes it easy to see connection history. Size is tiny.

## 11. References

Prior context this PRD builds on:
- `TECHNICAL_REFERENCE.md` — baseline system documentation
- The BLE audit (in prior conversation): identified the EVENTS_FROM_STRAP UUID routing gap, the no-op event handlers, the missing command response parsers, and the missing schema coverage
- `PRD_sleep_staging.md` — adjacent PRD; Feature 5 (daytime HRV) is designed to NOT overlap with that PRD's sleep-HRV (which is overnight-only via the existing algorithm)
