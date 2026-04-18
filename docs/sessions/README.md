# Session archives

Per-session working docs from autonomous agent runs — plans, logs,
decision records, and migration summaries. Archived here so root stays
clean and the docs are still retrievable by date.

One directory per session, named `YYYY-MM-DD-<short-slug>`. Inside:
- `PLAN.md` — the plan going in
- `SESSION_LOG_*.md` — what actually happened, appended as work proceeded
- `DECISIONS.md` — non-obvious choices with rationale
- `MIGRATION_SUMMARY.md` — what new migrations / tables / modules landed
- `SKIPPED.md` — features deferred with reason
- `SESSION_PLAN_*.md` — variant of PLAN.md for different session scopes

These are point-in-time records. Claims about code or line numbers may
drift; `git log` is always authoritative for the current state.

## Current sessions

- [2026-04-17-quick-wins](2026-04-17-quick-wins/) — sleep-staging scoring wiring + 7 quick-wins features + lab dashboard
