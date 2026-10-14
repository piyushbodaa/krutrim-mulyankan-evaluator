"""Parity between the Python km and the Rust km on free/local runs.

Runs both implementations of `km run` on the bundled suites (oracle + echo,
no network, no keys, no spend) and compares results.json, report.md and
manifest.json after removing volatile fields (run ids, times, latencies,
latency-driven speed verdicts, artifact hashes, source provenance, and
configuration ids, which hash the source tree each side reports).

Skipped unless a Rust binary is available: set KM_RUST_BIN (and optionally
KM_RUST_DESKTOP_BIN), or build km-rs/ first (release preferred, then debug).
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest
from typer.testing import CliRunner

from kritrim_mulyankan.cli import app

ROOT = Path(__file__).resolve().parents[1]


def _find_bin(env_name: str, names: list[Path]) -> Path | None:
    if env := os.environ.get(env_name):
        candidate = Path(env)
        return candidate if candidate.is_file() else None
    for candidate in names:
        if candidate.is_file():
            return candidate
    return None


RUST_KM = _find_bin(
    "KM_RUST_BIN",
    [
        ROOT / "km-rs/target/x86_64-unknown-linux-musl/release/km",
        ROOT / "km-rs/target/release/km",
        ROOT / "km-rs/target/debug/km",
    ],
)
RUST_DESKTOP = _find_bin(
    "KM_RUST_DESKTOP_BIN",
    [ROOT / "km-rs/target/release/km-desktop", ROOT / "km-rs/target/debug/km-desktop"],
)

pytestmark = pytest.mark.skipif(
    RUST_KM is None,
    reason="Rust km not built (set KM_RUST_BIN or build km-rs/)",
)

VOLATILE = {
    "id",
    "started_at",
    "finished_at",
    "latency_s",
    "attempt_started_at",
    "attempt_finished_at",
    "latency_scope",
    "sha256",
    "configuration_id",
    "source",
    "mean_latency_s",
    "median_latency_s",
}
MANIFEST_VOLATILE = VOLATILE | {"python", "run_id"}


def _scrub(value, volatile=VOLATILE):
    if isinstance(value, dict):
        return {
            key: "<volatile>" if key in volatile else _scrub(item, volatile)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_scrub(item, volatile) for item in value]
    return value


def _export(payload: dict) -> dict:
    payload = json.loads(json.dumps(payload))
    # Speed verdicts depend on measured latencies, which always differ.
    payload["verdicts"] = [v for v in payload["verdicts"] if v["metric"] == "accuracy"]
    return _scrub(payload)


def _run_python(suite_args: list[str], base: Path, extra_env: dict) -> Path:
    runner = CliRunner()
    env = {
        "MJ_RUNS_DIR": str(base / "runs"),
        "MJ_DB": str(base / "runs/results.db"),
        "MJ_SUITES_DIR": str(ROOT / "suites"),
        "COLUMNS": "250",
        **extra_env,
    }
    result = runner.invoke(app, suite_args, env=env)
    assert result.exit_code == 0, result.output
    return base / "runs"


def _run_rust(suite_args: list[str], base: Path, extra_env: dict) -> Path:
    env = {
        **os.environ,
        "MJ_RUNS_DIR": str(base / "runs"),
        "MJ_DB": str(base / "runs/results.db"),
        "MJ_SUITES_DIR": str(ROOT / "suites"),
        "COLUMNS": "250",
        **extra_env,
    }
    proc = subprocess.run(
        [str(RUST_KM), *suite_args],
        env=env,
        text=True,
        capture_output=True,
        timeout=300,
    )
    assert proc.returncode == 0, proc.stdout + proc.stderr
    return base / "runs"


def _one_run_dir(runs: Path) -> Path:
    dirs = sorted(path for path in runs.iterdir() if path.is_dir())
    assert len(dirs) == 1, [d.name for d in runs.iterdir()]
    return dirs[0]


def _compare(suite_args: list[str], tmp_path: Path) -> None:
    py_dir = _one_run_dir(_run_python(suite_args, tmp_path / "py", {}))
    rs_dir = _one_run_dir(_run_rust(suite_args, tmp_path / "rs", {}))

    py_results = json.loads((py_dir / "results.json").read_text(encoding="utf-8"))
    rs_results = json.loads((rs_dir / "results.json").read_text(encoding="utf-8"))
    assert _export(rs_results) == _export(py_results)

    py_manifest = json.loads((py_dir / "manifest.json").read_text(encoding="utf-8"))
    rs_manifest = json.loads((rs_dir / "manifest.json").read_text(encoding="utf-8"))
    assert _scrub(rs_manifest, MANIFEST_VOLATILE) == _scrub(py_manifest, MANIFEST_VOLATILE)

    def clean_md(text: str) -> list[str]:
        lines = []
        for line in text.splitlines():
            if line.startswith("Run `"):
                line = re.sub(r"Run `[^`]+`", "Run `<id>`", line)
            if "Speed" in line or "was faster" in line or "takes" in line:
                continue  # latency-dependent speed verdicts
            if "| Mean latency |" in line or " Mean latency " in line:
                cells = line.split("|")
                for idx, cell in enumerate(cells):
                    if cell.strip().endswith("s") and any(ch.isdigit() for ch in cell):
                        cells[idx] = " <latency> "
                line = "|".join(cells)
            lines.append(line)
        return lines

    py_md = clean_md((py_dir / "report.md").read_text(encoding="utf-8"))
    rs_md = clean_md((rs_dir / "report.md").read_text(encoding="utf-8"))
    assert rs_md == py_md


@pytest.mark.parametrize(
    "suite", ["suites/example.yaml", "suites/india-pilot.yaml", "suites/railway-pilot.yaml"]
)
def test_run_parity_bundled_suites(suite: str, tmp_path: Path) -> None:
    _compare(["run", suite, "-r", "oracle", "-r", "echo", "--seed", "7"], tmp_path)


def test_run_parity_repeats(tmp_path: Path) -> None:
    _compare(
        [
            "run",
            "suites/example.yaml",
            "-r",
            "oracle",
            "-r",
            "echo",
            "--seed",
            "3",
            "--repeats",
            "2",
        ],
        tmp_path,
    )


def test_list_and_help_smoke() -> None:
    for args in (["--help"], ["list", "suites"], ["list", "runners"]):
        proc = subprocess.run(
            [str(RUST_KM), *args],
            text=True,
            capture_output=True,
            timeout=60,
            cwd=ROOT,
            env={**os.environ, "COLUMNS": "250"},
        )
        assert proc.returncode == 0, (args, proc.stderr)
        assert proc.stdout.strip()


@pytest.mark.skipif(RUST_DESKTOP is None, reason="Rust km-desktop not built")
def test_desktop_bridge_free_run(tmp_path: Path) -> None:
    """The Rust desktop engine answers the same stdio protocol: ping, catalog,
    a confirmed free run, and a report whose grades match the CLI run."""
    env = {
        **os.environ,
        "MJ_RUNS_DIR": str(tmp_path / "runs"),
        "MJ_DB": str(tmp_path / "runs/results.db"),
        "MJ_SUITES_DIR": str(ROOT / "suites"),
        "KM_RESOURCE_ROOT": str(ROOT),
    }
    lines = "\n".join(
        json.dumps(request)
        for request in [
            {"id": "1", "method": "ping", "params": {}},
            {"id": "2", "method": "catalog", "params": {}},
            {
                "id": "3",
                "method": "estimate",
                "params": {
                    "suite": "example",
                    "runners": ["oracle", "echo"],
                    "max_usd": 1,
                    "repeats": 1,
                },
            },
            {
                "id": "4",
                "method": "run",
                "params": {
                    "suite": "example",
                    "runners": ["oracle", "echo"],
                    "max_usd": 1,
                    "repeats": 1,
                    "confirmed": True,
                    "seed": 0,
                },
            },
        ]
    )
    proc = subprocess.run(
        [str(RUST_DESKTOP)],
        input=lines + "\n",
        text=True,
        capture_output=True,
        timeout=300,
        env=env,
    )
    assert proc.returncode == 0, proc.stderr
    replies = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    by_id = {reply.get("id"): reply for reply in replies if "ok" in reply}
    assert by_id["1"]["result"]["engine"] == "kritrim_mulyankan"
    packs = by_id["2"]["result"]["packs"]
    assert {pack["id"] for pack in packs} >= {"example", "india-pilot", "railway-pilot"}
    assert by_id["3"]["result"]["ok"] is True
    assert by_id["3"]["result"]["paid"] is False
    run = by_id["4"]["result"]
    assert run["status"] == "completed"
    export = json.loads(Path(run["export_path"]).read_text(encoding="utf-8"))
    graded = {
        (row["task_id"], row["runner_id"]): row["passed"]
        for row in export["results"]
        if row["grade_status"] == "graded"
    }
    assert graded[("reasoning.bat-and-ball", "oracle")] is True
    assert graded[("reasoning.bat-and-ball", "echo")] is False
    assert Path(run["card_path"]).is_file()
    assert "/board/" in run["dashboard_url"]
    assert sys.version_info  # keep the import obvious for tooling
