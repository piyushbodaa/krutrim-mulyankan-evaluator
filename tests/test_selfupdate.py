"""km updates itself only with releases signed by the project's update key."""

from pathlib import Path

import pytest
from typer.testing import CliRunner

from kritrim_mulyankan import minisign, selfupdate
from kritrim_mulyankan.cli import app

FIXTURES = Path(__file__).resolve().parents[1] / "km-rs/crates/km/tests/fixtures"
SAMPLE = b"abc  km-linux-x86_64.tar.gz\n"


def test_signature_from_the_release_key_is_accepted_and_tampering_is_not():
    signature = (FIXTURES / "sample-SHA256SUMS.sig").read_text()
    assert minisign.verify(SAMPLE, signature, selfupdate.PUBLIC_KEY)
    assert not minisign.verify(SAMPLE.replace(b"abc", b"abd"), signature, selfupdate.PUBLIC_KEY)
    assert not minisign.verify(SAMPLE, "not a signature", selfupdate.PUBLIC_KEY)
    other_key = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"
    assert not minisign.verify(SAMPLE, signature, other_key)


def test_rust_and_python_km_trust_the_same_key_as_the_desktop_app():
    rust = (Path(__file__).resolve().parents[1] / "km-rs/crates/km/src/update.rs").read_text()
    assert f'"{selfupdate.PUBLIC_KEY}"' in rust
    import base64
    import json

    conf = json.loads(
        (Path(__file__).resolve().parents[1] / "desktop/src-tauri/tauri.conf.json").read_text()
    )
    desktop_key = base64.b64decode(conf["plugins"]["updater"]["pubkey"]).decode().splitlines()[1]
    assert desktop_key == selfupdate.PUBLIC_KEY


def test_versions_and_checksum_lists():
    assert selfupdate.is_newer("0.4.10", "0.4.9")
    assert not selfupdate.is_newer("0.4.1", "0.4.1")
    sums = "aa  kritrim_mulyankan-0.5.0-py3-none-any.whl\n"
    assert (
        selfupdate.listed_digest(sums, "kritrim_mulyankan-0.5.0-py3-none-any.whl", "0.5.0") == "aa"
    )
    with pytest.raises(selfupdate.UpdateError):
        selfupdate.listed_digest(sums, "kritrim_mulyankan-0.6.0-py3-none-any.whl", "0.6.0")


def test_automatic_update_stays_off_in_ci_and_development(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(selfupdate, "update_now", lambda timeout: calls.append(timeout))
    monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
    monkeypatch.setenv("CI", "true")
    selfupdate.auto()
    monkeypatch.delenv("CI")
    monkeypatch.setattr(selfupdate, "_development_install", lambda: True)
    selfupdate.auto()
    assert calls == []
    monkeypatch.setattr(selfupdate, "_development_install", lambda: False)
    selfupdate.auto()
    selfupdate.auto()  # same day: no second check
    assert calls == [4]


def test_version_flag_and_command():
    for args in (["--version"], ["version"]):
        result = CliRunner().invoke(app, args)
        assert result.exit_code == 0
        assert result.output.strip() == f"km {selfupdate.current()}"
