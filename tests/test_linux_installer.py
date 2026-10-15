"""Exercise the compiled offline setup using isolated user-data folders."""

import os
import runpy
import shutil
import subprocess
import tarfile
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
BUILD = runpy.run_path(str(ROOT / "desktop/linux-installer/build.py"))


@pytest.fixture(scope="session")
def setup_archive(tmp_path_factory):
    if not shutil.which("cc") or not shutil.which("pkg-config"):
        pytest.skip("A C compiler and GTK 3 development files are required")
    if subprocess.run(["pkg-config", "--exists", "gtk+-3.0"]).returncode:
        pytest.skip("GTK 3 development files are required")
    work = tmp_path_factory.mktemp("graphical-linux-setup")
    payload = work / "sample.AppImage"
    # Stand in for the downloaded AppImage and record precisely how setup opens it.
    payload.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$KM_TEST_LAUNCH"\n', encoding="utf-8")
    archive = BUILD["build"](payload, work / "setup.tar.gz", "0.3.0")
    return archive, payload.read_bytes()


@pytest.fixture
def installation(tmp_path, setup_archive):
    if os.geteuid() == 0:
        pytest.skip("The installer deliberately refuses root installations")
    archive, payload = setup_archive
    with tarfile.open(archive) as source:
        source.extractall(tmp_path / "download", filter="data")
    root = tmp_path / "download/Krutrim Mulyankan"
    data = tmp_path / "user data"
    home = tmp_path / "home"
    home.mkdir()
    env = {
        **os.environ,
        "HOME": str(home),
        "XDG_DATA_HOME": str(data),
        "KM_TEST_LAUNCH": str(tmp_path / "launched"),
    }
    setup = root / "Install Krutrim Mulyankan"

    def run(*args, check=True, executable=None):
        return subprocess.run(
            [str(executable or setup), *args], env=env, text=True, capture_output=True, check=check
        )

    return {
        "root": root,
        "data": data,
        "home": home,
        "env": env,
        "run": run,
        "setup": setup,
        "payload": payload,
        "launch": tmp_path / "launched",
    }


def installed_paths(installation):
    data = installation["data"]
    return (
        data / "krutrim-mulyankan/application.AppImage",
        data / "applications/in.krutrimmulyankan.evaluator.desktop",
        data / "icons/hicolor/256x256/apps/in.krutrimmulyankan.evaluator.png",
        data / "krutrim-mulyankan/installer",
    )


def test_archive_keeps_setup_executable_and_payload_nonexecutable(setup_archive):
    archive, _ = setup_archive
    with tarfile.open(archive) as source:
        setup = source.getmember("Krutrim Mulyankan/Install Krutrim Mulyankan")
        assert setup.mode == 0o755
        assert source.extractfile(setup).read(4) == b"\x7fELF"
        assert source.getmember("Krutrim Mulyankan/payload/application.AppImage").mode == 0o644


def test_installs_offline_registers_menu_and_retains_results_on_remove(installation):
    item = installation
    results = item["data"] / "krutrim-mulyankan/results/important.json"
    results.parent.mkdir(parents=True)
    results.write_text('{"saved":true}')
    keys = item["home"] / "keys-must-stay"
    keys.write_text("untouched")
    item["run"]("--install", "--no-launch")
    app, desktop, icon, manager = installed_paths(item)
    assert app.read_bytes() == item["payload"]
    assert app.stat().st_mode & 0o777 == 0o755
    assert manager.read_bytes().startswith(b"\x7fELF")
    assert icon.read_bytes().startswith(b"\x89PNG")
    assert "--appimage-extract-and-run" in desktop.read_text()
    assert "Terminal=false" in desktop.read_text()
    assert "Manage installation" in desktop.read_text()
    assert not item["launch"].exists()
    if shutil.which("desktop-file-validate"):
        subprocess.run(["desktop-file-validate", desktop], check=True)
    # The installed manager remains usable after deleting the downloaded setup.
    shutil.rmtree(item["root"])
    item["run"]("--uninstall", executable=manager)
    assert all(not path.exists() for path in (app, desktop, icon, manager))
    assert results.read_text() == '{"saved":true}'
    assert keys.read_text() == "untouched"


def test_payload_tampering_does_not_replace_previous_install(installation):
    item = installation
    item["run"]("--install", "--no-launch")
    app, desktop, icon, manager = installed_paths(item)
    previous = {path: path.read_bytes() for path in (app, desktop, icon, manager)}
    (item["root"] / "payload/application.AppImage").write_bytes(b"tampered payload")
    response = item["run"]("--install", "--no-launch", check=False)
    assert response.returncode == 1
    assert "integrity check" in response.stderr
    assert all(path.read_bytes() == value for path, value in previous.items())
    assert not list(item["data"].rglob("*.tmp-*"))


def test_icon_tampering_does_not_replace_previous_install(installation):
    item = installation
    item["run"]("--install", "--no-launch")
    before = {path: path.read_bytes() for path in installed_paths(item)}
    (item["root"] / "payload/icon.png").write_bytes(b"tampered icon")
    response = item["run"]("--install", "--no-launch", check=False)
    assert response.returncode == 1
    assert "integrity check" in response.stderr
    assert all(path.read_bytes() == value for path, value in before.items())


