"""Aggregate one run's results and render them as Markdown and HTML."""

from __future__ import annotations

import html
from dataclasses import dataclass
from statistics import mean

from kritrim_mulyankan.metrics import mean_ci, wilson_interval
from kritrim_mulyankan.store import ResultRecord, RunRecord, Store

DEFINITIONS = (
    "Success = passed / (graded + errored); pending (awaiting a judge) and skipped attempts are "
    "excluded. Score is normalised to 0-1, and a human score override wins when set. Intervals "
    "are 95% (Wilson for success, normal approximation for score). Fake runners report "
    "approximate tokens (characters / 4)."
)


# Indian-language benchmarks this project learns from; see docs/related-benchmarks.md.
RELATED_BENCHMARKS = [
    ("Samiksha", "Microsoft Research India and Karya", "https://evals.karya.in/samiksha/"),
    ("BharatBench", "Ola Krutrim", "https://tech.olakrutrim.com/bharat-bench/"),
]


@dataclass
class RunnerRow:
    runner: str
    kind: str
    passed: int
    judged: int
    success: float | None
    success_ci: tuple[float, float] | None
    score: tuple[float, float, float] | None
    pending: int
    errors: int
    skipped: int
    latency_s: float | None
    input_tokens: int | None
    output_tokens: int | None
    cost_usd: float | None
    cost_inr: float | None


@dataclass
class Report:
    run: RunRecord
    rows: list[RunnerRow]
    task_ids: list[str]
    categories: dict[str, str]
    runners: list[str]
    cells: dict[tuple[str, str], str]


def effective_score(r: ResultRecord) -> float | None:
    return r.human_score if r.human_score is not None else r.score


def build_report(store: Store, run_id: str) -> Report:
    run = store.get_run(run_id)
    if run is None:
        raise KeyError(f"no run {run_id!r}")
    results = store.results_for_run(run_id)

    rows = []
    for runner in run.runners:
        rs = [r for r in results if r.runner == runner]
        graded = [r for r in rs if r.status == "ok" and r.grade_status == "graded"]
        errors = sum(r.status == "error" for r in rs)
        passed = sum(bool(r.passed) for r in graded)
        judged = len(graded) + errors
        scores = [s for r in graded if (s := effective_score(r)) is not None]
        latencies = [r.latency_s for r in rs if r.status == "ok" and r.latency_s is not None]
        rows.append(
            RunnerRow(
                runner=runner,
                kind=rs[0].runner_kind if rs else "?",
                passed=passed,
                judged=judged,
                success=passed / judged if judged else None,
                success_ci=wilson_interval(passed, judged),
                score=mean_ci(scores),
                pending=sum(r.status == "ok" and r.grade_status != "graded" for r in rs),
                errors=errors,
                skipped=sum(r.status == "skipped" for r in rs),
                latency_s=mean(latencies) if latencies else None,
                input_tokens=sum(r.input_tokens or 0 for r in rs)
                if all(r.input_tokens is not None or r.status == "skipped" for r in rs)
                else None,
                output_tokens=sum(r.output_tokens or 0 for r in rs)
                if all(r.output_tokens is not None or r.status == "skipped" for r in rs)
                else None,
                cost_usd=sum(r.cost_usd for r in rs)
                if all(r.cost_usd is not None for r in rs)
                else None,
                cost_inr=sum(r.cost_inr for r in rs)
                if all(r.cost_inr is not None for r in rs)
                else None,
            )
        )
    rows.sort(
        key=lambda r: (-1 if r.success is None else r.success, r.score[0] if r.score else -1),
        reverse=True,
    )

    task_ids = list(dict.fromkeys(r.task_id for r in results))
    return Report(
        run=run,
        rows=rows,
        task_ids=task_ids,
        categories={r.task_id: r.category for r in results},
        runners=run.runners,
        cells={(r.task_id, r.runner): cell(r) for r in results},
    )


def cell(r: ResultRecord) -> str:
    if r.status != "ok":
        return r.status
    if r.grade_status == "graded":
        mark = "✓" if r.passed else "✗"
        score = effective_score(r)
        return f"{mark} {score:.2f}" if score is not None else mark
    return "judge error" if r.grade_status == "error" else "pending"


def _pct(x: float | None) -> str:
    return "–" if x is None else f"{x:.0%}"


def _ci(ci: tuple[float, float] | None) -> str:
    return "–" if ci is None else f"{ci[0]:.0%}–{ci[1]:.0%}"


def _score(s: tuple[float, float, float] | None) -> str:
    return "–" if s is None else f"{s[0]:.2f} ({max(0, s[1]):.2f}–{min(1, s[2]):.2f})"


def _secs(x: float | None) -> str:
    return "–" if x is None else f"{x:.3f}s" if x < 10 else f"{x:.1f}s"


LEADERBOARD_HEADERS = [
    "Runner", "Kind", "Success", "95% CI", "Mean score (95% CI)", "Pending", "Errors",
    "Skipped", "Mean latency", "Tokens in / out", "Cost USD", "Cost INR",
]  # fmt: skip


def leaderboard_cells(row: RunnerRow) -> list[str]:
    return [
        row.runner,
        row.kind,
        f"{_pct(row.success)} ({row.passed}/{row.judged})",
        _ci(row.success_ci),
        _score(row.score),
        str(row.pending),
        str(row.errors),
        str(row.skipped),
        _secs(row.latency_s),
        ("unknown" if row.input_tokens is None else f"{row.input_tokens:,}")
        + " / "
        + ("unknown" if row.output_tokens is None else f"{row.output_tokens:,}"),
        "unknown" if row.cost_usd is None else f"${row.cost_usd:.4f}",
        "unknown" if row.cost_inr is None else f"₹{row.cost_inr:.2f}",
    ]


