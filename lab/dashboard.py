"""
OpenWhoop Lab — Interactive Dev Dashboard

Run with:
    cd /Users/totem/code/openwhoop
    make lab
    # or: marimo edit lab/dashboard.py

See `docs/DEV_DASHBOARD_CONCEPT.md` for the vision.

Marimo design constraints in this file:
- Every top-level variable is unique across cells (Marimo's reactive
  model requires it). Each panel's locals are prefixed (`inbox_*`,
  `sleep_*`, `trend_*`, `log_*`, `commits_*`) to avoid collisions.
- All cells read from the same `conn` + `mo` that cell 0 provides.
- Read-only SQLite queries only — `openwhoop note` CLI is the write
  path, dashboard is a display.
"""

import marimo

__generated_with = "0.14.0"
app = marimo.App(width="medium")


# ---------------------------------------------------------------- cell 0
# Setup: import marimo as `mo`, resolve DB path, expose a read-only
# connection factory. Everything downstream depends on this cell.
@app.cell
def setup():
    import marimo as mo
    import os
    import sqlite3
    import subprocess
    from datetime import datetime
    from pathlib import Path

    def resolve_db_path() -> str:
        # Resolution order: OPENWHOOP_DB env → DATABASE_URL if sqlite://
        # → repo's ./db.sqlite → tray's live DB → repo's ./db.sqlite.
        env = os.environ.get("OPENWHOOP_DB") or os.environ.get("DATABASE_URL", "")
        if env.startswith("sqlite://"):
            env = env.removeprefix("sqlite://").split("?", 1)[0]
        if env and Path(env).exists():
            return env
        repo_db = Path(__file__).resolve().parent.parent / "db.sqlite"
        if repo_db.exists():
            return str(repo_db)
        tray_db = (
            Path.home()
            / "Library/Application Support/dev.brennen.openwhoop-tray/db.sqlite"
        )
        if tray_db.exists():
            return str(tray_db)
        return str(repo_db)  # may not exist; panels render emptiness

    db_path = resolve_db_path()
    repo_root = str(Path(__file__).resolve().parent.parent)

    def open_ro():
        """Open a fresh read-only sqlite connection. Callers must close."""
        c = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        c.row_factory = sqlite3.Row
        return c

    return datetime, db_path, mo, open_ro, repo_root, subprocess


# ---------------------------------------------------------------- cell 1
# Title / DB identity.
@app.cell
def header(db_path, mo):
    mo.md(
        f"""# OpenWhoop Lab

**DB:** `{db_path}`

Live view of agent activity and the data behind it. Read-only.
Write notes via `openwhoop note "Title"` from the CLI.
"""
    )
    return


# ---------------------------------------------------------------- cell 2
# Panel 1 — Agent inbox (unresolved dev_notes).
@app.cell
def inbox(mo, open_ro):
    inbox_icon = {
        "note": "📝",
        "question": "❓",
        "experiment": "🔬",
        "diff": "🔁",
        "status": "🚧",
    }

    inbox_con = open_ro()
    try:
        inbox_rows = inbox_con.execute(
            """
            SELECT id, created_at, author, kind, title, body_md,
                   related_commit, related_feature
            FROM dev_notes
            WHERE resolved_at IS NULL
            ORDER BY created_at DESC
            LIMIT 50
            """
        ).fetchall()
    except Exception as inbox_err:
        inbox_rows = []
        inbox_error = f"_Could not read dev_notes: {inbox_err}_"
    else:
        inbox_error = None
    finally:
        inbox_con.close()

    if inbox_error:
        inbox_panel = mo.md(f"### 📥 Agent inbox\n\n{inbox_error}")
    elif not inbox_rows:
        inbox_panel = mo.md("### 📥 Agent inbox\n\n_No open notes. Quiet morning._")
    else:
        inbox_cards = []
        for _inbox_r in inbox_rows:
            _icon = inbox_icon.get(_inbox_r["kind"], "•")
            _commit_badge = (
                f"`{_inbox_r['related_commit']}`" if _inbox_r["related_commit"] else "—"
            )
            _feature_badge = _inbox_r["related_feature"] or "—"
            _body = (_inbox_r["body_md"] or "").strip()
            _card = f"""
**#{_inbox_r['id']}** {_icon} **{_inbox_r['title']}**
<sub>{_inbox_r['created_at']} · author `{_inbox_r['author']}` · feature `{_feature_badge}` · commit {_commit_badge}</sub>

{_body}

---
"""
            inbox_cards.append(_card)
        inbox_panel = mo.md(
            f"### 📥 Agent inbox ({len(inbox_rows)} open)\n\n" + "".join(inbox_cards)
        )
    inbox_panel
    return


