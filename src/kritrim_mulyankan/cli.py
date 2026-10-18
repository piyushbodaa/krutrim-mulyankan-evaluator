"""`km`: the Kritrim Mulyankan command line. `mj` is kept as an alias."""

from __future__ import annotations

import os
from datetime import date
from enum import StrEnum
from pathlib import Path
from typing import Annotated, NoReturn

import typer
from dotenv import find_dotenv, load_dotenv
from rich.console import Console
from rich.markup import escape
from rich.table import Table

from kritrim_mulyankan.engine import MAX_REPEATS, PreflightError, preflight, run_suite
from kritrim_mulyankan.export import write_export
from kritrim_mulyankan.history import load_history
from kritrim_mulyankan.importer import import_suite
from kritrim_mulyankan.judge import judge_problems, make_judge, parse_judge_spec
from kritrim_mulyankan.microeval import write_micro_eval
from kritrim_mulyankan.miner import (
    VerdictCache,
    estimate_judge_cost_usd,
    mine_failures,
    select_pairs,
)
from kritrim_mulyankan.pricing import load_prices
from kritrim_mulyankan.report import (
    LEADERBOARD_HEADERS,
    VERDICT_HEADERS,
    build_report,
    leaderboard_cells,
    to_html,
    verdict_cells,
)
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
    repeats: Annotated[
        int,
        typer.Option(
            min=1,
            max=MAX_REPEATS,
            help="Tries per task and runner, run as whole rounds; enables consistency stats.",
        ),
    ] = 1,
    max_usd: Annotated[
        float | None, typer.Option(help="Hard budget cap for this run (default MJ_MAX_USD).")
    ] = None,
    dry_run: Annotated[
        bool, typer.Option("--dry-run", help="Estimate cost and stop; runs nothing.")
    ] = False,
    allow_paid: Annotated[
        bool, typer.Option(help="Explicitly permit paid API calls after preflight.")
    ] = False,
    judge_spec: Annotated[
        str | None,
        typer.Option(
            "--judge",
            help="Grading judge as <provider>/<model> (default MJ_JUDGE or the suite's judge).",
        ),
    ] = None,
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

    # Resolve the grading judge: --judge wins, then MJ_JUDGE, then the suite's own
    # judge_model (micro-evals record theirs). Without one, llm_judge tasks stay
    # 'pending' exactly as before; nothing is guessed.
    needs_judge = [t for t in tasks if t.grader.type == "llm_judge"]
    resolved_judge = judge_spec or os.environ.get("MJ_JUDGE")
    if needs_judge and not resolved_judge:
        first = next((t for t in needs_judge if t.grader.judge_model), None)
        resolved_judge = first.grader.judge_model if first is not None else None
    judge = None
    judge_paid = False
    judge_worst_usd: float | None = 0.0
    if resolved_judge and needs_judge:
        problems = judge_problems(resolved_judge)
        if problems:
            _fail(f"judge {resolved_judge!r} cannot run: {'; '.join(problems)}")
        provider, model = parse_judge_spec(resolved_judge)
        price = prices.lookup(provider, model)
        judge_paid = price is None or (price.input > 0 or price.output > 0)
        if price is None:
            judge_worst_usd = None  # unbounded judge refuses like an unpriced runner
        else:
            from kritrim_mulyankan.judge import DEFAULT_JUDGE_MAX_TOKENS
            from kritrim_mulyankan.pricing import max_input_tokens

            calls = len(needs_judge) * len(runners) * repeats
            per_call_in = max(
                max_input_tokens(t.render_prompt() + (t.rubric or "") + (t.reference or ""))
                + t.max_output_tokens
                for t in needs_judge
            )
            judge_worst_usd = calls * price.cost(per_call_in, DEFAULT_JUDGE_MAX_TOKENS)
        judge = make_judge(resolved_judge)
    elif judge_spec and not needs_judge:
        console.print("--judge given but no selected task uses an LLM judge; ignoring it.")

    # Always check first: keys, prices and the worst case against the cap. Nothing is called.
    check = preflight(tasks, runners, cap, repeats=repeats)
    table = Table(
        "Runner", "Attempts", "Worst case USD", "Worst case INR", "Status", title="Cost estimate"
    )
    runner_problems = {r.name: r.problems() for r in runners}
    for r in runners:
        mine = [e for e in check.estimates if e.runner == r.name]
        known = repeats * sum(e.cost_usd for e in mine if e.cost_usd is not None)
        unknown = any(e.cost_usd is None for e in mine)
        usd = "unknown" if unknown else f"${known:.4f}"
        inr = "unknown" if unknown else f"₹{known * settings.usd_to_inr:.2f}"
        status = "[red]not ready[/red]" if runner_problems[r.name] or unknown else "ready"
        table.add_row(escape(r.name), str(len(mine) * repeats), usd, inr, status)
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
    if judge is not None:
        if judge_worst_usd is None:
            err.print(
                f"[red]problem:[/red] no price for judge {resolved_judge}; grading cost "
                "cannot be bounded (add it to prices.yaml)."
            )
        else:
            total_worst = check.worst_case_usd + judge_worst_usd
            console.print(
                f"Grading judge: {resolved_judge} ({len(needs_judge)} task(s)), worst case "
                f"${judge_worst_usd:.4f} (₹{judge_worst_usd * settings.usd_to_inr:.2f}); "
                f"total worst case ${total_worst:.4f} against the ${cap:.2f} cap."
            )
            if total_worst > cap:
                err.print(
                    "[red]problem:[/red] runners + grading judge exceed the cap; "
                    "raise --max-usd or run fewer tasks/runners."
                )
    elif needs_judge:
        err.print(
            f"[yellow]warning:[/yellow] {len(needs_judge)} task(s) need an LLM judge and none "
            "is configured; they will be left 'pending'. Pass --judge <provider>/<model>."
        )
    judge_ok = judge_worst_usd is not None and (
        check.worst_case_usd + (judge_worst_usd or 0.0) <= cap
    )
    if dry_run:
        verdict = (
            "a real run would start" if check.ok and judge_ok else "a real run would be refused"
        )
        console.print(f"Dry run: nothing was called or spent; {verdict}.")
        raise typer.Exit(0 if check.ok and judge_ok else 1)
    if not check.ok or not judge_ok:
        _fail("refusing to run; fix the problems above (nothing was called or spent)")
    if not allow_paid and (check.worst_case_usd > 0 or judge_paid):
        _fail("paid execution requires --allow-paid and an approved --max-usd budget")

    def progress(r: ResultRecord) -> None:
        latency = "" if r.latency_s is None else f"{r.latency_s:.3f}s"
        detail = escape(r.error or r.grade_status or "")
        task_label = r.task_id + (f" (try {r.repeat_index + 1}/{repeats})" if repeats > 1 else "")
        console.print(
            f" {_mark(r)} {escape(task_label):<28} {escape(r.runner):<22} {latency:>8}  {detail}"
        )

    with Store(settings.db_path) as store:
        try:
            summary = run_suite(
                loaded,
                runners,
                store=store,
                settings=settings,
                seed=seed,
                repeats=repeats,
                max_usd=cap,
                task_ids=task,
                judge=judge,
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


class HistorySource(StrEnum):
    auto = "auto"
    chatgpt = "chatgpt"
    claude = "claude"
    claude_code = "claude-code"


def _history_source(value: HistorySource):  # noqa: ANN202 - Literal for load_history
    return "claude_code" if value is HistorySource.claude_code else value.value


@app.command("history")
def history_cmd(
    paths: Annotated[
        list[Path], typer.Argument(help="Export files/zips, or Claude Code *.jsonl / folder.")
    ],
    source: Annotated[
        HistorySource, typer.Option("--source", help="Format (default: auto-detect).")
    ] = HistorySource.auto,
) -> None:
    """Inspect your own chat history exports offline. Counts only; prints no content."""
    try:
        history = load_history(paths, _history_source(source))
    except (FileNotFoundError, OSError, ValueError) as exc:
        _fail(str(exc))
    pairs = history.pairs
    table = Table("Source", "Conversations", "Prompts with an answer")
    pair_counts: dict[str, int] = {}
    for pair in pairs:
        pair_counts[pair.source] = pair_counts.get(pair.source, 0) + 1
    for src, count in sorted(history.by_source().items()):
        table.add_row(src, str(count), str(pair_counts.get(src, 0)))
    console.print(table)
    console.print(
        f"Total: {len(history.conversations)} conversations, {len(pairs)} prompts. "
        "Nothing was uploaded or called."
    )


def _print_failure_breakdown(failures) -> None:  # noqa: ANN001
    if not failures:
        return
    from collections import Counter

    by_source = Counter(f.pair.source for f in failures)
    by_model = Counter(f.pair.model or "unknown model" for f in failures)
    table = Table("Failures by", "Count", title="Where your AI went wrong")
    for src, count in by_source.most_common():
        table.add_row(f"source: {src}", str(count))
    for model, count in by_model.most_common():
        table.add_row(f"original model: {model}", str(count))
    console.print(table)


@app.command("micro-eval")
def micro_eval_cmd(
    paths: Annotated[
        list[Path], typer.Argument(help="Your exports: ChatGPT/Claude zip or Claude Code data.")
    ],
    judge_spec: Annotated[
        str | None,
        typer.Option("--judge", help="Mining judge as <provider>/<model> (default MJ_JUDGE)."),
    ] = None,
    out: Annotated[Path, typer.Option("--out", "-o", help="Suite YAML to create.")] = Path(
        "suites/my-micro-eval.yaml"
    ),
    name: Annotated[str, typer.Option(help="Suite name.")] = "my-micro-eval",
    version: Annotated[str | None, typer.Option(help="Suite version (default: today).")] = None,
    source: Annotated[
        HistorySource, typer.Option("--source", help="Format (default: auto-detect).")
    ] = HistorySource.auto,
    limit: Annotated[
        int | None, typer.Option("--limit", min=1, help="Judge at most this many prompts.")
    ] = None,
    min_confidence: Annotated[
        float,
        typer.Option("--min-confidence", min=0.0, max=1.0, help="Keep wrong verdicts >= this."),
    ] = 0.7,
    max_usd: Annotated[
        float | None, typer.Option(help="Hard budget cap for judging (default MJ_MAX_USD).")
    ] = None,
    dry_run: Annotated[
        bool, typer.Option("--dry-run", help="Parse and price judging; calls nothing.")
    ] = False,
    allow_paid: Annotated[
        bool, typer.Option(help="Explicitly permit paid judge calls within --max-usd.")
    ] = False,
    cache_path: Annotated[
        Path, typer.Option("--cache", help="Verdict cache file (re-runs are free).")
    ] = Path("runs/judge-cache.jsonl"),
    no_cache: Annotated[
        bool, typer.Option("--no-cache", help="Do not use the verdict cache.")
    ] = False,
) -> None:
    """Mine prompts your AI answered wrongly and build them into a personal suite."""
    settings = Settings.from_env()
    try:
        history = load_history(paths, _history_source(source))
    except (FileNotFoundError, OSError, ValueError) as exc:
        _fail(str(exc))
    pairs = history.pairs
    selection = select_pairs(pairs, limit)
    selected = selection.pairs
    if not selected:
        _fail("no prompt/answer pairs found in that history")
    console.print(
        f"{len(history.conversations)} conversations, {len(pairs)} prompts; "
        f"judging {len(selected)} "
        f"({selection.duplicates} duplicates and {selection.oversized} oversized skipped)."
    )
    resolved_judge = judge_spec or os.environ.get("MJ_JUDGE")
    if not resolved_judge:
        _fail(
            "a mining judge is required: pass --judge <provider>/<model> or set MJ_JUDGE "
            "(free local example: --judge ollama/llama3.2:3b)"
        )
    problems = judge_problems(resolved_judge)
    for problem in problems:
        err.print(f"[red]problem:[/red] judge: {escape(problem)}")
    cache = None if no_cache else VerdictCache(cache_path)
    try:
        prices = load_prices(settings.prices_path)
        estimate = estimate_judge_cost_usd(pairs, resolved_judge, prices, limit=limit, cache=cache)
    except ValueError as exc:
        _fail(str(exc))
    cap = settings.max_usd if max_usd is None else max_usd
    if estimate is None:
        err.print(
            f"[red]problem:[/red] no price for judge {resolved_judge}; add it to prices.yaml "
            "(see `km list prices`) so judging cost can be bounded."
        )
    else:
        console.print(
            f"Judge {resolved_judge}: worst-case judging cost ${estimate:.4f} "
            f"(₹{estimate * settings.usd_to_inr:.2f}) against a cap of ${cap:.2f} "
            f"for {len(selected)} prompts."
        )
        if estimate > cap:
            err.print(
                "[red]problem:[/red] worst-case judging cost exceeds the cap; "
                "raise --max-usd or use --limit."
            )
    ready = not problems and estimate is not None and estimate <= cap
    if dry_run:
        console.print(
            "Dry run: nothing was called or spent; "
            + ("a real mining would start." if ready else "a real mining would be refused.")
        )
        raise typer.Exit(0 if ready else 1)
    if not ready:
        _fail("refusing to mine; fix the problems above (nothing was called or spent)")
    assert estimate is not None
    if estimate > 0 and not allow_paid:
        _fail("paid judging requires --allow-paid and an approved --max-usd budget")

    def progress(done: int, total: int, _pair) -> None:  # noqa: ANN001
        if done == 1 or done == total or done % 25 == 0:
            console.print(f" judging {done}/{total}…")

    result = mine_failures(
        pairs,
        make_judge(resolved_judge),
        judge_spec=resolved_judge,
        min_confidence=min_confidence,
        limit=limit,
        on_progress=progress,
        cache=cache,
    )
    console.print(
        f"Checked {result.checked}: {len(result.failures)} failures kept, "
        f"{result.correct} right, {result.unclear} unclear, "
        f"{result.below_threshold} wrong below {min_confidence} confidence, "
        f"{result.errors} judge errors skipped, {result.cache_hits} from cache, "
        f"{result.redacted_pairs} pairs with secrets redacted before judging."
    )
    _print_failure_breakdown(result.failures)
    if not result.failures:
        _fail("no confident failures found; no suite written. Try more history or --limit off.")
    try:
        suite, evidence_path = write_micro_eval(
            result.failures,
            out,
            name=name,
            version=version or date.today().isoformat(),
            judge_spec=resolved_judge,
        )
    except (OSError, ValueError) as exc:
        _fail(str(exc))
    console.print(f"Micro-eval: {out} ({len(suite.tasks)} tasks). Evidence: {evidence_path}.")
    console.print("Keep the evidence file private: it contains your original prompts/answers.")
    console.print(
        f"Run it: km run {name} -r model:<provider>/<model> --judge {resolved_judge} "
        "--allow-paid --max-usd <budget> (free local check first: -r oracle -r echo)"
    )


@app.command("doctor")
def doctor_cmd() -> None:
    """Check suites, prices, keys and the local judge path. Calls no model APIs."""
    settings = Settings.from_env()
    rows: list[tuple[str, str]] = []
    hard_fail = False
    paths = discover_suites(settings.suites_dir)
    loaded = 0
    for path in paths:
        try:
            load_suite(path)
            loaded += 1
        except ValueError as exc:
            rows.append((f"suite {path.name}", f"BROKEN: {exc}"))
            hard_fail = True
    rows.append(("suites", f"{loaded}/{len(paths)} load from {settings.suites_dir}/"))
    try:
        prices = load_prices(settings.prices_path)
        rows.append(("prices", f"{len(prices)} priced models"))
    except ValueError as exc:
        rows.append(("prices", f"BROKEN: {exc}"))
        hard_fail = True
        prices = None  # type: ignore[assignment]
    from kritrim_mulyankan.runners.providers import PROVIDERS, credentials_problem

    ready, missing = [], []
    for provider in sorted(PROVIDERS):
        (missing if credentials_problem(provider, os.environ) else ready).append(provider)
    rows.append(("providers ready", ", ".join(ready) or "none"))
    rows.append(("providers missing keys", ", ".join(missing) or "none"))
    judge = os.environ.get("MJ_JUDGE")
    if judge:
        problems = judge_problems(judge)
        rows.append(("MJ_JUDGE", judge if not problems else f"{judge} — {'; '.join(problems)}"))
    else:
        rows.append(("MJ_JUDGE", "not set (pass --judge when mining/running)"))
    import urllib.request

    try:
        with urllib.request.urlopen("http://localhost:11434/api/tags", timeout=2) as resp:
            rows.append(("ollama", f"reachable (HTTP {resp.status}) — free local judge"))
    except Exception:
        rows.append(("ollama", "not reachable at localhost:11434 (fine if unused)"))
    table = Table("Check", "Status", title="km doctor")
    for check, status in rows:
        table.add_row(check, escape(status))
    console.print(table)
    if hard_fail:
        _fail("doctor found broken suites/prices above")


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
    if rep.repeats > 1:
        consistency = Table(
            "Runner", "Right every time", "Sometimes right", "Never right",
            title=f"Consistency over {rep.repeats} tries per task",
        )  # fmt: skip
        for row in rep.rows:
            counts = (str(row.always), str(row.sometimes), str(row.never))
            consistency.add_row(escape(row.runner), *counts)
        console.print(consistency)
    if rep.verdicts:
        verdicts = Table(*VERDICT_HEADERS, title="Is the difference real?")
        for verdict in rep.verdicts:
            verdicts.add_row(*(escape(c) for c in verdict_cells(verdict)))
        console.print(verdicts)
        console.print("Too close to call means the gap could be chance, not that they are equal.")
