"""The single-file `km` binary must find suites without a source checkout."""

import sys

from kritrim_mulyankan.settings import Settings


def test_checkout_still_uses_the_suites_directory():
    assert Settings.from_env({}).suites_dir.as_posix() == "suites"


def test_frozen_km_prefers_suites_beside_the_binary(monkeypatch, tmp_path):
    suites = tmp_path / "suites"
    suites.mkdir()
    binary = tmp_path / "km"
    binary.write_text("", encoding="utf-8")
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(binary))
    assert Settings.from_env({}).suites_dir == suites


def test_frozen_km_uses_suites_packed_inside_the_binary(monkeypatch, tmp_path):
    bundled = tmp_path / "mei" / "suites"
    bundled.mkdir(parents=True)
    binary_dir = tmp_path / "bin"
    binary_dir.mkdir()
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(binary_dir / "km"))
    monkeypatch.setattr(sys, "_MEIPASS", str(tmp_path / "mei"), raising=False)
    assert Settings.from_env({}).suites_dir == bundled


def test_suites_dir_override_wins_when_frozen(monkeypatch, tmp_path):
    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(tmp_path / "km"))
    chosen = tmp_path / "custom"
    assert Settings.from_env({"MJ_SUITES_DIR": str(chosen)}).suites_dir == chosen
