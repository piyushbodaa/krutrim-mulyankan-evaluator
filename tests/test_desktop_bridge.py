"""The desktop engine bridge: catalog, cost confirmation, key handling, and the results page."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import urllib.error
import urllib.request
from pathlib import Path

import pytest

from kritrim_mulyankan.desktop_bridge import DesktopSession, dispatch
from kritrim_mulyankan.engine import run_suite

REPO = Path(__file__).resolve().parent.parent
UI = REPO / "desktop" / "ui"
PREVIEW_INDEX = REPO / "preview" / "index.html"
SECRET = "sk-test-secret-value"


@pytest.fixture
def session(settings):
    desktop = DesktopSession(settings=settings, env={})
    yield desktop
    desktop.close()


def test_catalog_leads_with_the_pilot_and_hides_keys(session):
    session.set_keys({"OPENAI_API_KEY": SECRET})
    catalog = session.catalog()
    assert catalog["packs"][0]["id"] == "india-pilot"
    assert catalog["packs"][0]["recommended"] is True
    assert catalog["honesty"]["pilot"] == "Unvalidated pilot"
    assert catalog["honesty"]["ranking"] == "Not a ranking"
    assert "Too close to call" in catalog["honesty"]["close"]
    assert catalog["keys"]["openai"]["saved"] is True
    assert SECRET not in json.dumps(catalog)
    hosts = {item["signup"] for item in catalog["providers"] if item["signup"]}
    assert "https://platform.openai.com/api-keys" in hosts
    assert "https://console.anthropic.com/settings/keys" in hosts
    assert "https://aistudio.google.com/apikey" in hosts
    assert "https://console.x.ai/" in hosts


def test_keys_stay_in_memory_and_off_disk(session, tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    session.set_keys({"ANTHROPIC_API_KEY": SECRET})
    with pytest.raises(ValueError, match="Unknown key"):
        session.set_keys({"MJ_DB": SECRET})
    with pytest.raises(ValueError, match="line break"):
        session.set_keys({"OPENAI_API_KEY": "sk-\nsecret"})
    assert not (tmp_path / ".env").exists()
    assert SECRET not in json.dumps(session.key_status())
    estimate = session.estimate(
        suite="india-pilot",
        runners=["model:anthropic/claude-haiku-4-5"],
        max_usd=0,
    )
    assert estimate["paid"] is True
    assert estimate["ok"] is False
    assert SECRET not in json.dumps(estimate)
    problems = " ".join(estimate["plain_problems"]).lower()
    assert "cap" in problems or "spending" in problems


def test_unconfirmed_and_over_cap_runs_call_nothing(session, settings):
    with pytest.raises(ValueError, match="Confirm"):
        session.run(suite="india-pilot", runners=["echo", "oracle"], confirmed=False)
    session.set_keys({"ANTHROPIC_API_KEY": SECRET})
    with pytest.raises(ValueError, match="Nothing was called"):
        session.run(
            suite="india-pilot",
            runners=["model:anthropic/claude-haiku-4-5"],
            confirmed=True,
            max_usd=0,
        )
    assert not settings.db_path.exists()


def test_practice_run_feeds_the_existing_dashboard(session):
    before = PREVIEW_INDEX.read_bytes()
    result = session.run(
        suite="india-pilot",
        runners=["oracle", "echo"],
        confirmed=True,
        max_usd=1,
    )
    assert result["status"] == "completed"
    assert result["spent_usd"] == 0
    report = session.report(result["run_id"])
    assert report["verdicts"]
    assert {item["verdict"] for item in report["verdicts"]} & {
        "clearly_different",
        "too_close_to_call",
    }
    limitations = json.dumps(report["limitations"])
    assert "ranking" in limitations
    assert "validated" in limitations
    card = session.share_card(result["run_id"])
    for label in ("Unvalidated pilot", "Not a ranking", "Too close to call"):
        assert label in card["html"]
        assert label in card["summary"] or label == "Too close to call"
    assert "Synthetic smoke fixture" not in card["html"]
    assert "total_inr" not in card["html"]
    assert SECRET not in card["html"]
    url = session.dashboard_url(result["run_id"], theme="dark")
    with urllib.request.urlopen(url, timeout=5) as response:
        html = response.read().decode()
    assert PREVIEW_INDEX.read_bytes() == before
    assert "dashboard-shim.js" in html
    assert "__KM_DASHBOARD__" in html
    assert "km.theme.v1" in html
    assert "Not a ranking" in html
    style = urllib.request.urlopen(url.split("?")[0] + "style.css", timeout=5)
    assert b"Sora" in style.read()
    wrong = url.replace(session._token, "x" * len(session._token))
    with pytest.raises(urllib.error.HTTPError) as caught:
        urllib.request.urlopen(wrong, timeout=5)
    assert caught.value.code == 403


def test_cancel_stops_the_active_run(monkeypatch, session):
    release = threading.Event()
    entered = threading.Event()

    def blocking(*args, **kwargs):
        entered.set()
        assert release.wait(5)
        return run_suite(*args, **kwargs)

    monkeypatch.setattr("kritrim_mulyankan.desktop_bridge.run_suite", blocking)
    holder: dict = {}

    def go():
        try:
            holder["result"] = session.run(
                suite="example", runners=["echo"], confirmed=True, max_usd=1
            )
        except Exception as exc:  # pragma: no cover - assertion below
            holder["error"] = exc

    thread = threading.Thread(target=go)
    thread.start()
    assert entered.wait(5)
    assert session.cancel()["cancelling"] is True
    release.set()
    thread.join(10)
    assert "error" not in holder
    assert holder["result"]["status"] == "cancelled"


def test_dispatch_redacts_key_material(session):
    messages = []

    def emit(message):
        messages.append(message)

    dispatch(
        session,
        {"id": "k", "method": "set_keys", "params": {"keys": {"OPENAI_API_KEY": SECRET}}},
        emit,
    )
    dispatch(session, {"id": "c", "method": "catalog"}, emit)
    blob = json.dumps(messages)
    assert SECRET not in blob
    assert messages[0]["ok"] is True
    assert messages[1]["result"]["packs"][0]["id"] == "india-pilot"


def _read_json(proc, timeout=15):
    holder: dict = {}

    def target():
        holder["line"] = proc.stdout.readline()

    thread = threading.Thread(target=target)
    thread.start()
    thread.join(timeout)
    if thread.is_alive():
        proc.kill()
        raise AssertionError(proc.stderr.read() if proc.stderr else "engine timed out")
    return json.loads(holder["line"])


def test_stdio_protocol_runs_a_practice_check(tmp_path):
    env = {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(tmp_path),
        "KM_RESOURCE_ROOT": str(REPO),
        "KM_UI_ROOT": str(UI),
        "MJ_RUNS_DIR": str(tmp_path / "runs"),
        "MJ_DB": str(tmp_path / "runs" / "results.db"),
        "MJ_SUITES_DIR": str(REPO / "suites"),
        "PYTHONPATH": str(REPO / "src"),
        "PYTHONUNBUFFERED": "1",
    }
    proc = subprocess.Popen(
        [sys.executable, "-m", "kritrim_mulyankan.desktop_bridge"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        cwd=tmp_path,
    )
    try:
        assert proc.stdin is not None
        proc.stdin.write(json.dumps({"id": "1", "method": "ping"}) + "\n")
        proc.stdin.write(
            json.dumps(
                {"id": "2", "method": "set_keys", "params": {"keys": {"XAI_API_KEY": SECRET}}}
            )
            + "\n"
        )
        proc.stdin.write(
            json.dumps(
                {
                    "id": "3",
                    "method": "run",
                    "params": {
                        "suite": "india-pilot",
                        "runners": ["oracle", "echo"],
                        "confirmed": True,
                        "max_usd": 1,
                    },
                }
            )
            + "\n"
        )
        proc.stdin.flush()
        ping = _read_json(proc)
        assert ping["result"]["honesty"]["ranking"] == "Not a ranking"
        saved = _read_json(proc)
        assert saved["ok"] is True and SECRET not in json.dumps(saved)
        progress = []
        final = None
        while final is None:
            message = _read_json(proc)
            if message.get("event") == "progress":
                progress.append(message["result"])
            else:
                final = message
        assert final["ok"] is True
        assert final["result"]["status"] == "completed"
        assert any(item.get("type") == "attempt" for item in progress)
        assert SECRET not in json.dumps(final)
        assert not (tmp_path / ".env").exists()
    finally:
        proc.kill()
        stderr = proc.stderr.read() if proc.stderr else ""
        assert SECRET not in stderr


def test_shell_keeps_honesty_labels_bilingual_copy_and_the_logo():
    html = (UI / "index.html").read_text()
    script = (UI / "app.js").read_text() + (UI / "i18n.js").read_text()
    for label in ("Unvalidated pilot", "Not a ranking", "Too close to call"):
        assert label in html
    assert 'data-lang="hi"' in html
    assert "km.desktop.lang" in script
    assert "https://platform.openai.com/api-keys" not in html  # links come from the engine catalog
    for name in (
        "km-chakra-wordmark.svg",
        "km-chakra-wordmark-white.svg",
        "favicon.ico",
        "favicon-32.png",
        "fonts/Sora.woff2",
        "fonts/NotoSansDevanagari-Regular.woff2",
    ):
        copied = (UI / "brand" / name).read_bytes()
        original = (REPO / "preview" / "brand" / name).read_bytes()
        assert copied == original
    assert 'class="logo-on-light" src="brand/km-chakra-wordmark.svg"' in html
    assert 'class="logo-on-dark" src="brand/km-chakra-wordmark-white.svg"' in html