@pytest.fixture(scope="session")
def fail_final_rename(tmp_path_factory):
    """Inject a filesystem failure without exposing any test hooks in setup."""
    work = tmp_path_factory.mktemp("rename-failure")
    source = work / "fail-rename.c"
    source.write_text(
        r"""
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
int rename(const char *source, const char *destination) {
    const char *suffix = "/application.AppImage";
    size_t length = strlen(destination), suffix_length = strlen(suffix);
    if (getenv("KM_TEST_FAIL_ACTIVATE") && length >= suffix_length &&
        strcmp(destination + length - suffix_length, suffix) == 0) {
        errno = EIO;
        return -1;
    }
    int (*real_rename)(const char *, const char *) = dlsym(RTLD_NEXT, "rename");
    return real_rename(source, destination);
}
""",
        encoding="utf-8",
    )
    library = work / "fail-rename.so"
    subprocess.run(["cc", "-shared", "-fPIC", source, "-ldl", "-o", library], check=True)
    return library


@pytest.mark.parametrize("existing_install", [False, True])
def test_activation_failure_rolls_back_all_files(installation, fail_final_rename, existing_install):
    item = installation
    if existing_install:
        item["run"]("--install", "--no-launch")
    before = {
        path.relative_to(item["data"]): (path.read_bytes(), path.stat().st_mode)
        for path in item["data"].rglob("*")
        if path.is_file()
    }
    item["env"].update(LD_PRELOAD=str(fail_final_rename), KM_TEST_FAIL_ACTIVATE="1")
    response = item["run"]("--install", "--no-launch", check=False)
    assert response.returncode == 1
    assert "activate the installation" in response.stderr
    after = {
        path.relative_to(item["data"]): (path.read_bytes(), path.stat().st_mode)
        for path in item["data"].rglob("*")
        if path.is_file()
    }
    assert after == before


@pytest.mark.parametrize("target", ["app", "directory", "desktop", "icon"])
def test_rejects_destination_symlinks_without_touching_target(installation, target):
    item = installation
    app, desktop, icon, _ = installed_paths(item)
    victim = item["home"] / "preserve"
    victim.write_text("unchanged")
    path = {"app": app, "directory": app.parent, "desktop": desktop, "icon": icon}[target]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.symlink_to(victim)
    response = item["run"]("--install", "--no-launch", check=False)
    assert response.returncode == 1
    assert "symbolic link" in response.stderr
    assert victim.read_text() == "unchanged"


def test_refuses_to_remove_unmanaged_files(installation):
    item = installation
    app, desktop, icon, _ = installed_paths(item)
    for path in (app, desktop, icon):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("not owned by this setup")
    response = item["run"]("--uninstall", check=False)
    assert response.returncode == 1
    assert "does not own" in response.stderr
    assert all(path.read_text() == "not owned by this setup" for path in (app, desktop, icon))


def test_launch_uses_argv_and_fuse_free_mode_with_special_characters(installation):
    item = installation
    item["env"]["XDG_DATA_HOME"] = str(
        item["data"] / 'spaces % "quotes" $HOME `touch nope` \\ path'
    )
    item["run"]("--install")
    for _ in range(100):
        if item["launch"].exists():
            break
        time.sleep(0.01)
    assert item["launch"].read_text() == "--appimage-extract-and-run\n"
    data = Path(item["env"]["XDG_DATA_HOME"])
    desktop = data / "applications/in.krutrimmulyankan.evaluator.desktop"
    if shutil.which("desktop-file-validate"):
        subprocess.run(["desktop-file-validate", desktop], check=True)
    # GLib parses the generated desktop Exec and launches it as a real app menu would.
    if shutil.which("gio"):
        item["launch"].unlink()
        subprocess.run(
            ["gio", "launch", desktop], env=item["env"], check=True, capture_output=True, text=True
        )
        for _ in range(100):
            if item["launch"].exists():
                break
            time.sleep(0.01)
        assert item["launch"].read_text() == "--appimage-extract-and-run\n"


def test_root_account_is_refused(setup_archive, tmp_path):
    if os.geteuid() != 0:
        pytest.skip("This check only applies when the test runner is root")
    archive, _ = setup_archive
    with tarfile.open(archive) as source:
        source.extractall(tmp_path, filter="data")
    setup = tmp_path / "Krutrim Mulyankan/Install Krutrim Mulyankan"
    response = subprocess.run([setup, "--install", "--no-launch"], text=True, capture_output=True)
    assert response.returncode == 1
    assert "regular account" in response.stderr


def test_graphical_setup_stays_open_in_display_session(installation):
    """Run under xvfb-run in CI; ordinary headless test runs skip visual smoke."""
    if not os.environ.get("KM_TEST_GUI"):
        pytest.skip("Set KM_TEST_GUI=1 under Xvfb to exercise the native setup window")
    item = installation
    process = subprocess.Popen([item["setup"]], env=item["env"], stderr=subprocess.PIPE, text=True)
    try:
        time.sleep(2)
        assert process.poll() is None, process.stderr.read()
        screenshot = os.environ.get("KM_SETUP_SCREENSHOT")
        if screenshot and shutil.which("import"):
            subprocess.run(["import", "-window", "root", screenshot], check=True)
    finally:
        process.terminate()
        process.wait(timeout=10)
