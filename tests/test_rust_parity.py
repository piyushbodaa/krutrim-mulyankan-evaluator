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
import threading
from dataclasses import replace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import httpx2
import pytest
from typer.testing import CliRunner

from kritrim_mulyankan.cli import app
from kritrim_mulyankan.runners import providers as py_providers

_REAL_HANDLE = httpx2.HTTPTransport.handle_request

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
    # The Python monkeypatch points the SDK at the mock, so it records the
    # loopback URL; the Rust KM_* test hook keeps the canonical endpoint.
    for payload in (py_results, rs_results):
        for model in payload["models"]:
            model["endpoint"] = "<mock>"
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


# ---- model-runner parity against one loopback mock provider server ----


MOCK_PROVIDERS = [
    "anthropic",
    "openai",
    "xai",
    "gemini",
    "openrouter",
    "ollama",
    "vllm",
    "compatible",
]

KEY_ENVS = {
    "anthropic": "ANTHROPIC_API_KEY",
    "openai": "OPENAI_API_KEY",
    "xai": "XAI_API_KEY",
    "gemini": "GEMINI_API_KEY",
    "openrouter": "OPENROUTER_API_KEY",
    "compatible": "COMPATIBLE_API_KEY",
}


class _MockProvider:
    def __init__(self) -> None:
        self.requests: list[tuple[str, dict]] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # noqa: D401 - keep test output clean
                pass

            def do_POST(self):
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
                outer.requests.append((self.path, body))
                if self.path.endswith("/v1/messages"):
                    payload = {
                        "id": "msg_test",
                        "type": "message",
                        "role": "assistant",
                        "model": "test-model",
                        "content": [{"type": "text", "text": "5"}],
                        "stop_reason": "end_turn",
                        "usage": {
                            "input_tokens": 100,
                            "output_tokens": 50,
                            "cache_read_input_tokens": 10,
                            "cache_creation_input_tokens": 5,
                        },
                    }
                else:
                    payload = {
                        "id": "chatcmpl-test",
                        "object": "chat.completion",
                        "created": 1,
                        "model": "test-model",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": "5"},
                                "finish_reason": "stop",
                            }
                        ],
                        "usage": {
                            "prompt_tokens": 120,
                            "completion_tokens": 30,
                            "total_tokens": 150,
                            "prompt_tokens_details": {"cached_tokens": 20},
                        },
                    }
                data = json.dumps(payload).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()
        return False


@pytest.mark.parametrize("provider", MOCK_PROVIDERS)
def test_model_runner_parity(provider: str, tmp_path: Path, monkeypatch) -> None:
    """Same mock endpoint, same canned usage: both kms must record the same
    output, token usage and USD cost, and send the same request shape."""

    def loopback_only(self, request):
        # conftest disables all network in tests; this parity test talks to a
        # loopback mock only, so re-enable 127.0.0.1 and keep refusing the rest.
        if request.url.host != "127.0.0.1":
            raise RuntimeError(f"network disabled in tests: {request.method} {request.url}")
        return _REAL_HANDLE(self, request)

    monkeypatch.setattr(httpx2.HTTPTransport, "handle_request", loopback_only)
    prices = tmp_path / "prices.yaml"
    blocks = []
    for name in MOCK_PROVIDERS:
        blocks.append(
            f"{name}:\n  source: https://example.invalid/pricing\n"
            '  as_of: "2026-10-14"\n  models:\n'
            "    test-model: {input: 4.00, output: 20.00, cache_read: 0.20, cache_write: 5.00}\n"
        )
    prices.write_text("\n".join(blocks), encoding="utf-8")

    with _MockProvider() as mock:
        base = f"http://127.0.0.1:{mock.port}/v1"
        env = {
            "MJ_PRICES": str(prices),
            "NO_PROXY": "127.0.0.1",
            "no_proxy": "127.0.0.1",
            f"KM_{provider.upper()}_BASE_URL": base,
            "COMPATIBLE_BASE_URL": base,
            "OLLAMA_BASE_URL": base,
            "VLLM_BASE_URL": base,
        }
        if provider in KEY_ENVS:
            env[KEY_ENVS[provider]] = "test-key"
        if provider == "anthropic":
            # Anthropic bases carry no /v1 suffix; both sides append it.
            env["ANTHROPIC_BASE_URL"] = f"http://127.0.0.1:{mock.port}"
            env["KM_ANTHROPIC_BASE_URL"] = f"http://127.0.0.1:{mock.port}"
        # Python cloud providers pin their base URL in code; point the
        # in-process copy at the mock for this test only.
        original = py_providers.PROVIDERS[provider]
        if provider == "anthropic":
            pass  # the Anthropic SDK honours ANTHROPIC_BASE_URL
        elif provider not in {"ollama", "vllm", "compatible"}:
            monkeypatch.setitem(py_providers.PROVIDERS, provider, replace(original, base_url=base))
        args = [
            "run",
            "suites/example.yaml",
            "-r",
            f"model:{provider}/test-model",
            "--seed",
            "7",
            "--allow-paid",
            "--max-usd",
            "1",
        ]
        py_dir = _one_run_dir(_run_python(args, tmp_path / "py", env))
        py_requests = list(mock.requests)
        mock.requests.clear()
        rs_dir = _one_run_dir(_run_rust(args, tmp_path / "rs", env))
        rs_requests = list(mock.requests)

    py_results = json.loads((py_dir / "results.json").read_text(encoding="utf-8"))
    rs_results = json.loads((rs_dir / "results.json").read_text(encoding="utf-8"))
    # The Python monkeypatch points the SDK at the mock, so it records the
    # loopback URL; the Rust KM_* test hook keeps the canonical endpoint.
    for payload in (py_results, rs_results):
        for model in payload["models"]:
            model["endpoint"] = "<mock>"
    assert _export(rs_results) == _export(py_results)

    def rows(payload):
        return {
            row["task_id"]: (
                row["output"],
                row["usage"]["input_tokens"],
                row["usage"]["output_tokens"],
                row["cost"]["usd"],
                row["passed"],
            )
            for row in payload["results"]
        }

    assert rows(rs_results) == rows(py_results)

    def shapes(requests):
        normalized = []
        for path, body in requests:
            body = dict(body)
            body.pop("stream", None)  # SDKs may send an explicit default
            normalized.append((path, body))
        return sorted(normalized, key=lambda item: json.dumps(item, sort_keys=True))

    assert shapes(rs_requests) == shapes(py_requests)