# ---------------------------------------------------------------- cell 3
# Panel 2 — Latest sleep: hypnogram + stages + scalars.
@app.cell
def latest_sleep(datetime, mo, open_ro):
    import plotly.graph_objects as _sleep_go

    sleep_stage_color = {
        "Wake": "#e57373",
        "Light": "#81c784",
        "Deep": "#1976d2",
        "REM": "#ba68c8",
        "Unknown": "#bdbdbd",
    }

    sleep_con = open_ro()
    try:
        sleep_cycle = sleep_con.execute(
            """
            SELECT id, sleep_id, start, end, score, performance_score,
                   awake_minutes, light_minutes, deep_minutes, rem_minutes,
                   sleep_efficiency, sleep_latency_minutes, waso_minutes,
                   cycle_count, wake_event_count, avg_respiratory_rate,
                   skin_temp_deviation_c, sleep_need_hours, sleep_debt_hours,
                   classifier_version
            FROM sleep_cycles
            ORDER BY end DESC
            LIMIT 1
            """
        ).fetchone()
        if sleep_cycle is not None:
            sleep_epochs = sleep_con.execute(
                "SELECT epoch_start, epoch_end, stage FROM sleep_epochs "
                "WHERE sleep_cycle_id = ? ORDER BY epoch_start",
                (sleep_cycle["id"],),
            ).fetchall()
        else:
            sleep_epochs = []
    except Exception as sleep_err:
        sleep_cycle = None
        sleep_epochs = []
        sleep_error = f"_Could not read sleep_cycles: {sleep_err}_"
    else:
        sleep_error = None
    finally:
        sleep_con.close()

    def _opt(x, fmt="{:.1f}"):
        return fmt.format(x) if x is not None else "—"

    if sleep_error:
        sleep_panel = mo.md(f"### 🛌 Latest sleep\n\n{sleep_error}")
    elif sleep_cycle is None:
        sleep_panel = mo.md("### 🛌 Latest sleep\n\n_No sleep cycles yet._")
    else:
        sleep_start_ts = datetime.fromisoformat(sleep_cycle["start"].replace(" ", "T"))
        sleep_end_ts = datetime.fromisoformat(sleep_cycle["end"].replace(" ", "T"))
        sleep_duration_min = (sleep_end_ts - sleep_start_ts).total_seconds() / 60

        hypno_fig = _sleep_go.Figure()
        for _ep in sleep_epochs:
            hypno_fig.add_trace(
                _sleep_go.Scatter(
                    x=[_ep["epoch_start"], _ep["epoch_end"]],
                    y=[_ep["stage"], _ep["stage"]],
                    mode="lines",
                    line=dict(
                        color=sleep_stage_color.get(_ep["stage"], "#bdbdbd"), width=18
                    ),
                    showlegend=False,
                    hovertemplate=f"{_ep['stage']} · %{{x}}<extra></extra>",
                )
            )
        hypno_fig.update_layout(
            title=(
                f"Hypnogram — {sleep_cycle['start']} → {sleep_cycle['end']} "
                f"({sleep_duration_min:.0f} min)"
            ),
            xaxis_title="time",
            yaxis=dict(
                categoryorder="array",
                categoryarray=["Wake", "REM", "Light", "Deep", "Unknown"],
            ),
            height=220,
            margin=dict(l=40, r=20, t=40, b=30),
        )

        stage_min_map = {
            "Awake": sleep_cycle["awake_minutes"] or 0,
            "Light": sleep_cycle["light_minutes"] or 0,
            "Deep": sleep_cycle["deep_minutes"] or 0,
            "REM": sleep_cycle["rem_minutes"] or 0,
        }
        stage_fig = _sleep_go.Figure()
        for _stg, _m in stage_min_map.items():
            stage_fig.add_trace(
                _sleep_go.Bar(
                    y=["minutes"],
                    x=[_m],
                    name=_stg,
                    orientation="h",
                    marker_color=sleep_stage_color.get(
                        "Wake" if _stg == "Awake" else _stg, "#bdbdbd"
                    ),
                    hovertemplate=f"{_stg}: {_m:.1f} min<extra></extra>",
                )
            )
        stage_fig.update_layout(
            barmode="stack",
            height=120,
            margin=dict(l=40, r=20, t=20, b=30),
            showlegend=True,
        )

        sleep_md = f"""
### 🛌 Latest sleep — {sleep_cycle['sleep_id']}

- **Performance score:** **{_opt(sleep_cycle['performance_score'])}** / 100 (classifier `{sleep_cycle['classifier_version'] or '—'}`)
- **Efficiency:** {_opt(sleep_cycle['sleep_efficiency'])}% · **Latency:** {_opt(sleep_cycle['sleep_latency_minutes'])} min · **WASO:** {_opt(sleep_cycle['waso_minutes'])} min
- **Cycles:** {sleep_cycle['cycle_count'] if sleep_cycle['cycle_count'] is not None else '—'} · **Wake events:** {sleep_cycle['wake_event_count'] if sleep_cycle['wake_event_count'] is not None else '—'}
- **Respiratory:** {_opt(sleep_cycle['avg_respiratory_rate'])} bpm · **Skin temp Δ:** {_opt(sleep_cycle['skin_temp_deviation_c'], '{:.2f}')} °C
- **Sleep need:** {_opt(sleep_cycle['sleep_need_hours'])} h · **Debt:** {_opt(sleep_cycle['sleep_debt_hours'], '{:.2f}')} h
"""
        sleep_panel = mo.vstack([mo.md(sleep_md), stage_fig, hypno_fig])
    sleep_panel
    return


