from pathlib import Path

import httpx2
import pytest

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
