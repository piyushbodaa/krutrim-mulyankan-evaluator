"""`km`: the Kritrim Mulyankan command line. `mj` is kept as an alias."""

from __future__ import annotations

from enum import StrEnum
from pathlib import Path
from typing import Annotated, NoReturn
import os

import typer
from dotenv import find_dotenv, load_dotenv
from rich.console import Console
from rich.markup import escape
from rich.table import Table

from kritrim_mulyankan.engine import PreflightError, preflight, run_suite
from kritrim_mulyankan.export import write_export
from kritrim_mulyankan.importer import import_suite
from kritrim_mulyankan.pricing import load_prices
from kritrim_mulyankan.report import LEADERBOARD_HEADERS, build_report, leaderboard_cells, to_html
from kritrim_mulyankan.report import to_markdown as report_markdown
from kritrim_mulyankan.runners import RUNNER_CATALOG, Runner, make_runner
from kritrim_mulyankan.settings import Settings
from kritrim_mulyankan.store import ResultRecord, Store
from kritrim_mulyankan.suite import Suite, discover_suites, load_suite, resolve_suite

app = typer.Typer(
    help=(
        "Krutrim Mulyankan: test and compare AI models, benchmark harnesses and personal agents "
        "on the same tasks."
    ),
    no_args_is_help=True,
    add_completion=False,
)
console = Console()
err = Console(stderr=True)


def _apply_console_width() -> None:
    """Honour COLUMNS at invocation time.

    Rich snapshots environ when Console is constructed, and a dumb/non-TTY
    terminal short-circuits size detection to 80x25 unless both width and
    height are set. CliRunner tests export COLUMNS=250 for full cell text.
    """
    columns = os.environ.get("COLUMNS")
    if columns and columns.isdigit():
        width = max(40, int(columns))
        lines = os.environ.get("LINES")
        height = int(lines) if lines and lines.isdigit() else max(25, console.height or 25)
        console.size = (width, height)
        err.size = (width, height)


@app.callback()
def main() -> None:
    # Secrets come from the environment or a local .env; real env vars win.
    load_dotenv(find_dotenv(usecwd=True), override=False)
    _apply_console_width()


def _fail(msg: str) -> NoReturn:
    err.print(f"[red]error:[/red] {escape(msg)}")
    raise typer.Exit(1)


def _load(name_or_path: str, settings: Settings) -> Suite:
    try:
        return load_suite(resolve_suite(name_or_path, settings.suites_dir))
    except (FileNotFoundError, ValueError) as exc:
        _fail(str(exc))


class Listable(StrEnum):
    suites = "suites"
    tasks = "tasks"
    runners = "runners"
    prices = "prices"
    runs = "runs"


@app.command("list")
def list_cmd(
    what: Annotated[Listable, typer.Argument(help="What to list.")] = Listable.suites,
    suite: Annotated[
        str | None, typer.Option("--suite", "-s", help="Only this suite (for tasks).")
    ] = None,
) -> None:
    """List suites, tasks, runners, model prices or past runs."""
    settings = Settings.from_env()

    if what is Listable.prices:
        try:
            prices = load_prices(settings.prices_path)
        except ValueError as exc:
            _fail(str(exc))
        table = Table("Provider/model", "Input $/MTok", "Output $/MTok", "Source", "As of")
        for key, price in sorted(prices.prices.items()):
            table.add_row(
                key, f"{price.input:g}", f"{price.output:g}", price.source or "", price.as_of or ""
            )
        console.print(table)
        override = settings.prices_path
        state = "loaded" if override.is_file() else "not present"
        console.print(f"Overrides: {override} ({state}). Models without a price are refused.")
        return

    if what is Listable.runners:
        table = Table("Runner spec", "Kind", "Status", "Notes")
        for spec, kind, ready, note in RUNNER_CATALOG:
            table.add_row(spec, kind, "ready" if ready else "[dim]planned[/dim]", note)
        console.print(table)
        return

    if what is Listable.runs:
        if not settings.db_path.exists():
            console.print("No runs yet. Try: km run example -r echo -r oracle")
            return
        with Store(settings.db_path) as store:
            runs = store.list_runs()
        table = Table("Run", "Suite", "Runners", "Status", "Spent USD", "Started")
        for r in runs:
            table.add_row(
                r.id,
                f"{r.suite_name} v{r.suite_version}",
                escape(", ".join(r.runners)),
                r.status,
                f"${r.spent_usd:.4f}",
                r.started_at[:19].replace("T", " "),
            )
        console.print(table)
        return

    paths = (
        [resolve_suite(suite, settings.suites_dir)]
        if suite
        else discover_suites(settings.suites_dir)
    )
    if not paths:
        console.print(f"No suites found in {settings.suites_dir}/")
        return
    suites = [_load(str(p), settings) for p in paths]

    if what is Listable.suites:
        table = Table("Suite", "Version", "Tasks", "Categories", "File")
        for s in suites:
            cats = ", ".join(sorted({t.category for t in s.tasks}))
            table.add_row(s.name, s.version, str(len(s.tasks)), cats, str(s.path))
        console.print(table)
    else:
        table = Table("Suite", "Task", "v", "Category", "Grader", "Timeout", "Budget")
        for s in suites:
            for t in s.tasks:
                budget = "–" if t.budget_usd is None else f"${t.budget_usd:.2f}"
                table.add_row(
                    s.name, t.id, str(t.version), t.category, t.grader.type,
                    f"{t.timeout_s:g}s", budget,
                )  # fmt: skip
        console.print(table)