def _meta_line(run: RunRecord) -> str:
    spent_inr = run.spent_usd * run.usd_to_inr
    qualifier = "known cost (total unknown)" if run.uncertain_cost else "token-priced cost"
    return (
        f"Run `{run.id}` · status **{run.status}** · seed {run.seed} · "
        f"{qualifier} ${run.spent_usd:.4f} "
        f"(₹{spent_inr:.2f}) of ${run.max_usd:.2f} cap · km {run.mj_version}"
    )


def to_markdown(rep: Report) -> str:
    def table(headers: list[str], rows: list[list[str]]) -> list[str]:
        esc = [[c.replace("|", "\\|") for c in r] for r in rows]
        return [
            "| " + " | ".join(headers) + " |",
            "|" + "|".join("---" for _ in headers) + "|",
            *("| " + " | ".join(r) + " |" for r in esc),
        ]

    lines = [
        f"# Krutrim Mulyankan report: {rep.run.suite_name} v{rep.run.suite_version}",
        "",
        _meta_line(rep.run),
        "",
        "## Leaderboard",
        "",
        *table(LEADERBOARD_HEADERS, [leaderboard_cells(r) for r in rep.rows]),
        "",
        f"_{DEFINITIONS}_",
        "",
        "## Per task",
        "",
        *table(
            ["Task", "Category", *rep.runners],
            [
                [t, rep.categories[t], *(rep.cells.get((t, r), "–") for r in rep.runners)]
                for t in rep.task_ids
            ],
        ),
        "",
    ]
    return "\n".join(lines)


_CSS = """
:root { --bg:#fbfbfa; --fg:#1d1d1b; --muted:#6b6b66; --line:#e3e2de; --head:#f1f0ec;
        --pass:#1f7a3a; --fail:#b3261e; }
@media (prefers-color-scheme: dark) {
  :root { --bg:#161615; --fg:#ecebe7; --muted:#a19f98; --line:#33322f; --head:#21201e;
          --pass:#6fcf8a; --fail:#f2877e; } }
body { background:var(--bg); color:var(--fg); margin:0;
       font:15px/1.5 ui-sans-serif, system-ui, sans-serif; }
main { max-width:1100px; margin:0 auto; padding:32px 16px 64px; }
h1 { font-size:1.5rem; margin:0 0 .25rem; } h2 { font-size:1.1rem; margin:2rem 0 .5rem; }
.meta, .note { color:var(--muted); font-size:.9rem; }
.scroll { overflow-x:auto; }
table { border-collapse:collapse; width:100%; font-variant-numeric:tabular-nums; }
th, td { text-align:left; padding:6px 10px; border-bottom:1px solid var(--line);
         white-space:nowrap; }
th { background:var(--head); font-weight:600; }
code { font:0.9em ui-monospace, monospace; }
.pass { color:var(--pass); } .fail { color:var(--fail); } .other { color:var(--muted); }
"""


def to_html(rep: Report) -> str:
    e = html.escape

    def table(headers: list[str], rows: list[list[str]], classes: list[list[str]] | None = None):
        head = "".join(f"<th>{e(h)}</th>" for h in headers)
        body = []
        for i, r in enumerate(rows):
            tds = "".join(
                f'<td class="{classes[i][j]}">{e(c)}</td>' if classes else f"<td>{e(c)}</td>"
                for j, c in enumerate(r)
            )
            body.append(f"<tr>{tds}</tr>")
        return f'<div class="scroll"><table><tr>{head}</tr>{"".join(body)}</table></div>'

    def css_class(text: str) -> str:
        return "pass" if text.startswith("✓") else "fail" if text.startswith("✗") else "other"

    task_rows = [
        [t, rep.categories[t], *(rep.cells.get((t, r), "–") for r in rep.runners)]
        for t in rep.task_ids
    ]
    task_classes = [["", "", *(css_class(c) for c in row[2:])] for row in task_rows]
    run = rep.run
    meta = (
        f"Run <code>{e(run.id)}</code> · status <b>{e(run.status)}</b> · seed {run.seed} · "
        f"{'known cost (total unknown)' if run.uncertain_cost else 'token-priced cost'} "
        f"${run.spent_usd:.4f} (₹{run.spent_usd * run.usd_to_inr:.2f}) of "
        f"${run.max_usd:.2f} cap · km {e(run.mj_version)}"
    )
    title = f"Krutrim Mulyankan: {run.suite_name} v{run.suite_version}"
    related = " · ".join(
        f'<a href="{e(url)}">{e(name)}</a> ({e(org)})' for name, org, url in RELATED_BENCHMARKS
    )
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{e(title)}</title>
<style>{_CSS}</style>
</head>
<body>
<main>
<h1>{e(title)}</h1>
<p class="meta">{meta}</p>
<h2>Leaderboard</h2>
{table(LEADERBOARD_HEADERS, [leaderboard_cells(r) for r in rep.rows])}
<p class="note">{e(DEFINITIONS)}</p>
<h2>Per task</h2>
{table(["Task", "Category", *rep.runners], task_rows, task_classes)}
<footer class="note">Related benchmarks: {related}</footer>
</main>
</body>
</html>
"""
