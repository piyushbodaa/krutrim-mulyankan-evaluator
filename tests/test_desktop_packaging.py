"""Guards for desktop packaging choices that are easy to lose in an edit.

These read the committed config/UI/workflow files. They do not build
anything; the desktop workflow does the real building.
"""

import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


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


def test_release_publishes_a_deb_with_the_website_filename():
    workflow = _read(".github/workflows/release.yml")
    assert "--bundles appimage,deb" in workflow
    assert "Krutrim-Mulyankan-linux.deb" in workflow
    assert "test -f release/Krutrim-Mulyankan-linux.deb" in workflow
    conf = json.loads(_read("desktop/src-tauri/tauri.conf.json"))
    assert conf["bundle"]["shortDescription"]


def test_rust_engine_is_built_for_cli_and_desktop_without_python():
    # The deployed binaries are Rust now (km-rs/). The Python package/wheel
    # stays, but no PyInstaller engine may creep back into the installers.
    release = _read(".github/workflows/release.yml")
    desktop = _read(".github/workflows/desktop.yml")
    for workflow in (release, desktop):
        assert "cargo build --release" in workflow
        assert "km-desktop-engine" in workflow
        assert "PyInstaller" not in workflow
        assert "resources/engine/preview" in workflow
        assert "resources/engine/suites" in workflow
    # The CLI release keeps install.sh's asset name and layout, now static.
    assert "x86_64-unknown-linux-musl" in release
    assert "tar -C dist/km-linux -czf km-linux-x86_64.tar.gz km suites" in release
    workspace = _read("km-rs/Cargo.toml")
    assert 'name = "km"' in _read("km-rs/crates/km/Cargo.toml")
    assert 'name = "km-desktop"' in _read("km-rs/crates/km-desktop/Cargo.toml")
    assert "crates/evaluator" in workspace
