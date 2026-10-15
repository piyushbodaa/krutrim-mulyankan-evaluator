#!/usr/bin/env python3
"""Open the real release app under Xvfb and remove its isolated installation.

Run as a regular Ubuntu CI user with xdotool, ImageMagick, desktop-file-utils,
and gio installed. This script accepts an existing archive and makes no downloads.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import shutil
import signal
import subprocess
import tarfile
import tempfile
import time
from pathlib import Path

APP_ID = "in.krutrimmulyankan.evaluator"
TITLE = "Krutrim Mulyankan"


def isolated_environment(root: Path) -> dict[str, str]:
    env = {
        key: os.environ[key]
        for key in ("PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "DISPLAY", "XAUTHORITY")
        if key in os.environ
    }
    # An allowlist excludes every provider credential and the caller's session bus.
    for key, name in (
        ("HOME", "home"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
    ):
        folder = root / name
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    env.update(
        GDK_BACKEND="x11",
        GIO_USE_VFS="local",
        NO_AT_BRIDGE="1",
        LIBGL_ALWAYS_SOFTWARE="1",
    )
    return env


class Smoke:
    def __init__(self, env: dict[str, str]):
        self.env = env
        self.window_pid: int | None = None

    def run(self, *args: str | Path, timeout: float = 15, check: bool = True, capture: bool = True):
        return subprocess.run(
            [str(arg) for arg in args],
            env=self.env,
            cwd=self.env["HOME"],
            text=True,
            stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
            stderr=subprocess.PIPE if capture else subprocess.DEVNULL,
            timeout=timeout,
            check=check,
        )

    def owns_pid(self, pid: int) -> bool:
        try:
            values = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            return False
        return f"HOME={self.env['HOME']}".encode() in values

    def find_window(self, visible: bool = True) -> str | None:
        args = ["xdotool", "search"]
        if visible:
            args.append("--onlyvisible")
        found = self.run(*args, "--name", f"^{TITLE}$", timeout=3, check=False)
        if found.returncode not in (0, 1):
            raise RuntimeError(f"Window search failed: {found.stderr}")
        for window in found.stdout.splitlines():
            name = self.run("xdotool", "getwindowname", window, check=False)
            owner = self.run("xdotool", "getwindowpid", window, check=False)
            if name.returncode or owner.returncode or name.stdout.strip() != TITLE:
                continue
            pid = int(owner.stdout.strip())
            if self.owns_pid(pid):
                self.window_pid = pid
                return window
        return None

    def wait_for_window(self) -> str:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if window := self.find_window():
                return window
            time.sleep(0.25)
        raise TimeoutError(f"No visible {TITLE!r} window appeared within 60 seconds")

    def capture(self, destination: Path) -> None:
        window = self.wait_for_window()
        # Allow the real WebKit frontend to paint after GTK maps its window.
        time.sleep(2)
        self.run("import", "-window", window, destination, timeout=20)
        if not destination.is_file() or destination.stat().st_size < 1024:
            raise AssertionError(f"Missing or empty app screenshot: {destination}")

    def stop_window(self) -> None:
        pid = self.window_pid
        if pid is None:
            # A failed startup can leave a mapped but hidden app window.
            self.find_window(visible=False)
            pid = self.window_pid
        if pid is None or not self.owns_pid(pid):
            self.window_pid = None
            return
        with contextlib.suppress(ProcessLookupError):
            os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 10
        while self.owns_pid(pid):
            if time.monotonic() >= deadline:
                raise TimeoutError(f"App window PID {pid} did not stop after SIGTERM")
            time.sleep(0.1)
        self.window_pid = None


def smoke(archive: Path, screenshots: Path) -> None:
    if os.geteuid() == 0:
        raise RuntimeError("Run this smoke test as a regular user, not root")
    if not os.environ.get("DISPLAY"):
        raise RuntimeError("DISPLAY is missing; run this test under xvfb-run")
    for command in ("xdotool", "import", "gio", "desktop-file-validate"):
        if shutil.which(command) is None:
            raise RuntimeError(f"Required smoke-test command is missing: {command}")
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="km-release-smoke-") as temporary:
        root = Path(temporary)
        env = isolated_environment(root)
        runner = Smoke(env)
        download = root / "download"
        with tarfile.open(archive) as source:
            if any(not (member.isdir() or member.isfile()) for member in source.getmembers()):
                raise ValueError("Setup archive must contain only regular files and folders")
            source.extractall(download, filter="data")
        setup = download / TITLE / "Install Krutrim Mulyankan"
        if not setup.is_file() or not os.access(setup, os.X_OK):
            raise AssertionError("Extracted setup is not executable without changing permissions")
        with setup.open("rb") as source:
            if source.read(4) != b"\x7fELF":
                raise AssertionError("Extracted graphical setup is not an ELF executable")
        data = Path(env["XDG_DATA_HOME"])
        app_dir = data / "krutrim-mulyankan"
        app = app_dir / "application.AppImage"
        manager = app_dir / "installer"
        desktop = data / "applications" / f"{APP_ID}.desktop"
        icon = data / "icons/hicolor/256x256/apps" / f"{APP_ID}.png"
        marker = app_dir / "results" / "retained-smoke-result.json"
        marker.parent.mkdir(parents=True)
        marker.write_text('{"retain": true}\n', encoding="utf-8")
        removed = False
        try:
            # The CLI invokes the same install/open actions as the GTK button.
            runner.run(setup, "--install", timeout=60, capture=False)
            runner.run("desktop-file-validate", desktop)
            entry = desktop.read_text(encoding="utf-8")
            if "--appimage-extract-and-run" not in entry or "Terminal=false" not in entry:
                raise AssertionError("Installed menu entry does not launch without FUSE/terminal")
            if not all(path.is_file() for path in (app, manager, desktop, icon)):
                raise AssertionError("Setup did not register all installation files")
            runner.capture(screenshots / "linux-setup-open.png")
            runner.stop_window()
            # Launch the actual installed menu entry, independently of setup.
            # Do not give the detached GUI child a captured pipe to keep open.
            runner.run("gio", "launch", desktop, timeout=20, capture=False)
            runner.capture(screenshots / "linux-menu-open.png")
            runner.stop_window()
            shutil.rmtree(download)
            runner.run(manager, "--uninstall", timeout=30)
            removed = True
            installed = (app, manager, desktop, icon, app_dir / "setup-owner")
            if any(path.exists() for path in installed):
                raise AssertionError("Removal left installed app or menu files behind")
            if marker.read_text(encoding="utf-8") != '{"retain": true}\n':
                raise AssertionError("Removal changed pre-existing result data")
        finally:
            try:
                runner.stop_window()
            finally:
                if not removed and manager.is_file():
                    runner.run(manager, "--uninstall", timeout=30, check=False)
    print(json.dumps({"archive": str(archive), "screenshots": str(screenshots), "passed": True}))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path, required=True)
    args = parser.parse_args()
    smoke(args.archive.resolve(strict=True), args.screenshots.resolve())


if __name__ == "__main__":
    main()