# ---------------------------------------------------------------- cell 4
# Panel 3 — 14-night trend (score, efficiency, Deep %, REM %).
@app.cell
def trend(mo, open_ro):
    import pandas as _trend_pd
    import plotly.graph_objects as _trend_go

    trend_con = open_ro()
    try:
        trend_rows = trend_con.execute(
            """
            SELECT sleep_id, end, performance_score, sleep_efficiency,
                   awake_minutes, light_minutes, deep_minutes, rem_minutes
            FROM sleep_cycles
            WHERE classifier_version IS NOT NULL AND classifier_version != 'failed'
            ORDER BY end DESC
            LIMIT 14
            """
        ).fetchall()
    except Exception as trend_err:
        trend_rows = []
        trend_error = f"_Could not read sleep_cycles: {trend_err}_"
    else:
        trend_error = None
    finally:
        trend_con.close()

    if trend_error:
        trend_panel = mo.md(f"### 📈 14-night trend\n\n{trend_error}")
    elif len(trend_rows) < 2:
        trend_panel = mo.md(
            f"### 📈 14-night trend\n\n_{len(trend_rows)} night(s) so far — need at "
            "least 2 classified nights to draw a trend._"
        )
    else:
        trend_df = _trend_pd.DataFrame([dict(r) for r in trend_rows]).iloc[::-1]
        trend_total = (
            trend_df["awake_minutes"].fillna(0)
            + trend_df["light_minutes"].fillna(0)
            + trend_df["deep_minutes"].fillna(0)
            + trend_df["rem_minutes"].fillna(0)
        )
        trend_df["deep_pct"] = (
            trend_df["deep_minutes"].fillna(0) / trend_total * 100
        ).where(trend_total > 0, 0)
        trend_df["rem_pct"] = (
            trend_df["rem_minutes"].fillna(0) / trend_total * 100
        ).where(trend_total > 0, 0)

        trend_fig = _trend_go.Figure()
        trend_fig.add_trace(
            _trend_go.Scatter(
                x=trend_df["sleep_id"],
                y=trend_df["performance_score"],
                name="Performance score",
                yaxis="y1",
            )
        )
        trend_fig.add_trace(
            _trend_go.Scatter(
                x=trend_df["sleep_id"],
                y=trend_df["sleep_efficiency"],
                name="Efficiency %",
                yaxis="y1",
            )
        )
        trend_fig.add_trace(
            _trend_go.Scatter(
                x=trend_df["sleep_id"], y=trend_df["deep_pct"], name="Deep %", yaxis="y2"
            )
        )
        trend_fig.add_trace(
            _trend_go.Scatter(
                x=trend_df["sleep_id"], y=trend_df["rem_pct"], name="REM %", yaxis="y2"
            )
        )
        trend_fig.update_layout(
            title="14-night sleep trends",
            height=320,
            yaxis=dict(title="Score / Efficiency", range=[0, 100]),
            yaxis2=dict(title="Stage %", overlaying="y", side="right", range=[0, 50]),
            margin=dict(l=40, r=40, t=40, b=30),
        )
        trend_panel = trend_fig
    trend_panel
    return


