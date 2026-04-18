# Project Status

Living snapshot of where OpenWhoop is. Update this when a chunk of work
lands or a new initiative starts. If this file is out of date, `git log`
is authoritative — but the point of this file is so you don't need to
read 50 commits to pick up where you left off.

**Last updated:** 2026-04-17

## Where we are

### Shipped on `master`

- **Sleep staging Phase 1 (rule-v2)** — Wake/Light/Deep/REM classifier with within-night HR + RMSSD percentile gates, full architecture metrics (TIB, TST, latency, WASO, efficiency, wake events, cycles), respiratory rate, skin-temp deviation, sleep need/debt, composite Sleep Performance Score with all 5 sub-score inputs wired. See [docs/SLEEP_STAGING.md](docs/SLEEP_STAGING.md).
- **Quick-wins batch (7 features)** — events ingestion, device_info, alarm_history, wear_periods, daytime HRV samples, sync_log, activity classifier (rule-v0). See [docs/prds/quick_wins.md](docs/prds/quick_wins.md) for the PRD and [docs/sessions/2026-04-17-quick-wins/](docs/sessions/2026-04-17-quick-wins/) for the build log.
- **OpenWhoop Lab** — Marimo dev dashboard + `openwhoop note` CLI for agent↔dev async comms. See [lab/README.md](lab/README.md).
- **Battery log** — `battery_log` table + entity, powering the tray's battery prediction.
- **Activity fixes** — cap workout duration `[10m, 4h]`, stop writing Active-wake runs as workouts.

### Quick stats

- 267 tests, `cargo clippy --workspace --all-targets` clean
- 22 migrations total (12 baseline → + battery_log + sleep_staging + 7 quick-wins + dev_notes)
- Workspace crates: `openwhoop`, `openwhoop-algos`, `openwhoop-codec`, `openwhoop-db`, `openwhoop-entities`, `openwhoop-migration`, `openwhoop-types`

### Branches

| Branch | Purpose | State |
|---|---|---|
| `master` | mainline | active |
| `integration/whoop-tray` | what the tray submodule points at | auto-merged from master as features land |
| `fix/download-history-exit` | upstream PR #19 on `bWanShiTong/openwhoop` | open |
| `fix/calculate-loops` | upstream PR #21 on `bWanShiTong/openwhoop` | open |

## What's next

Rough priority order. The first item is the biggest unlock — a lot of
shipped backend data is invisible to the user until the tray catches up.

1. **Tray UI integration** — plan in [docs/TRAY_INTEGRATION_PLAN.md](docs/TRAY_INTEGRATION_PLAN.md). Phase 1 (hypnogram + score components in the Latest Sleep card) is the highest-value first PR.
2. **Sleep staging follow-ups (open findings, deferred)**:
   - Rule-v2 Deep has a structural ~3–5% ceiling from the 5-gate AND-conjunction. Next moves: soft-score Deep, or drop one feature gate (RMSSD/HF overlap). See [SLEEP_STAGING.md §6](docs/SLEEP_STAGING.md) "Structural limit of percentile-AND conjunction".
   - Wake fall-through over-fires on restless-Light clusters (no HR confirmation on Rule 5). Proposed fix sketched. Both deferred pending ground-truth labels or Phase 2 ML.
3. **Lab dashboard tier-2** — in-dashboard note resolve, threshold sliders, before/after diff views. [docs/DEV_DASHBOARD_CONCEPT.md](docs/DEV_DASHBOARD_CONCEPT.md).
4. **Sleep staging Phase 2 (ML)** — ONNX LightGBM on MESA. Feature names already aligned with NeuroKit2 for drop-in. Not started.
5. **Upstream PRs** — chase #19 and #21 through review on `bWanShiTong/openwhoop`.

## How to pick up from here

1. `git log master --oneline -20` for the recent state.
2. `cargo test --workspace && cargo clippy --workspace --all-targets` should be clean.
3. Spin up the lab dashboard: `make lab` (opens localhost:2718). It reads the tray's live DB by default.
4. Check the inbox panel in the lab for any unresolved dev notes from agent sessions.
5. This file + [docs/SLEEP_STAGING.md](docs/SLEEP_STAGING.md) + [docs/TRAY_INTEGRATION_PLAN.md](docs/TRAY_INTEGRATION_PLAN.md) are the three docs to skim before starting new work.

## Related repos

- **Tray app** — `~/code/openwhoop-tray` (local-only, no remote). Tauri 2 macOS menu bar app wrapping this crate via git submodule at `vendor/openwhoop` on the `integration/whoop-tray` branch.
