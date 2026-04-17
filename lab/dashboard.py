"""
OpenWhoop Lab — Interactive Dev Dashboard

Run with:
    cd /Users/totem/code/openwhoop
    make lab
    # or: marimo edit lab/dashboard.py

See `docs/DEV_DASHBOARD_CONCEPT.md` for the vision.
Backing table: `dev_notes` (see docs/DEV_DASHBOARD_CONCEPT.md §"MVP
concrete plan"). Every panel below reads from the same SQLite file
that the CLI and tray app use.

Design principles:
- Each cell (@app.cell) is independent; reactive dependencies are
  explicit via function parameters.
- Prefer pandas + plotly for charts — minimal ceremony.
- Read-only queries only; mutations happen via the CLI (`openwhoop
  note`) or the Rust pipeline, never from the notebook. Keeps the
  dashboard side-effect-free and safe to open at any time.
"""

import marimo as mo

__generated_with = "0.14.0"
app = mo.App(width="medium")


@app.cell
def _():
    import os
    import sqlite3
    import subprocess
    from pathlib import Path

    # DB resolution order: env var, repo root, tray's live path.
    def resolve_db_path() -> str:
        env = os.environ.get("OPENWHOOP_DB") or os.environ.get("DATABASE_URL", "")
        if env.startswith("sqlite://"):
            env = env.removeprefix("sqlite://").split("?", 1)[0]
        if env and Path(env).exists():
            return env
        repo_db = Path(__file__).resolve().parent.parent / "db.sqlite"
        if repo_db.exists():
            return str(repo_db)
        tray_db = Path.home() / "Library/Application Support/dev.brennen.openwhoop-tray/db.sqlite"
        if tray_db.exists():
            return str(tray_db)
        return str(repo_db)  # may not exist yet; panels will show emptiness

    db_path = resolve_db_path()

    def conn():
        # Read-only connection to make it obvious to reviewers that
        # the dashboard never mutates data.
        c = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        c.row_factory = sqlite3.Row
        return c

    return conn, db_path, mo, Path, subprocess


@app.cell
def _(db_path, mo):
    mo.md(
        f"""
        # OpenWhoop Lab

        **DB:** `{db_path}`

        Live view of the agent's recent work and the data behind it.
        Read-only. Write notes via `openwhoop note "title"` from the CLI.
        """
    )
    return


@app.cell
def _(conn, mo):
    """Panel 1 — Unresolved agent notes (the inbox)."""
    with conn() as c:
        rows = c.execute(
            """
            SELECT id, created_at, author, kind, title, body_md,
                   related_commit, related_feature
            FROM dev_notes
            WHERE resolved_at IS NULL
            ORDER BY created_at DESC
            LIMIT 50
            """
        ).fetchall()

    if not rows:
        panel = mo.md("### 📥 Agent inbox\n\n_No open notes. Quiet morning._")
    else:
        cards = []
        for r in rows:
            kind_icon = {
                "note": "📝",
                "question": "❓",
                "experiment": "🔬",
                "diff": "🔁",
                "status": "🚧",
            }.get(r["kind"], "•")
            commit_badge = f"`{r['related_commit']}`" if r["related_commit"] else "—"
            feature_badge = r["related_feature"] or "—"
            body = (r["body_md"] or "").strip()
            card = f"""
**#{r['id']}** {kind_icon} **{r['title']}**
<sub>{r['created_at']} · author `{r['author']}` · feature `{feature_badge}` · commit {commit_badge}</sub>

{body}

---
"""
            cards.append(card)
        panel = mo.md(f"### 📥 Agent inbox ({len(rows)} open)\n\n" + "".join(cards))
    panel
    return