@app.command()
def run(
    suite: Annotated[str, typer.Argument(help="Suite name in suites/ or a path to a YAML file.")],
    runner: Annotated[
        list[str] | None,
        typer.Option("--runner", "-r", help="Runner spec, repeatable. See `km list runners`."),
    ] = None,
    task: Annotated[
        list[str] | None, typer.Option("--task", "-t", help="Only these task ids, repeatable.")
    ] = None,
    seed: Annotated[int, typer.Option(help="Run seed; each task derives its own from it.")] = 0,
    max_usd: Annotated[
        float | None, typer.Option(help="Hard budget cap for this run (default MJ_MAX_USD).")
    ] = None,
    dry_run: Annotated[
        bool, typer.Option("--dry-run", help="Estimate cost and stop; runs nothing.")
    ] = False,
    allow_paid: Annotated[
        bool, typer.Option(help="Explicitly permit paid API calls after preflight.")
    ] = False,
) -> None:
    """Run a suite against one or more runners, grade, store and report."""
    settings = Settings.from_env()
    loaded = _load(suite, settings)
    try:
        prices = load_prices(settings.prices_path)
        runners: list[Runner] = [make_runner(s, prices=prices) for s in (runner or ["echo"])]
        tasks = loaded.select(task)
    except ValueError as exc:
        _fail(str(exc))
    if len({r.name for r in runners}) != len(runners):
        _fail("the same runner was given twice")
    cap = settings.max_usd if max_usd is None else max_usd

    # Always check first: keys, prices and the worst case against the cap. Nothing is called.
    check = preflight(tasks, runners, cap)
    table = Table(
        "Runner", "Attempts", "Worst case USD", "Worst case INR", "Status", title="Cost estimate"
    )
    runner_problems = {r.name: r.problems() for r in runners}
    for r in runners:
        mine = [e for e in check.estimates if e.runner == r.name]
        known = sum(e.cost_usd for e in mine if e.cost_usd is not None)
        unknown = any(e.cost_usd is None for e in mine)
        usd = "unknown" if unknown else f"${known:.4f}"
        inr = "unknown" if unknown else f"₹{known * settings.usd_to_inr:.2f}"
        status = "[red]not ready[/red]" if runner_problems[r.name] or unknown else "ready"
        table.add_row(escape(r.name), str(len(mine)), usd, inr, status)
    console.print(table)
    worst_inr = check.worst_case_usd * settings.usd_to_inr
    console.print(
        f"Worst case ${check.worst_case_usd:.4f} (₹{worst_inr:.2f}) against a cap of ${cap:.2f}. "
        "Worst case = prompt bytes in, max_output_tokens out."
    )
    for warning in check.warnings:
        err.print(f"[yellow]warning:[/yellow] {escape(warning)}")
    for problem in check.problems:
        err.print(f"[red]problem:[/red] {escape(problem)}")
    if dry_run:
        verdict = "a real run would start" if check.ok else "a real run would be refused"
        console.print(f"Dry run: nothing was called or spent; {verdict}.")
        raise typer.Exit(0 if check.ok else 1)
    if not check.ok:
        _fail("refusing to run; fix the problems above (nothing was called or spent)")
    if not allow_paid and check.worst_case_usd > 0:
        _fail("paid execution requires --allow-paid and an approved --max-usd budget")

    def progress(r: ResultRecord) -> None:
        latency = "" if r.latency_s is None else f"{r.latency_s:.3f}s"
        detail = escape(r.error or r.grade_status or "")
        console.print(
            f" {_mark(r)} {escape(r.task_id):<28} {escape(r.runner):<22} {latency:>8}  {detail}"
        )

    with Store(settings.db_path) as store:
        try:
            summary = run_suite(
                loaded,
                runners,
                store=store,
                settings=settings,
                seed=seed,
                max_usd=cap,
                task_ids=task,
                on_result=progress,
            )
        except PreflightError as exc:
            _fail(str(exc))
        paths = _write_reports(store, summary.run_id, summary.run_dir, "both")
        _print_leaderboard(store, summary.run_id)

    if summary.status == "budget_exceeded":
        err.print("[yellow]warning:[/yellow] budget cap reached; some attempts were skipped.")
    console.print(f"Run [bold]{summary.run_id}[/bold] · raw outputs in {summary.run_dir}/")
    for p in paths:
        console.print(f"Report: {p}")


