"""Guards for desktop packaging choices that are easy to lose in an edit.

These read the committed config/UI/workflow files. They do not build
anything; the desktop workflow does the real building.
"""

import hashlib
import json
import runpy
import tomllib
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
PACKAGING = runpy.run_path(str(ROOT / "scripts/package_downloads.py"))


def _read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_api_key_cards_cannot_overflow_their_row():
    css = _read("desktop/ui/app.css")
    # The password field must be allowed to shrink below its default size,
    # or it pushes the Save button off the right edge of the card.
    assert ".key-row input{" in css
    assert "min-width:0" in css.split(".key-row input{", 1)[1].split("}", 1)[0]
    assert ".key-row button{flex:0 0 auto" in css
    # Grid items default to min-width:auto and can overflow their track.
    assert ".card{" in css and "min-width:0" in css.split(".card{", 1)[1].split("}", 1)[0]
    assert "minmax(min(100%,280px),1fr)" in css
    # A long provider name must not push the "Get a key" link out either.
    assert ".provider-head{display:flex;flex-wrap:wrap;" in css


def test_keyring_uses_a_real_platform_store_not_the_mock():
    cargo = tomllib.loads(_read("desktop/src-tauri/Cargo.toml"))
    # keyring 3 has no default store: no platform feature means the mock
    # store, and saved keys are never persisted. Keep a real store per OS.
    linux = cargo["target"]['cfg(target_os = "linux")']["dependencies"]["keyring"]
    assert "linux-native-sync-persistent" in linux["features"]
    assert "crypto-rust" in linux["features"]
    windows = cargo["target"]['cfg(target_os = "windows")']["dependencies"]["keyring"]
    assert "windows-native" in windows["features"]
    macos = cargo["target"]['cfg(target_os = "macos")']["dependencies"]["keyring"]
    assert "apple-native" in macos["features"]
    assert "keyring" not in cargo["dependencies"]


def test_ci_and_release_use_the_same_native_download_builds():
    for filename in ("desktop.yml", "release.yml"):
        workflow = yaml.safe_load(_read(f".github/workflows/{filename}"))
        assert workflow["jobs"]["downloads"]["uses"] == "./.github/workflows/build-downloads.yml"
        assert workflow["jobs"]["downloads"]["needs"] == "test"
    release = yaml.safe_load(_read(".github/workflows/release.yml"))
    assert set(release["jobs"]["publish"]["needs"]) == {"wheel", "downloads"}
    conf = json.loads(_read("desktop/src-tauri/tauri.conf.json"))
    assert conf["bundle"]["shortDescription"]


def test_rust_engine_is_built_for_cli_and_desktop_without_python():
    # The deployed binaries are Rust now (km-rs/). The Python package/wheel
    # stays, but no PyInstaller engine may creep back into the installers.
    workflow = yaml.safe_load(_read(".github/workflows/build-downloads.yml"))
    commands = "\n".join(step.get("run", "") for step in workflow["jobs"]["build"]["steps"])
    assert "cargo build --locked --release" in commands
    assert "-p km-cli" in commands and "-p km-desktop-engine" in commands
    assert "cargo test --locked" in commands
    assert "npm ci" in commands
    assert "PyInstaller" not in commands
    workspace = tomllib.loads(_read("km-rs/Cargo.toml"))
    assert "crates/evaluator" in workspace["workspace"]["members"]
    cli = tomllib.loads(_read("km-rs/crates/km/Cargo.toml"))
    engine = tomllib.loads(_read("km-rs/crates/km-desktop/Cargo.toml"))
    assert cli["bin"][0]["name"] == "km"
    assert engine["bin"][0]["name"] == "km-desktop"


def test_native_matrix_uses_matching_architecture_and_installer_formats():
    workflow = yaml.safe_load(_read(".github/workflows/build-downloads.yml"))
    rows = workflow["jobs"]["build"]["strategy"]["matrix"]["include"]
    actual = {
        (row["platform"], row["os"], row["cli-target"], row["desktop-target"], row["bundles"])
        for row in rows
    }
    assert actual == {
        (
            "linux-x86_64",
            "ubuntu-22.04",
            "x86_64-unknown-linux-musl",
            "x86_64-unknown-linux-gnu",
            "appimage,deb",
        ),
        (
            "windows-x86_64",
            "windows-2022",
            "x86_64-pc-windows-msvc",
            "x86_64-pc-windows-msvc",
            "nsis",
        ),
        (
            "macos-aarch64",
            "macos-15",
            "aarch64-apple-darwin",
            "aarch64-apple-darwin",
            "dmg",
        ),
        (
            "macos-x86_64",
            "macos-15-intel",
            "x86_64-apple-darwin",
            "x86_64-apple-darwin",
            "dmg",
        ),
    }


def test_release_assets_match_the_download_contract():
    expected = {
        "Krutrim-Mulyankan-linux.AppImage",
        "Krutrim-Mulyankan-linux.deb",
        "km-desktop-linux.tar.gz",
        "Krutrim-Mulyankan-windows.exe",
        "Krutrim-Mulyankan-mac-arm64.dmg",
        "Krutrim-Mulyankan-mac-x86_64.dmg",
        "km-linux-x86_64.tar.gz",
        "km-windows-x86_64.zip",
        "km-macos-aarch64.tar.gz",
        "km-macos-x86_64.tar.gz",
    }
    assert PACKAGING["ASSETS"] == expected
    release = yaml.safe_load(_read(".github/workflows/release.yml"))
    steps = release["jobs"]["publish"]["steps"]
    publish = next(step for step in steps if step.get("uses", "").startswith("softprops/"))
    files = {Path(path).name for path in publish["with"]["files"].splitlines()}
    assert files == expected | {"kritrim_mulyankan-*.whl", "SHA256SUMS"}


def test_release_verification_rejects_incomplete_downloads_and_hashes_all_assets(tmp_path):
    expected = PACKAGING["ASSETS"] | {"kritrim_mulyankan-1.2.3-py3-none-any.whl"}
    verify = PACKAGING["verify_release"]
    with pytest.raises(RuntimeError, match="missing"):
        verify(tmp_path, "1.2.3")
    for name in expected:
        (tmp_path / name).write_bytes(name.encode())
    extra = tmp_path / "unexpected.exe"
    extra.write_bytes(b"unexpected")
    with pytest.raises(RuntimeError, match="extra"):
        verify(tmp_path, "1.2.3")
    extra.unlink()
    asset = tmp_path / "km-windows-x86_64.zip"
    asset.write_bytes(b"")
    with pytest.raises(RuntimeError, match="Empty"):
        verify(tmp_path, "1.2.3")
    asset.write_bytes(asset.name.encode())
    verify(tmp_path, "1.2.3")
    actual = (tmp_path / "SHA256SUMS").read_text().splitlines()
    assert actual == [
        f"{hashlib.sha256(name.encode()).hexdigest()}  {name}" for name in sorted(expected)
    ]


def test_release_versions_stay_in_sync():
    version = tomllib.loads(_read("pyproject.toml"))["project"]["version"]
    PACKAGING["check_versions"](version)
    npm_lock = json.loads(_read("desktop/package-lock.json"))
    assert npm_lock["version"] == version
    assert npm_lock["packages"][""]["version"] == version