@app.cell
def _(conn, mo):
    """Panel 2 — Latest sleep: score + stage breakdown + hypnogram."""
    import pandas as pd
    import plotly.graph_objects as go
    from datetime import datetime

    with conn() as c:
        cycle = c.execute(
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

        if cycle is None:
            epochs = []
        else:
            epochs = c.execute(
                "SELECT epoch_start, epoch_end, stage FROM sleep_epochs WHERE sleep_cycle_id = ? ORDER BY epoch_start",
                (cycle["id"],),
            ).fetchall()

    if cycle is None:
        panel = mo.md("### 🛌 Latest sleep\n\n_No sleep cycles yet._")
    else:
        stage_color = {
            "Wake": "#e57373",
            "Light": "#81c784",
            "Deep": "#1976d2",
            "REM": "#ba68c8",
            "Unknown": "#bdbdbd",
        }
        start_ts = datetime.fromisoformat(cycle["start"].replace(" ", "T"))
        end_ts = datetime.fromisoformat(cycle["end"].replace(" ", "T"))
        duration_min = (end_ts - start_ts).total_seconds() / 60

        # Hypnogram — horizontal colored bands.
        hypno_fig = go.Figure()
        if epochs:
            for e in epochs:
                hypno_fig.add_trace(
                    go.Scatter(
                        x=[e["epoch_start"], e["epoch_end"]],
                        y=[e["stage"], e["stage"]],
                        mode="lines",
                        line=dict(color=stage_color.get(e["stage"], "#bdbdbd"), width=18),
                        showlegend=False,
                        hovertemplate=f"{e['stage']} · %{{x}}<extra></extra>",
                    )
                )
        hypno_fig.update_layout(
            title=f"Hypnogram — {cycle['start']} → {cycle['end']} ({duration_min:.0f} min)",
            xaxis_title="time",
            yaxis=dict(
                categoryorder="array",
                categoryarray=["Wake", "REM", "Light", "Deep", "Unknown"],
            ),
            height=220,
            margin=dict(l=40, r=20, t=40, b=30),
        )

        # Stage breakdown as horizontal stacked bar.
        stage_min = {
            "Awake": cycle["awake_minutes"] or 0,
            "Light": cycle["light_minutes"] or 0,
            "Deep": cycle["deep_minutes"] or 0,
            "REM": cycle["rem_minutes"] or 0,
        }
        stage_fig = go.Figure()
        cum = 0
        for stg, m in stage_min.items():
            stage_fig.add_trace(
                go.Bar(
                    y=["minutes"],
                    x=[m],
                    name=stg,
                    orientation="h",
                    marker_color=stage_color.get(
                        "Wake" if stg == "Awake" else stg, "#bdbdbd"
                    ),
                    hovertemplate=f"{stg}: {m:.1f} min<extra></extra>",
                )
            )
            cum += m
        stage_fig.update_layout(
            barmode="stack",
            height=120,
            margin=dict(l=40, r=20, t=20, b=30),
            showlegend=True,
        )

        # Number block.
        md = f"""
### 🛌 Latest sleep — {cycle['sleep_id']}

- **Performance score:** **{cycle['performance_score']:.1f}** / 100 (classifier `{cycle['classifier_version'] or '—'}`)
- **Efficiency:** {cycle['sleep_efficiency']:.1f}% · **Latency:** {cycle['sleep_latency_minutes']:.1f} min · **WASO:** {cycle['waso_minutes']:.1f} min
- **Cycles:** {cycle['cycle_count']} · **Wake events:** {cycle['wake_event_count']}
- **Respiratory:** {cycle['avg_respiratory_rate']:.1f} bpm · **Skin temp Δ:** {cycle['skin_temp_deviation_c']:.2f}°C
- **Sleep need:** {cycle['sleep_need_hours']:.1f} h · **Debt:** {cycle['sleep_debt_hours']:.2f} h
"""
        panel = mo.vstack([mo.md(md), stage_fig, hypno_fig])
    panel
    return


@app.cell
def _(conn, mo):
    """Panel 3 — 14-night trends: score, efficiency, Deep %, REM %."""
    import pandas as pd
    import plotly.graph_objects as go

    with conn() as c:
        rows = c.execute(
            """
            SELECT sleep_id, end, performance_score, sleep_efficiency,
                   awake_minutes, light_minutes, deep_minutes, rem_minutes
            FROM sleep_cycles
            WHERE classifier_version IS NOT NULL AND classifier_version != 'failed'
            ORDER BY end DESC
            LIMIT 14
            """
        ).fetchall()
    if not rows:
        panel = mo.md("### 📈 14-night trend\n\n_Not enough nights yet._")
    else:
        df = pd.DataFrame([dict(r) for r in rows]).iloc[::-1]
        total = df["awake_minutes"] + df["light_minutes"] + df["deep_minutes"] + df["rem_minutes"]
        df["deep_pct"] = (df["deep_minutes"] / total * 100).where(total > 0, 0)
        df["rem_pct"] = (df["rem_minutes"] / total * 100).where(total > 0, 0)

        fig = go.Figure()
        fig.add_trace(
            go.Scatter(x=df["sleep_id"], y=df["performance_score"], name="Performance score", yaxis="y1")
        )
        fig.add_trace(
            go.Scatter(x=df["sleep_id"], y=df["sleep_efficiency"], name="Efficiency %", yaxis="y1")
        )
        fig.add_trace(
            go.Scatter(x=df["sleep_id"], y=df["deep_pct"], name="Deep %", yaxis="y2")
        )
        fig.add_trace(
            go.Scatter(x=df["sleep_id"], y=df["rem_pct"], name="REM %", yaxis="y2")
        )
        fig.update_layout(
            title="14-night sleep trends",
            height=320,
            yaxis=dict(title="Score / Efficiency", range=[0, 100]),
            yaxis2=dict(title="Stage %", overlaying="y", side="right", range=[0, 50]),
            margin=dict(l=40, r=40, t=40, b=30),
        )
        panel = fig
    panel
    return


@app.cell
def _(conn, mo):
    """Panel 4 — Sync log: most recent attempts + outcome + error."""
    with conn() as c:
        rows = c.execute(
            """
            SELECT id, attempt_started_at, attempt_ended_at, outcome,
                   error_message, heart_rate_rows_added, sleep_cycles_created, trigger
            FROM sync_log
            ORDER BY attempt_started_at DESC
            LIMIT 10
            """
        ).fetchall()
    if not rows:
        panel = mo.md("### 🔄 Sync log\n\n_No sync attempts recorded yet._")
    else:
        lines = ["### 🔄 Sync log (last 10 attempts)\n"]
        for r in rows:
            icon = {
                "success": "✅",
                "error": "❌",
                "cancelled": "⚠️",
                "timeout": "⏱",
                "in_progress": "⏳",
            }.get(r["outcome"], "•")
            counts = f"+{r['heart_rate_rows_added'] or 0} HR rows · +{r['sleep_cycles_created'] or 0} cycles"
            trigger = r["trigger"] or "—"
            line = f"- {icon} **{r['attempt_started_at']}** — `{r['outcome']}` ({trigger}) · {counts}"
            if r["error_message"]:
                line += f"\n    ↳ error: `{r['error_message']}`"
            lines.append(line)
        panel = mo.md("\n".join(lines))
    panel
    return


@app.cell
def _(mo, subprocess):
    """Panel 5 — Commit timeline: recent git activity."""
    try:
        out = subprocess.run(
            ["git", "log", "--oneline", "--decorate", "-20", "--no-color"],
            capture_output=True,
            text=True,
            check=True,
            cwd=str(__import__("pathlib").Path(__file__).resolve().parent.parent),
        )
        commits = out.stdout.strip().split("\n")
    except Exception as e:  # noqa: BLE001
        commits = [f"_git log failed: {e}_"]
    panel = mo.md(
        "### 🌳 Recent commits\n\n```\n" + "\n".join(commits[:20]) + "\n```"
    )
    panel
    return


@app.cell
def _(mo):
    mo.md(
        """
        ---
        <sub>
        Future panels (tier 2/3): threshold sliders, before/after diffs,
        events/alarm timeline, wear-time + activity rollup, HRV trend.
        See `docs/DEV_DASHBOARD_CONCEPT.md` for the roadmap.
        </sub>
        """
    )
    return


if __name__ == "__main__":
    app.run()
