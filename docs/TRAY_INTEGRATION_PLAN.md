# Tray Integration Plan

**Goal:** surface all the new backend data (sleep staging + 7 quick-wins features) in the `openwhoop-tray` UI. The backend is done; this is purely the frontend contract + React/Tauri work.

## Current state

**Backend (committed on `master`, merged to `integration/whoop-tray`):**
- Sleep staging: hypnogram, stages, efficiency, WASO, cycles, respiratory rate, skin-temp deviation, sleep need/debt, composite performance score, score components.
- Quick-wins: events (WristOn/Off, charging, alarms, double-tap), device_info (firmware versions), alarm_history, wear_periods, daytime HRV samples, sync_log, activity_samples (sedentary/light/moderate/vigorous).

**Tray frontend (from memory — `openwhoop-tray/src/App.tsx`):**
- Dashboard window with cards: Latest Sleep (score + BPM/HRV min/avg/max), Today's BPM sparkline, Recent Activities, Last 7 Days summary, Battery.
- Menu bar: battery %, last sync, presence, sync button, "Buzz strap".
- Backend already wraps `stage_sleep()` into the sync pipeline (local commit `6bf8cb6`).

**Gap:** only `sleep_cycles.score` (= new `performance_score`) visibly changed. All the new data is in tables the frontend never queries.

## Phase 1 — highest-value, tightest-scope (first PR)

These give the biggest perceived jump for the least work.

### 1.1 New Tauri command: `get_sleep_snapshot`
Backend already has `openwhoop::sleep_staging::latest_sleep_snapshot(&db) -> Option<SleepSnapshot>`. Just wrap it.

```rust
// src-tauri/src/lib.rs
#[tauri::command]
async fn get_sleep_snapshot(state: State<'_, AppState>) -> Result<Option<SleepSnapshot>, String> {
    let db = state.db.lock().await;
    openwhoop::sleep_staging::latest_sleep_snapshot(&db)
        .await
        .map_err(|e| e.to_string())
}
```

Register in the `invoke_handler![]` macro.

### 1.2 Extend the "Latest Sleep" card
Replace the current numeric-only view with:

- **Hypnogram strip**: horizontal bar ~40px tall, color-coded by stage (Wake/Light/Deep/REM/Unknown). Data from `snapshot.hypnogram`. Use `recharts` (already a common React chart lib) or roll a simple SVG.
- **Stage breakdown**: small horizontal stacked bar showing Awake/Light/Deep/REM minutes with hover tooltips. Data from `snapshot.stages`.
- **Score components donut/spider**: 5-point radar or donut showing `score_components.{sufficiency, efficiency, restorative, consistency, sleep_stress}`. Makes the composite score legible.
- **Architecture row**: efficiency %, latency min, WASO min, cycle count, wake events. Just text, one line each.
- **Respiratory + skin temp line**: "14.2 bpm · +0.2°C vs baseline". Tooltip if deviation > 0.5°C.

Rough layout:
```
┌─ Last night ──────── score: 68 ─┐
│ [HYPNOGRAM BAR — 0h to 7h]      │
│ Awake  5m │ Light 249m │ Deep..│  <- stage bar
│ [Score components radar]         │
│ Efficiency 92.1% · Latency 15m  │
│ WASO 7m · 3 cycles · 2 wakes    │
│ Respiratory 14.2 bpm · skin +0.2│
└─────────────────────────────────┘
```

### 1.3 Calibrating-user disclaimer
PRD for sleep staging says to flag the first 14 nights. Add a subtle "Calibrating — score will adapt after ~2 weeks" badge when `snapshot.classifier_version is Some("rule-v1")` AND the baseline's `window_nights < 14`. Requires exposing baseline info in the snapshot or a separate `get_user_baseline` command.

**Estimated effort:** 4-6 hours of TypeScript + a few lines of Rust.

## Phase 2 — today's daytime data

Adds the non-sleep quick-wins.

### 2.1 `get_daily_snapshot` command
New backend function aggregating:
- Today's HRV samples (from `hrv_samples`)
- Today's activity breakdown (from `activity_samples` — minutes per class)
- Today's wear time total (`wear_minutes_in_range`)
- Last 5 events (from `events`)
- Latest device_info firmware string
- Last 3 alarm_history entries
- Last 3 sync_log entries