class ReportFormat(StrEnum):
    md = "md"
    html = "html"
    both = "both"
    json = "json"


@app.command("import-suite")
def import_cmd(
    source: Path, destination: Path, name: str = "india-pilot", version: str = "2026-10-12"
) -> None:
    """Validate original JSON/JSONL items and create a YAML suite; never overwrite one."""
    try:
        suite = import_suite(source, destination, name=name, version=version)
    except (OSError, ValueError) as exc:
        _fail(str(exc))
    console.print(f"Imported {len(suite.tasks)} items to {destination} (unvalidated pilot).")


@app.command("ui")
def ui_cmd(project_dir: Path = Path("."), port: int = 8765) -> None:
    """Open a local pilot workbench with task selection and free mock checks."""
    from kritrim_mulyankan.local_ui import serve

    if not 1024 <= port <= 65535:
        _fail("choose a port between 1024 and 65535")
    try:
        serve(project_dir, port)
    except (OSError, ValueError) as exc:
        _fail(str(exc))


@app.command("export")
def export_cmd(
    run_id: str | None = None, output: Path | None = None, include_outputs: bool = False
) -> None:
    """Export schema v1 JSON. Output text is private and excluded by default."""
    settings = Settings.from_env()
    if not settings.db_path.is_file():
        _fail("No runs yet")
    with Store(settings.db_path) as store:
        selected = run_id or store.latest_run_id()
        run = store.get_run(selected) if selected else None
        if run is None:
            _fail("run not found")
        destination = output or Path(run.artifacts_dir) / "results.json"
        try:
            write_export(store, run.id, destination, include_outputs=include_outputs)
        except (OSError, ValueError) as exc:
            _fail(str(exc))
    console.print(f"Export: {destination}")


@app.command()
def report(
    run_id: Annotated[str | None, typer.Argument(help="Run id (default: latest).")] = None,
    fmt: Annotated[ReportFormat, typer.Option("--format", "-f")] = ReportFormat.both,
) -> None:
    """Write the Markdown/HTML report for a run and print its leaderboard."""
    settings = Settings.from_env()
    if not settings.db_path.exists():
        _fail(f"no results database at {settings.db_path}; run `km run` first")
    with Store(settings.db_path) as store:
        run_id = run_id or store.latest_run_id()
        run = store.get_run(run_id) if run_id else None
        if run is None:
            _fail(f"no run {run_id!r}" if run_id else "no runs recorded yet")
        paths = _write_reports(store, run.id, Path(run.artifacts_dir), fmt)
        _print_leaderboard(store, run.id)
    for p in paths:
        console.print(f"Report: {p}")


def _mark(r: ResultRecord) -> str:
    if r.status == "error":
        return "[red]![/red]"
    if r.status == "skipped":
        return "-"
    if r.passed is None:
        return "·"  # graded later (e.g. awaiting a judge)
    return "[green]✓[/green]" if r.passed else "[red]✗[/red]"


def _write_reports(store: Store, run_id: str, out_dir: Path, fmt: str) -> list[Path]:
    rep = build_report(store, run_id)
    out_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    if fmt in ("md", "both"):
        paths.append(out_dir / "report.md")
        paths[-1].write_text(report_markdown(rep), encoding="utf-8")
    if fmt in ("html", "both"):
        paths.append(out_dir / "report.html")
        paths[-1].write_text(to_html(rep), encoding="utf-8")
    if fmt in ("json", "both"):
        paths.append(write_export(store, run_id, out_dir / "results.json"))
    return paths


def _print_leaderboard(store: Store, run_id: str) -> None:
    rep = build_report(store, run_id)
    table = Table(*LEADERBOARD_HEADERS, title=f"{rep.run.suite_name} v{rep.run.suite_version}")
    for row in rep.rows:
        table.add_row(*(escape(c) for c in leaderboard_cells(row)))
    console.print(table)
