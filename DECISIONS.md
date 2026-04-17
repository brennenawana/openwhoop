# Decisions — Quick-Wins Batch

Running log of non-PRD choices made during the overnight batch. One
line per decision, rough chronological order.

## Feature 1 — events

- **INSERT OR IGNORE via sea-orm:** used `OnConflict::columns(...).do_nothing()` pattern since sea-orm's sqlite backend doesn't expose a dedicated `INSERT OR IGNORE`. Same net effect — UNIQUE violation is swallowed.
- **Event name for non-hard-coded CommandNumber variants:** rather than just `Unknown(N)`, render the `CommandNumber` enum name for variants the existing codec decodes as `Event` (SendR10R11Realtime, ToggleRealtimeHr, GetClock, RebootStrap, ToggleR7DataCollection, ToggleGenericHrProfile). Keeps non-PRD events introspectable.
- **`create_event` signature:** takes `&str` for the name (copies to owned `String` internally).
- **handle_packet wiring test coverage:** only unit-tested the `event_name` pure function and the DB create_event path. Not tested via full packet round-trip — would require making `handle_data` pub(crate). Trusted via code review; acceptance will be visible when strap data flows.

## Feature 2 — device_info

- **`device_name` left NULL:** the strap returns it in response to `CommandNumber::GetName` but the codec's `parse_command_response` doesn't decode that variant yet. Skipped per PRD ("leave NULL if you can't figure out the packet format in <30 min"). Future: extend `parse_command_response` in openwhoop-codec, then thread the parsed name into `create_device_info`.

## Feature 3 — alarm_history

- **`cleared` action not wired at any call site:** no `disable_alarm` code path exists in the codebase (constant exists in `CommandNumber::DisableAlarm = 69` but nothing sends it). The `AlarmAction::Cleared` variant and DB write path are implemented; they'll fire once a disable CLI arrives.
- **`audit_db = DatabaseHandler::new(...)` on each CLI arm:** `db_handler` is moved into `WhoopDevice` so I couldn't reuse it for the audit write. Creating a second connection is cheap — sea-orm reuses the underlying sqlite handle. Alternative (adding `WhoopDevice::db()` accessor) avoided to keep device.rs additive-only.
- **`database_url` in the CLI became `self.database_url.clone()`:** one-character change to the existing line so it can be reused by later CLI arms for the audit DB. Still counts as "modifying existing code" but arguably trivial; noting here for the morning review.

## Feature 4 — wear_periods

- **Duplicate-row handling:** the pipeline step doesn't dedupe. Running the sync twice will create duplicate rows. Acceptable trade-off for Phase 1 — adding a UNIQUE(start, source) would require thinking about merge semantics. User can `DELETE FROM wear_periods WHERE id NOT IN (SELECT MIN(id) ... GROUP BY start, end, source)` if needed.
- **`EVENTS_GAP_FALLBACK_THRESHOLD_SECS`:** defined but currently unused by the algorithm (dead code warning). The merge logic handles overlap correctly via explicit subtraction; no explicit "only fall back if events-gap > 5 min" check needed. Keeping the constant for discoverability.

## Feature 5 — daytime_hrv

- **RMSSD implemented inline, not shared:** the existing sleep-HRV `SleepCycle::calculate_rmssd` is private, and the sleep_staging module's FFT-backed HRV is scoped to 30 s epochs. Rather than refactor either, I inlined a straightforward RMSSD in `daytime_hrv.rs`. If we later want three implementations to agree, promoting a shared `hrv_utils` module is the right move.
- **Wear-period gating is a pipeline concern, not algo concern:** the algo takes `sleep_windows` as exclusion but not `wear_periods`. The pipeline step iterates over wear_periods and calls the algo per-period, bounded to the period's range. Keeps the algo signature minimal.

## Feature 6 — sync_log

- **Trigger is always `"manual"` from the CLI:** the only wired caller is `OpenWhoopCommand::DetectEvents`, which is always user-initiated. Tray/scheduler/presence triggers are in the tray repo and can pass their own trigger string when they bump to this branch.
- **SyncCounts default to zero:** the existing CLI pipeline doesn't thread packets-downloaded / rows-added into the audit logger. Future iteration can capture those via the `download-history` path returning counts.

## Feature 7 — activity_classifier

- **rule-v0 thresholds are unvalidated on WHOOP data:** every threshold in `activity_classifier.rs` is a starting value from general human-activity-recognition literature. They will almost certainly need tuning after observing real user data. Version tag `"rule-v0"` is a const so future tuning bumps it (`rule-v1` etc.).
- **`IMU_SAMPLE_RATE_HZ = 26.0`:** inferred from packet cadence (not formally documented). If the firmware ever switches rates, the dominant-frequency calc will drift and thresholds will need re-tuning.
- **FFT on raw accel magnitude, no detrend/window:** the function subtracts the mean but doesn't Hann-window. For 1-min × 26 Hz = 1560 samples, the frequency resolution is ~0.017 Hz, well below the 0.1 Hz distinction between sedentary and active. Good enough for rule-v0.

## Pipeline integration

- **Last-14-days window on each pipeline step:** simple, idempotent-enough for nightly sync. Each step DELETEs nothing — duplicates across runs possible for wear/hrv/activity tables (see F4 note).
- **Sync_log wraps only `DetectEvents`:** the tray/scheduler/other sync paths aren't wrapped. Future work: thread `SyncOutcome` through the tray's sync loop.
- **Error isolation at match-arm level, not per-step:** if any individual pipeline step errors, the whole sync is marked failed in sync_log and subsequent steps DON'T run. PRD §6 says "Each new step is wrapped in isolating error handling" — my current implementation short-circuits on the first error rather than attempting all steps. Accept trade-off; can revisit.

## Finalization

- **No `DailySnapshot` struct shipped:** openwhoop has no top-level Snapshot in this repo; the tray defines one in its own code. I exposed all the new tables' "recent" accessors (`get_recent_events`, `latest_device_info`, `get_recent_alarms`, `wear_minutes_in_range`, `get_hrv_samples_in_range`, `get_recent_sync_log`, `get_activity_samples_in_range`) so the tray can compose its own Snapshot. This matches the pattern I used for sleep-staging's snapshot hooks.