Exact struct shape:
```rust
pub struct DailySnapshot {
    pub today_wear_minutes: f64,
    pub today_hrv_samples: Vec<HrvSampleLite>,  // time, rmssd, context
    pub today_activity_breakdown: ActivityBreakdown, // minutes per class
    pub recent_events: Vec<EventLite>,
    pub device_info: Option<DeviceInfoLite>,
    pub alarm_history: Vec<AlarmLite>,
    pub recent_sync_log: Vec<SyncLogLite>,
}
```

### 2.2 New "Today" card
- Wear time ring (e.g. 11h 20m of 16h awake = 71%)
- Activity stacked bar (minutes sedentary/light/moderate/vigorous)
- HRV day-trend line (last 8 HRV samples, color-coded by context)
- Latest events as a small list: "09:12 WristOn · 11:45 ChargingOn · …"

### 2.3 Firmware + sync status line in the menu bar
The menu bar's existing status text adds: firmware version (tiny text), last sync outcome + error if any (from most recent `sync_log`).

**Estimated effort:** 6-10 hours.

## Phase 3 — history + trends

### 3.1 History page
A new window (or tab) showing:
- Last 14 nights: thumbnail hypnograms, score trend line, stage composition over time
- HRV trend (per-day avg daytime RMSSD + sleep RMSSD)
- Weekly wear-time
- Activity minutes per class per day

### 3.2 Sync log drawer
Small dev-adjacent feature: click the sync button, see a timeline of past attempts with outcomes + error messages. Helps debug strap issues without touching the DB.

### 3.3 Events/alarm log page
Searchable/filterable list of all events and alarm actions. Mostly for curiosity/debugging.

**Estimated effort:** 10-20 hours. Optional; skip if Phase 1+2 land nicely.

## Data-contract summary

New Tauri commands to add in the tray's `src-tauri/src/lib.rs`:

| Command | Returns | Phase |
|---|---|---|
| `get_sleep_snapshot` | `Option<SleepSnapshot>` | 1 |
| `get_daily_snapshot` | `DailySnapshot` | 2 |
| `get_sleep_history` | `Vec<SleepHistoryEntry>` (last 14 nights) | 3 |
| `get_events_page` | paginated events | 3 |

Each takes no args (uses current-user DB from app state).

## Submodule bump plan

1. On `openwhoop` fork: current `integration/whoop-tray` tip has all backend work.
2. In `openwhoop-tray` repo:
   ```sh
   cd vendor/openwhoop
   git fetch origin
   git checkout integration/whoop-tray
   git pull
   cd ../..
   git add vendor/openwhoop
   git commit -m "Bump openwhoop: quick-wins batch"
   ```
3. Add the new Tauri commands in `src-tauri/src/lib.rs`.
4. Build+run once to confirm migrations apply and existing UI doesn't regress.
5. Then start Phase 1 React work.

## Testing

- **Unit-level:** each new Tauri command gets a minimal test that calls it against a seeded in-memory DB and asserts shape.
- **Visual smoke:** run the tray locally after a sync, eyeball each new UI element with real data.
- **No regressions:** existing "Latest Sleep" and "Recent Activities" cards must still render (PRD principle — preserve existing behavior).

## Recommended order to action

1. Bump submodule, verify tray still builds and runs.
2. Phase 1.1 (Tauri command) — ~30 min.
3. Phase 1.2 (hypnogram + stages in Latest Sleep) — few hours.
4. Phase 1.3 (calibrating badge) — 30 min.
5. Land Phase 1 as a PR/commit. Live with it for a few days.
6. Phase 2 when you have appetite.
7. Phase 3 only if you find you want it.

## Known caveats

- The tray's frontend hasn't been touched in this session; I don't know its exact state-management pattern. Phase 1 assumes the existing pattern extends; adjust if it uses Zustand/Redux/Context differently.
- Performance: the snapshot query runs on every dashboard open. If the epoch count gets large (one full night = ~960 epochs), consider caching the latest snapshot in the Rust app state and only recomputing on sync.
- The tray repo has no remote (per project memory) — all PRs are local-only for now.
