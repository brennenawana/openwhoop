# OpenWhoop Lab

Interactive dev dashboard. Shows recent agent activity, data snapshots
from the algorithms, sleep-cycle breakdowns, and the sync log.

## Why this exists

See `../docs/DEV_DASHBOARD_CONCEPT.md` for the full vision. The short
version: agent writes structured notes into the `dev_notes` table
while working; this dashboard surfaces them next to the data they
describe, so review is interactive rather than "read a long markdown
file + run SQL."

## Running it

First-time setup:

```sh
cd /Users/totem/code/openwhoop
python -m venv lab/.venv
source lab/.venv/bin/activate
pip install -r lab/requirements.txt
```

Then launch:

```sh
# from repo root
make lab
# equivalent:
source lab/.venv/bin/activate && marimo edit lab/dashboard.py
```

A browser tab opens at `localhost:2718` with the reactive notebook.

## What it reads

In this resolution order:

1. `$OPENWHOOP_DB` env var (absolute path to a SQLite file)
2. `$DATABASE_URL` if it starts with `sqlite://`
3. The tray's live DB at `~/Library/Application Support/dev.brennen.openwhoop-tray/db.sqlite` — **this is the default**, since the tray daemon is what actually writes sync data
4. `./db.sqlite` (repo root — stale fallback, used by the CLI when no other DB is wired up)

The connection is **read-only** (opened with `?mode=ro`) so the
dashboard can never mutate data. Writes happen via the CLI
(`openwhoop note "title"`) or the Rust pipeline.

> **Note:** the default `openwhoop note ...` invocation (using the
> repo's `.env`) writes to `./db.sqlite`, not the tray DB. To log
> notes against the live tray DB that the dashboard reads, use the
> `make note` helper which sets `DATABASE_URL` for you:
> ```sh
> make note ARGS='"Title" --kind status --feature dev_dashboard --body "..."'
> ```

## What it shows

Five panels in current MVP:

1. **Agent inbox** — unresolved `dev_notes`. Each card shows title,
   kind (note/question/experiment/diff/status), body, feature scope,
   commit SHA. Resolve from the CLI (future: from within the
   dashboard).
2. **Latest sleep** — performance score, stage minutes + percentages,
   efficiency, latency, WASO, cycles, respiratory rate, skin-temp
   deviation, need/debt. Plus a colored hypnogram strip.
3. **14-night trends** — performance score, efficiency, Deep %,
   REM %. Two y-axes (scores left, stage % right).
4. **Sync log** — last 10 sync attempts with outcome + error messages.
5. **Commit timeline** — `git log --oneline -20` from the repo.

## Writing notes from an agent session

```sh
# note summary at end of session
openwhoop note "Deep HR investigation — next-day plan" \
    --kind status \
    --feature sleep_staging \
    --body "Ran baseline-deleted reclassify; got 134 Deep epochs. With personalized baseline, 17. See SESSION_LOG_20260417.md § Block 5."

# question awaiting dev decision
openwhoop note "Pick Option 1, 2, or 3 for the Deep HR gate" \
    --kind question \
    --feature sleep_staging \
    --body "See docs/DEEP_HR_RESEARCH_PROMPT.md — results land in DEEP_HR_RESEARCH_FINDINGS.md when ready."
```

`--commit` auto-fills with the current HEAD short SHA; override only
if you want to reference a different commit.

## Resolving a note

From SQLite for now (dashboard-driven resolution is a tier-2 feature):

```sh
sqlite3 db.sqlite "UPDATE dev_notes SET resolved_at = datetime('now'), resolved_by = 'dev' WHERE id = 42;"
```

## Known limitations / tier-2 work

- No write/resolve from within the dashboard yet — read-only by design in MVP.
- No threshold-sliders / what-if replay yet (tier 3).
- No HRV/activity/wear charts yet. Scaffolding exists in the DB; panels are a future iteration.
- Notebook state can be stale — press `R` in marimo to re-run all cells after a new sync.

See `docs/DEV_DASHBOARD_CONCEPT.md` §"Tier 2/3 expansion" for what's next.
