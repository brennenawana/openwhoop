# Dev Dashboard Concept

**Status:** ideation / pre-spec. Write-up of the idea Brennen floated on 2026-04-17 for an interactive, agent-authored development dashboard. Not yet action-ready — this doc is for talking-through + refinement.

## The problem

Review of agent work today is:
- Read a long markdown session log.
- Mentally map claims ("Deep dropped from 21% to 2.7%") back to actual data in SQLite tables.
- Run SQL queries or sift commits to verify.
- Either accept the agent's framing or ignore it.

This is:
- **Passive** — static text, no interaction.
- **Effortful** — you have to re-do the data inspection the agent already did.
- **Opaque** — no way to "run the same thing with one thing different" without another agent round-trip.
- **Ephemeral** — session logs pile up, drift out of date, get forgotten.

## The vision (Brennen's framing, expanded)

An interactive, living surface where:

- The agent can write **descriptive, in-context annotations** as it works ("I changed the Deep HR gate — here's yesterday's data with the old rule vs. the new one; click to expand").
- The dev can **browse the actual data** underlying every claim without leaving the tool.
- Changes to code produce **observable shifts** in the dashboard. If I bumped `DEEP_HR_OFFSET_BPM` today, the Deep-% panel shows "today: X%, yesterday (with old constant): Y%, Δ +Z pp".
- **Open questions from the agent** show up as interactive prompts, not bullet points in a markdown file. "I have two paths here — pick one" with actual previews of each path's effect on real data.
- **Experiments are cheap**: drag a slider for `RESTORATIVE_TARGET_PCT`, see the performance score update live across the last 14 nights.

The shape is closer to a Jupyter notebook + Grafana + Linear-inbox hybrid than a traditional dashboard — the key property is that the agent writes *into* the dashboard as it works, and the dev interacts with it out-of-band.

## Concrete capabilities (what I'd build, in order)

### Tier 1 — foundation (the thing you'd use daily)

1. **Latest-data panels.** For each algorithm/feature, a live-updated panel with a canonical visualization:
   - Sleep: last 7 nights, hypnogram thumbnails + score trend
   - Daytime HRV: 14-day trend line colored by context
   - Activity: stacked bar by class per day
   - Wear time: daily + 14-day avg
   - Sync log: most recent attempts with outcome + error
   - Stress/SpO2/skin temp: sparklines

2. **Agent-authored notes.** A new `dev_notes` table. The agent writes markdown entries tagged to a feature, commit, or time range. Rendered inline next to the relevant panel. Dev can mark "seen", "follow up", "dismissed".

3. **Commit timeline.** A lane showing the last N commits with their affected tables/files. Click a commit → panels highlight the data it affected.

### Tier 2 — the agent ↔ dev async channel

4. **Questions panel.** The agent writes structured questions ("Pick one of {A, B, C}, each shows a preview"). Dev picks an answer; the agent picks it up on next session.

5. **Before/after diffs.** When the agent changes a threshold, it snapshots the affected rows before the change and runs the new code against the same data. Shows side-by-side in the dashboard. "See what this commit did to your 2026-04-17 cycle."

6. **Runbook / decision records.** The DECISIONS.md pattern, but as first-class rows in the dashboard — linkable, searchable, with context.

### Tier 3 — interactive experimentation

7. **Threshold sliders.** For things like `DEEP_HR_OFFSET_BPM`, expose a slider. On release, re-run the classifier on the last 14 nights using the in-memory threshold and render the delta. Commits only happen on explicit save.

8. **What-if replay.** "Pretend this night had the old baseline" / "pretend we hadn't wired in consistency_score yet." Run the staging pipeline against the in-memory overrides.

9. **Ad-hoc SQL scratchpad.** With saved queries the agent or dev has authored.

## Architecture decisions to make

### A. Standalone app vs. extend the tray?

**Recommend: standalone.** Dev tooling has different users, different deploy cadence, different feature tolerance for being broken. Mixing it into the tray means every dev-tool iteration risks the end-user experience. A standalone binary that connects to the same SQLite file is cleaner.

Name idea: `openwhoop-lab` or `openwhoop-labs`.

### B. Tech stack

Three live options:

**Option 1: Tauri (Rust + React).** Matches the tray's stack. Same skillset. Ships as a native app.
- Pro: reuses `openwhoop` lib directly, no HTTP layer, fast.
- Con: rebuilding every iteration is slower than pure web.

**Option 2: Rust web server (axum) + browser UI.** `openwhoop-lab serve` exposes localhost:3000, opens in browser.
- Pro: iterate the UI with full browser devtools, no app rebuild.
- Pro: easy to eventually share (over tailscale or similar) for remote dev review.
- Con: two process boundaries.

**Option 3: Python + Streamlit / Panel / Marimo.** Data-science-first. Minimal UI code.
- Pro: fastest to MVP. Great charts out of the box (plotly, altair).
- Con: introduces Python dep to the project. But it's dev-only, so…

**Lean: Option 3 (Marimo) for tier 1 and tier 2.** Python + Marimo gives us the full "notebook-but-live" experience, including agent-authored cells, for essentially free. It also matches where the Phase-2 ML work is going anyway (NeuroKit2 parity harness + eventual LightGBM training). Promote to Option 2 (standalone Rust) if we ever want a non-dev audience.

