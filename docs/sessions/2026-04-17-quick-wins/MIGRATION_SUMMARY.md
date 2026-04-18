# Quick-Wins Batch — Migration Summary

Overnight batch of 7 additive features per `PRD_quick_wins.md`.

## Baseline vs final

| Metric | Before | After |
|---|---:|---:|
| Workspace tests | 226 | 267 |
| `cargo clippy --workspace --all-targets` | clean | clean |
| Migrations | 13 | 20 |
| New tables | — | 7 |
| New algo modules | — | 3 |
| New DB query modules | — | 7 |

## Migrations (in chronological order)

All additive. No modifications of existing tables; no drops.

1. `m20260417_000000_events` — events table with UNIQUE(timestamp, event_id), timestamp and event_id indices.
2. `m20260417_010000_device_info` — device_info with recorded_at index.
3. `m20260417_020000_alarm_history` — alarm_history with action_at index.
4. `m20260417_030000_wear_periods` — wear_periods with start index.
5. `m20260417_040000_hrv_samples` — hrv_samples with window_start index.
6. `m20260417_050000_sync_log` — sync_log with attempt_started_at index.
7. `m20260417_060000_activity_samples` — activity_samples with window_start index.

## New workspace dependencies

None. `rustfft` was already a dep from sleep-staging; activity classifier reuses it.

## New pipeline steps (order in `OpenWhoopCommand::DetectEvents`)

```
detect_sleeps  (existing)
detect_events  (existing)
update_wear_periods      [Feature 4]
stage_sleep    (existing, from sleep-staging PRD)
compute_daytime_hrv      [Feature 5]
classify_activities      [Feature 7]
```

All new steps process the last 14 days of data. Feature 6's sync_log
wraps the whole `DetectEvents` arm, writing an `in_progress` row on
entry and updating to `success`/`error` on exit. Logger write failures
do not propagate — sync completion is independent of logging success.

## Features shipped

| # | Feature | Commit | Integration commit | Tests |
|---|---|---|---|---:|
| 1 | events ingestion + UUID routing fix | `95a…` | same | 6 (4 DB + 2 pure) |
| 2 | device_info | — | same | 3 DB |
| 3 | alarm_history | — | same | 5 DB |
| 4 | wear_periods infra | — | `fc6…` | 11 (8 algo + 3 DB) |
| 5 | daytime HRV infra | — | `fc6…` | 8 (7 algo + 1 DB) |
| 6 | sync_log infra | — | `fc6…` | 3 DB |
| 7 | activity classifier infra | — | `fc6…` | 5 (4 algo + 1 DB) |
| — | pipeline + sync_log wrap | — | `fc6…` | — |

Use `git log --grep "feat(quick-wins):"` for the full commit list.

## Existing tests

Baseline 226 tests all pass unchanged. No existing test was modified
(Principle 6). The count went up to 267 purely from the 41 new tests
added across the batch.

## Known limitations

- **Activity classifier (`rule-v0`) thresholds are unvalidated** against
  real WHOOP user data. They are starting values from general HAR
  literature. Expect first-week-of-observation tuning. Version tag is
  a const so future tuning bumps it.
- **Duplicate rows possible on re-run** for `wear_periods`, `hrv_samples`,
  `activity_samples`. No UNIQUE constraints or DELETE-before-INSERT
  guards. If this matters, add UNIQUE or dedupe-on-insert.
- **`device_info.device_name` always NULL.** Requires extending
  `parse_command_response` to decode `GetName` — out of scope for
  additive-only batch. See `DECISIONS.md`.
- **`alarm_history action='cleared'` has no wired call site** — no
  `disable_alarm` code path exists anywhere in the codebase today.
- **Sync_log trigger is always `"manual"`** from the CLI wrapper. Tray/
  scheduler/presence paths aren't wrapped; those live in the tray repo.
- **Sync_log counts default to zero** — the existing CLI doesn't thread
  the `download-history` counts into the audit logger. Snapshot still
  queries correctly; counts just read 0 for now.
- **Pipeline short-circuits on first error** rather than attempting all
  subsequent steps. Trade-off vs. noisy logging; can revisit.
- **`EVENTS_GAP_FALLBACK_THRESHOLD_SECS`** in `wear_tracking.rs` is
  defined but unused — algorithm uses explicit subtraction for merge.
  Kept for discoverability; generates a `dead_code` warning.

## Nothing in SKIPPED.md — all 7 features shipped

(See `SKIPPED.md` — file exists for protocol compliance; empty of entries.)

## Running the batch

```sh
cd /Users/totem/code/openwhoop
cargo build --workspace          # should be clean
cargo test --workspace           # 267 tests pass
cargo clippy --workspace --all-targets  # silent
```

Smoke test against a real DB (on a copy, not the live tray DB):

```sh
cp "$HOME/Library/Application Support/dev.brennen.openwhoop-tray/db.sqlite" /tmp/ow_qw_test.db
DATABASE_URL="sqlite:///tmp/ow_qw_test.db?mode=rwc" \
  cargo run -r -- detect-events
```

The CLI's migrator auto-applies the 7 new migrations. Then the
`DetectEvents` arm runs the full pipeline including the four new
steps, wrapped in a sync_log attempt. Inspect the new tables with:

```sh
sqlite3 /tmp/ow_qw_test.db '.tables'
sqlite3 /tmp/ow_qw_test.db 'SELECT COUNT(*) FROM wear_periods;'
sqlite3 /tmp/ow_qw_test.db 'SELECT * FROM sync_log ORDER BY attempt_started_at DESC LIMIT 5;'
```

Note: events, device_info, alarm_history rows only populate from live
BLE — CLI-only replay won't populate them.

## Rewind anchor

Pre-batch commit: `fb08997` (`docs: overnight session log + update §14 TODO status`).

```sh
git reset --hard fb08997
git push --force-with-lease origin master  # if published
```