# ---------------------------------------------------------------- cell 5
# Panel 4 — Sync log (last 10 attempts).
@app.cell
def sync_log_panel(mo, open_ro):
    log_icon = {
        "success": "✅",
        "error": "❌",
        "cancelled": "⚠️",
        "timeout": "⏱",
        "in_progress": "⏳",
    }

    log_con = open_ro()
    try:
        log_rows = log_con.execute(
            """
            SELECT id, attempt_started_at, attempt_ended_at, outcome,
                   error_message, heart_rate_rows_added, sleep_cycles_created,
                   trigger
            FROM sync_log
            ORDER BY attempt_started_at DESC
            LIMIT 10
            """
        ).fetchall()
    except Exception as log_err:
        log_rows = []
        log_error = f"_Could not read sync_log: {log_err}_"
    else:
        log_error = None
    finally:
        log_con.close()

    if log_error:
        log_panel = mo.md(f"### 🔄 Sync log\n\n{log_error}")
    elif not log_rows:
        log_panel = mo.md("### 🔄 Sync log\n\n_No sync attempts recorded yet._")
    else:
        log_lines = ["### 🔄 Sync log (last 10 attempts)\n"]
        for _log_r in log_rows:
            _icon = log_icon.get(_log_r["outcome"], "•")
            _counts = (
                f"+{_log_r['heart_rate_rows_added'] or 0} HR rows · "
                f"+{_log_r['sleep_cycles_created'] or 0} cycles"
            )
            _trigger = _log_r["trigger"] or "—"
            _line = (
                f"- {_icon} **{_log_r['attempt_started_at']}** — "
                f"`{_log_r['outcome']}` ({_trigger}) · {_counts}"
            )
            if _log_r["error_message"]:
                _line += f"\n    ↳ error: `{_log_r['error_message']}`"
            log_lines.append(_line)
        log_panel = mo.md("\n".join(log_lines))
    log_panel
    return


# ---------------------------------------------------------------- cell 6
# Panel 5 — Recent git commits.
@app.cell
def commit_timeline(mo, repo_root, subprocess):
    try:
        commits_out = subprocess.run(
            ["git", "log", "--oneline", "--decorate", "-20", "--no-color"],
            capture_output=True,
            text=True,
            check=True,
            cwd=repo_root,
        )
        commits_list = commits_out.stdout.strip().split("\n")
    except Exception as commits_err:  # noqa: BLE001
        commits_list = [f"_git log failed: {commits_err}_"]
    commits_panel = mo.md(
        "### 🌳 Recent commits\n\n```\n" + "\n".join(commits_list[:20]) + "\n```"
    )
    commits_panel
    return


# ---------------------------------------------------------------- cell 7
# Footer.
@app.cell
def footer(mo):
    mo.md(
        """---
<sub>
Future panels (tier 2/3): threshold sliders, before/after diffs,
events/alarm timeline, wear-time + activity rollup, HRV trend,
in-dashboard note resolution. See
`docs/DEV_DASHBOARD_CONCEPT.md` for the roadmap.
</sub>
"""
    )
    return


if __name__ == "__main__":
    app.run()