### C. Agent ↔ dashboard protocol

The agent needs a way to write into this thing. Two paths:

**Path A: write directly to `dev_notes` table via the same DB.** Simplest. Agent picks up DB path from `DATABASE_URL` env var. Notes get persisted same place as everything else.

**Path B: write to `docs/dev_dashboard/` as structured markdown.** Git-trackable. Survives DB resets. Version-controllable.

**Recommend: both.** Agent writes structured JSON annotations to `dev_notes`. Agent also writes longform markdown to `docs/dev_dashboard/YYYY-MM-DD-<title>.md`. Dashboard reads from both.

### D. Where it lives

Options:

- `~/code/openwhoop/lab/` subfolder in this repo.
- `~/code/openwhoop-lab` standalone new repo.
- Part of the tray repo as a second binary.

**Recommend: subfolder in `~/code/openwhoop`.** Easiest iteration — agent can write both the staging code and the dashboard panel in the same session. Promote to its own repo if it gets a life of its own and the repo boundary becomes useful (e.g. if you open-source the tray but not the lab).

## MVP concrete plan

**Goal: a thing you'd open this Saturday morning and go "oh, that's useful."**

Two-day-equivalent scope, in order:

1. New `dev_notes` SQLite table (one additive migration):
   ```sql
   CREATE TABLE dev_notes (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       created_at DATETIME NOT NULL,
       author TEXT NOT NULL,         -- "agent" or "dev"
       kind TEXT NOT NULL,           -- "note" | "question" | "experiment" | "diff"
       title TEXT NOT NULL,
       body_md TEXT,
       related_commit TEXT,          -- SHA or null
       related_feature TEXT,         -- "sleep_staging" | "quick_wins" | etc.
       related_range_start DATETIME, -- for "this applies to data from X..Y"
       related_range_end DATETIME,
       resolved_at DATETIME,
       resolved_by TEXT,
       payload_json JSON             -- before/after diffs, slider state, etc.
   );
   ```

2. Small Rust helper crate `openwhoop-lab-notes` with:
   - `DevNote::note(title, body)`, `DevNote::question(...)`, `DevNote::diff(...)` constructors
   - `write_note(&db, note) -> Result<()>`
   - Agent uses these during sessions.

3. A `lab/marimo_app.py` notebook that:
   - Reads `DATABASE_URL` (default `./db.sqlite` or the tray's live DB)
   - Panel 1: latest sleep hypnogram + stages + score breakdown
   - Panel 2: 14-night trend lines (score, efficiency, Deep %, REM %, daytime RMSSD)
   - Panel 3: latest sync log + any error messages, syntax-highlighted
   - Panel 4: unresolved `dev_notes` where `author = 'agent'`, rendered as markdown cards with "resolve" buttons
   - Panel 5: commit timeline (last 7 days of `git log`, clicking surfaces files changed)

4. A `Makefile` target: `make lab` launches it.

5. Agent SOP update: at the end of every session, write a `DevNote::note(...)` summarizing what happened and what's open, instead of (or in addition to) the current SESSION_LOG markdown pattern.

This gets us: a real dashboard, real agent-dev async comms, real commit-aware context — all inside a Marimo notebook that renders in a browser. Total new code: ~300 lines Rust (the notes helper) + ~400 lines Python.

## Tier 2/3 expansion (later)

Once the MVP is in hand and proves its value:

- **Live threshold tuning:** Marimo reactive slider → in-memory re-classify → delta table. Commit-to-file button writes a `DevNote::experiment` for the record.
- **Before/after diffs:** agent writes these as `DevNote::diff` with a structured payload; dashboard renders them as side-by-side tables/charts.
- **Natural-language search over notes and code.** Optional. Requires embedding store, but if it's local-only (e.g. `qdrant`, `sqlite-vss`) it's reasonable.

## Open questions for refinement

Questions I'd want your answer on before writing MVP code:

1. **Marimo (Python) or Tauri (Rust + React)?** My lean is Marimo for speed; yours?
2. **Same SQLite DB as the tray, or a separate one?** Separate lets you nuke the lab DB without affecting the tray. Same lets the lab show live tray data.
3. **Where does this live in the file tree?** `openwhoop/lab/`, `openwhoop-lab/` (new repo), or somewhere else?
4. **Who writes the first notebook cell?** I can generate the Marimo skeleton + first three panels, or you can prototype and I'll fill in.
5. **Do you want agent-authored `DevNote`s to start immediately (as part of the next algo work) even before the dashboard exists?** I can start writing them into the table now; the dashboard catches up later.

## Risks / things that could go sideways

- **Scope creep.** This could absorb a lot of time vs. just shipping features. Enforce tier discipline.
- **Python dep.** If you'd rather keep the whole stack in Rust, Marimo's off the table.
- **Notebook-state fatigue.** Marimo reactive cells can create weird coupling. Keep individual panels independent.
- **"Dashboard rot."** If the agent stops writing notes, the dashboard becomes a read-only view of stale data and nothing more. Needs agent-side discipline.

## Next step

Pick answers for the 5 questions above; I'll either (a) draft the MVP code, or (b) refine this plan further if one of the answers changes the architecture.
