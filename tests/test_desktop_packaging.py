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


def test_engine_builds_stay_slim_and_keep_cli_pygments():
    release = _read(".github/workflows/release.yml")
    desktop = _read(".github/workflows/desktop.yml")
    for workflow in (release, desktop):
        assert "--strip" in workflow
        assert "--exclude-module pygments" in workflow
        assert "--exclude-module setuptools" in workflow
    # The km CLI is the other PyInstaller build: Typer's rich help imports
    # pygments, so only the desktop bridge may exclude it. The desktop
    # exclusion is the one whose entry is desktop/sidecar/entry.py.
    cli_block = release.split("desktop/sidecar/km_entry.py")[0].rsplit("PyInstaller", 1)[1]
    assert "pygments" not in cli_block
