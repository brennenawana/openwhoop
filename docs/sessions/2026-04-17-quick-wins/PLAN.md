# Quick-Wins Batch — Execution Plan

**Target:** 7 additive features per `PRD_quick_wins.md`, overnight autonomous.

## Baseline (pre-batch)

- Branch: `master`
- Tip: `fb08997` (`docs: overnight session log + update §14 TODO status`)
- `cargo build --workspace`: ✅
- `cargo test --workspace`: 226 tests passing
- `cargo clippy --workspace --all-targets`: silent (0 warnings)
- Tray repo `main` at `6bf8cb6`, submodule at `7ea29af` (post first-session merge)

## Conventions I'll follow (matched from existing code)

| Concern | Choice |
|---|---|
| Migration framework | sea-orm 1.1.x |
| Migration file name | `m{YYYYMMDD}_{HHMMSS}_{snake_name}.rs` |
| Entity framework | sea-orm entities in `openwhoop-entities/src/`, registered in `lib.rs` + `prelude.rs` |
| Algo layout | flat file under `openwhoop-algos/src/`, takes `&[ParsedHistoryReading]`, returns typed score struct; tests inline via `#[cfg(test)] mod tests` |
| DB queries | methods on `DatabaseHandler` in `openwhoop-db/src/algo_impl/<feature>.rs` |
| Pipeline step | method on `OpenWhoop` struct returning `anyhow::Result<()>` |
| Error type | `anyhow::Error` at boundary, pure-domain errors via `WhoopError` internally |
| Commit prefix | `feat(quick-wins):` per protocol, numbered `(N/7)` |

## Feature order (1 → 7 as PRD specifies)

Dependencies:
- Feature 1 provides the `events` table that Features 3 (alarm fired) and 4 (WristOn/Off source) read from
- Feature 4 provides `wear_periods` that Features 5 and 7 read from
- Feature 5 and 7 both skip windows overlapping `sleep_cycles` (existing)

Running 1→7 sequentially respects all dependencies.

## Per-feature approach

### Feature 1 — events ingestion
- Migration: `events` table with UNIQUE `(timestamp, event_id)`, indices on timestamp and event_id.
- Entity: `openwhoop-entities/src/events.rs`.
- `handle_packet` in `openwhoop.rs` gets `EVENTS_FROM_STRAP` match arm (the one permitted edit). Route to same parser as DATA.
- `handle_data` arms for `WhoopData::Event`, `UnknownEvent`, `RunAlarm` get row inserts via a new `create_event` DB method (INSERT OR IGNORE).
- Hard-code event ID → name table from PRD in a `const fn event_name(id: u8) -> Option<&'static str>`.
- Tests: synthetic packet → row; unknown ID → `Unknown(N)`; duplicate → one row.

### Feature 2 — device info persistence
- Migration: `device_info` table.
- Entity.
- Persist on `initialize()` (in device.rs). Called immediately after `hello_harvard` response when version is known — actually `device.rs:164` has `if let WhoopData::VersionInfo { .. }` in `get_version`. I'll also hook into `handle_data`'s `VersionInfo` arm (which currently just logs) and add a DB insert.
- Device name capture: `CommandNumber::GetName`/`get_name` response parsing isn't implemented in `parse_command_response`. PRD says skip if unclear in <30 min. Leave NULL.

### Feature 3 — alarm history
- Migration: `alarm_history` table.
- Entity.
- `set_alarm` CLI command writes `action='set'` row.
- `disable_alarm` equivalent writes `action='cleared'`.
- `WhoopData::RunAlarm` arm writes `action='fired'` row (alongside the events row from Feature 1).
- `AlarmInfo` arm (e.g. from `get_alarm`) writes `action='queried'` row.

### Feature 4 — wear-time tracking
- Migration: `wear_periods` table.
- New algo module `openwhoop-algos/src/wear_tracking.rs`.
- Events source: `WristOn`/`WristOff` pairs from events (handled by name since PRD's hard-code is `9 → WristOn`, `10 → WristOff`).
- Fallback: `skin_contact = 1` runs from `heart_rate.sensor_data` JSON. Need a DB query that walks rows and returns contiguous runs.
- Merge logic per PRD: events-only primary, skin_contact fallback for ≥5 min gaps, prefer events boundaries, tag `'fused'` on overlap.
- Pipeline step: new `OpenWhoop::update_wear_periods` after `detect_sleeps`.

### Feature 5 — daytime HRV samples
- Migration: `hrv_samples` table.
- New algo module `openwhoop-algos/src/daytime_hrv.rs`.
- Reuse `SleepCycle::calculate_rmssd` — it already exists but is private. I'll either promote it or mirror its math in a shared utility. DECISIONS.md note.
- Aligned 5-minute windows (local time). Exclude windows overlapping sleep_cycles or outside wear_periods.
- Pipeline step: new `OpenWhoop::compute_daytime_hrv` after wear-period update.

### Feature 6 — sync log
- Migration: `sync_log` table.
- Entity.
- Wrap the existing `detect-events` CLI handler (and the tray's sync orchestrator) with begin/end hooks. A new `SyncLogger` struct captures counts.
- The `trigger` column: I can pipe 'manual' from the CLI and leave it NULL from other paths since those aren't wired.
- Key: the log write itself must be fail-safe. Wrap in try/catch; log-warn on failure.

### Feature 7 — basic activity classification
- Migration: `activity_samples` table.
- New algo module `openwhoop-algos/src/activity_classifier.rs`.
- Uses `rustfft` (already added for sleep staging — no new dep).
- 1-minute windows over heart_rate with imu_data, excluding sleep and non-wear.
- `classifier_version = "rule-v0"` const in code.
- Pipeline step: new `OpenWhoop::classify_activities` at end of pipeline.

## Snapshot extension (after all 7)

openwhoop currently has no top-level `Snapshot` struct — only `SleepSnapshot` from the sleep-staging work. PRD §7 says "extend the Snapshot struct returned by get_snapshot" but that struct lives in the tray repo.

**Decision:** create a new `DailySnapshot` struct + `get_daily_snapshot()` function in openwhoop lib that carries the quick-wins fields. The tray can wrap/flatten it into its own `Snapshot` in a separate PR.

## Rewind anchors

Same as the prior session — if any of this goes sideways, reset to:

- `openwhoop/master` at `fb08997`
- `openwhoop/integration/whoop-tray` at `7ea29af` (on origin)
- `openwhoop-tray/main` at `6bf8cb6` (local)

## What I will NOT do

- Modify any existing algorithm (sleep detection, stress, spo2, skin_temp, sleep staging, RMSSD-in-sleep, strain, consistency).
- Modify any existing test (per Principle 6). If a test breaks, investigate; fix the new code, not the test.
- Add any native-dep crate.
- Touch the frontend.
- Push to remote until each feature's unit tests pass locally.

## Skipped-feature protocol

If any feature can't land clean after two honest attempts, I write to `SKIPPED.md` and move on. The other features continue independently.
