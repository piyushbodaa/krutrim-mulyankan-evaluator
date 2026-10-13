from pathlib import Path

import httpx2
import pytest
from typer.testing import CliRunner

from kritrim_mulyankan.cli import app
from kritrim_mulyankan.settings import Settings
from kritrim_mulyankan.store import Store
from kritrim_mulyankan.suite import Suite, load_suite

REPO = Path(__file__).resolve().parent.parent
SUITES_DIR = REPO / "suites"


@pytest.fixture
def example_suite() -> Suite:
    return load_suite(SUITES_DIR / "example.yaml")


@pytest.fixture
def settings(tmp_path: Path) -> Settings:
    runs = tmp_path / "runs"
    return Settings(runs_dir=runs, db_path=runs / "results.db", suites_dir=SUITES_DIR)


@pytest.fixture
def store(settings: Settings):
    with Store(settings.db_path) as s:
        yield s


@pytest.fixture
def suites_dir() -> Path:
    return SUITES_DIR


@pytest.fixture(autouse=True)
def no_network(monkeypatch):
    """Tests must never reach a real API (and never spend money), even with keys in the env.

    Only httpx2.MockTransport (used explicitly by tests) can answer requests.
    """

    def refuse(self, request):
        raise RuntimeError(f"network disabled in tests: {request.method} {request.url}")

    monkeypatch.setattr(httpx2.HTTPTransport, "handle_request", refuse)


@pytest.fixture
def km(tmp_path, suites_dir, monkeypatch):
    monkeypatch.chdir(tmp_path)
    base_env = {
        "MJ_RUNS_DIR": str(tmp_path / "runs"),
        "MJ_DB": str(tmp_path / "runs" / "results.db"),
        "MJ_SUITES_DIR": str(suites_dir),
        "MJ_PRICES": str(tmp_path / "prices.yaml"),
        "COLUMNS": "250",
        "TERM": "xterm-256color",  # Rich forces 80 columns when TERM=dumb.
        # Hide any real credentials so results don't depend on the developer's machine.
        "XDG_CONFIG_HOME": str(tmp_path / "config"),
        **{k: None for k in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_PROFILE",
                             "OPENAI_API_KEY", "XAI_API_KEY", "GEMINI_API_KEY",
                             "OPENROUTER_API_KEY")},
    }  # fmt: skip
    runner = CliRunner()

    def invoke(*args: str, env: dict | None = None):
        return runner.invoke(app, list(args), env={**base_env, **(env or {})})

    invoke.tmp_path = tmp_path
    return invoke
