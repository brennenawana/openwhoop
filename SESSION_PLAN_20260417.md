# Overnight Session Plan — 2026-04-17

**Started:** evening of 2026-04-17
**Purpose:** push the sleep staging work forward while Brennen sleeps. Focus on
completing the scoring inputs that fall back to neutrals today, plus
workspace cleanup. Pure backend work — no frontend/UI changes, no new
algorithms. Every task is testable and reversible.

## Rewind anchor — baseline commits

If any of this goes sideways, reset to these exact SHAs to undo
everything I do overnight.

| Repo | Branch | SHA | Meaning |
|---|---|---|---|
| `~/code/openwhoop` | `master` | `15a5889` | `fix(sleep-staging): threshold tuning` (pushed) |
| `~/code/openwhoop` | `feat/dashboard-command` | `a86411d` | `Add dashboard command` (pushed) |
| `~/code/openwhoop` | `integration/whoop-tray` (origin) | `9b53641` | `Merge branch 'master' into integration/whoop-tray` (pushed) |
| `~/code/openwhoop-tray` | `main` | `6bf8cb6` | `Wire stage_sleep into sync pipeline` (local only) |
| `~/code/openwhoop-tray` | `vendor/openwhoop` submodule | `9b53641` | same as integration/whoop-tray above |

Rewind recipes:

```sh
# openwhoop master to baseline
cd ~/code/openwhoop
git fetch origin
git checkout master
git reset --hard 15a5889
git push --force-with-lease origin master  # only if I pushed past this
```

```sh
# openwhoop-tray main to baseline (local only, no force-push concern)
cd ~/code/openwhoop-tray
git checkout main
git reset --hard 6bf8cb6
```

Any new branches I create are safe to `git branch -D <name>`.

## Plan — approx 2 h of work

Goal: finish wiring the three score-component inputs that currently
fall back to neutral values (`consistency_score`, `avg_sleep_stress`,
`prior_day_strain`). Today Performance Score uses placeholders for the
first two and pretends strain is always zero for need. After tonight
it uses real values from the existing `SleepConsistencyAnalyzer`,
`heart_rate.stress` column, and `StrainCalculator`.

Also clean up two pre-existing clippy warnings that have been bugging
the workspace.

### Blocks

1. **Clippy cleanup** (15 min) — `time_math.rs:203` `approx_constant`
   and `db.rs:59,101` `redundant_closure`. Non-functional, makes
   `cargo clippy --workspace --all-targets` clean.
2. **Wire `consistency_score`** (30 min) — call
   `SleepConsistencyAnalyzer::calculate_consistency_metrics()` over
   the user's recent N nights, feed `total_score` into
   `ScoringInputs`. Currently defaults to 50.
3. **Wire `avg_sleep_stress`** (30 min) — aggregate `heart_rate.stress`
   over each sleep window via a new DB query. Feed into
   `ScoringInputs`. Currently defaults to 5 (neutral).
4. **Wire `prior_day_strain`** (30 min) — query prior day's
   heart_rate, run `StrainCalculator`, feed into `SleepNeedInputs`.
   Currently `None` → zero adjustment.
5. **Reclassify + verify** (15 min) — run against a `/tmp` copy of
   the live tray DB, diff performance_score and sleep_need_hours
   before/after. Document deltas in the log.
6. **Docs + wrap** (remaining) — update `docs/SLEEP_STAGING.md` §14
   TODO table, note the calibration shift in §6 history, finalize
   `SESSION_LOG_20260417.md`.

Each step is its own commit on master. If any step is blocked I skip
it and log why; no partial-feature commits.

### What I will NOT touch tonight

- Frontend / Tauri commands / UI rendering — needs your design input.
- Phase 2 ONNX — requires Python offline work.
- NeuroKit2 parity fixtures — same.
- The live tray DB — only operate on `/tmp` copies.
- `db.sqlite.pre-staging` — your backup, left alone.
- `integration/whoop-tray` branch — unchanged; if `stage_one_cycle`
  changes in master, a future merge will bring it over.

## Morning review checklist

- [ ] Read `SESSION_LOG_20260417.md` for blow-by-blow what happened
- [ ] `git log --oneline 15a5889..master` to see the new commits
- [ ] `cargo test --workspace` should still pass (tests written per commit)
- [ ] `cargo clippy --workspace --all-targets` should now be clean
- [ ] If happy: push master and let me know. If not: use rewind recipe above.
