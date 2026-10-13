import pytest

from kritrim_mulyankan.engine import run_suite
from kritrim_mulyankan.metrics import mean_ci, wilson_interval
from kritrim_mulyankan.report import build_report, to_html, to_markdown
from kritrim_mulyankan.runners import EchoRunner, OracleRunner


def test_wilson_interval():
    assert wilson_interval(0, 0) is None
    low, high = wilson_interval(5, 5)
    assert high == 1.0 and low == pytest.approx(0.5655, abs=1e-3)
    low, high = wilson_interval(0, 5)
    assert low == 0.0 and high == pytest.approx(0.4345, abs=1e-3)


def test_mean_ci():
    assert mean_ci([]) is None
    assert mean_ci([0.5]) == (0.5, 0.5, 0.5)
    m, low, high = mean_ci([0.0, 1.0, 0.0, 1.0])
    assert m == 0.5 and low < 0.5 < high


@pytest.fixture
def report(example_suite, store, settings):
    summary = run_suite(
        example_suite, [EchoRunner(), OracleRunner()], store=store, settings=settings
    )
    return build_report(store, summary.run_id)


def test_leaderboard(report):
    rows = {r.runner: r for r in report.rows}
    assert [r.runner for r in report.rows] == ["oracle", "echo"]  # best first
    assert (rows["oracle"].passed, rows["oracle"].judged, rows["oracle"].success) == (4, 4, 1.0)
    assert (rows["echo"].passed, rows["echo"].judged, rows["echo"].success) == (0, 4, 0.0)
    assert rows["oracle"].pending == 1 and rows["oracle"].errors == 0
    assert rows["echo"].input_tokens > 0 and rows["echo"].cost_usd == 0


def test_task_matrix(report):
    assert len(report.task_ids) == 5
    assert report.cells[("coding.sum-of-squares", "oracle")] == "✓ 1.00"
    assert report.cells[("coding.sum-of-squares", "echo")] == "✗ 0.00"
    assert report.cells[("writing.late-order-apology", "echo")] == "pending"


def test_human_score_overrides_the_grader(example_suite, store, settings):
    summary = run_suite(example_suite, [EchoRunner()], store=store, settings=settings)
    with store.conn:
        store.conn.execute(
            "UPDATE results SET human_score = 1.0 WHERE run_id = ? AND task_id = ?",
            (summary.run_id, "coding.sum-of-squares"),
        )
    rep = build_report(store, summary.run_id)
    assert rep.cells[("coding.sum-of-squares", "echo")] == "✗ 1.00"
    assert rep.rows[0].score[0] == pytest.approx(0.25)


def test_markdown_and_html(report):
    md = to_markdown(report)
    assert md.startswith("# Krutrim Mulyankan report: example v1")
    assert "## Leaderboard" in md and "## Per task" in md
    assert "| oracle | fake | 100% (4/4)" in md

    page = to_html(report)
    assert page.startswith("<!doctype html>")
    assert "prefers-color-scheme: dark" in page
    assert '<td class="pass">✓ 1.00</td>' in page


def test_html_footer_links_related_benchmarks(report):
    page = to_html(report)
    assert "Related benchmarks:" in page
    assert '<a href="https://evals.karya.in/samiksha/">Samiksha</a>' in page
    assert '<a href="https://tech.olakrutrim.com/bharat-bench/">BharatBench</a>' in page


def test_html_escapes_content(report):
    report.cells[("coding.sum-of-squares", "echo")] = "<script>x</script>"
    assert "<script>x</script>" not in to_html(report)
