"""Unit checks for release archive layout and strict installer collection."""

import importlib.util
import subprocess
import tarfile
import zipfile
from pathlib import Path

import pytest

SPEC = importlib.util.spec_from_file_location(
    "package_native", Path(__file__).resolve().parents[1] / "scripts/package_native.py"
)
packaging = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packaging)


@pytest.mark.parametrize(
    "platform", ["linux-x86_64", "windows-x86_64", "macos-aarch64", "macos-x86_64"]
)
def test_cli_archive_layout(tmp_path, monkeypatch, platform):
    monkeypatch.setattr(packaging, "ROOT", tmp_path)
    windows = platform.startswith("windows")
    binary = "km.exe" if windows else "km"
    build = tmp_path / "km-rs/target"
    if platform == "linux-x86_64":
        build /= "x86_64-unknown-linux-musl"
    build /= "release"
    build.mkdir(parents=True)
    (build / binary).write_text("test binary")
    (build / binary).chmod(0o755)
    (tmp_path / "suites").mkdir()
    (tmp_path / "suites/example.yaml").write_text("test suite")
    (tmp_path / "LICENSE").write_text("test license")
    calls = []

    def checked(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, '{"id":"ci","ok":true}\n', "")

    monkeypatch.setattr(packaging, "checked", checked)
    packaging.package_cli(platform)
    output = tmp_path / "dist/native"
    if windows:
        with zipfile.ZipFile(output / f"km-{platform}.zip") as archive:
            names = archive.namelist()
    else:
        with tarfile.open(output / f"km-{platform}.tar.gz") as archive:
            names = archive.getnames()
            assert archive.getmember(binary).mode & 0o111
    assert {binary, "suites/example.yaml", "LICENSE"}.issubset(names)
    assert calls[1][1]["cwd"] != str(tmp_path)
    assert calls[1][0][-4:] == ["-r", "oracle", "-r", "echo"]


def test_missing_or_ambiguous_installer_is_rejected(tmp_path, monkeypatch):
    monkeypatch.setattr(packaging, "ROOT", tmp_path)
    with pytest.raises(AssertionError):
        packaging.collect_installers("windows-x86_64")
    bundle = tmp_path / "desktop/src-tauri/target/release/bundle/nsis"
    bundle.mkdir(parents=True)
    (bundle / "one.exe").write_bytes(b"one")
    (bundle / "two.exe").write_bytes(b"two")
    with pytest.raises(AssertionError):
        packaging.collect_installers("windows-x86_64")
