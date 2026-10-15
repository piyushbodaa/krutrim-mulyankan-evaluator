#!/usr/bin/env python3
"""Build the offline, graphical Linux setup archive with a pinned payload hash."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ARCHIVE_ROOT = "Krutrim Mulyankan"
SETUP_NAME = "Install Krutrim Mulyankan"


def build(appimage: Path, output: Path, version: str, icon: Path | None = None) -> Path:
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Expected a stable semantic version, for example 0.3.0")
    if icon is None:
        icon = HERE.parent / "src-tauri/icons/128x128@2x.png"
    appimage, icon, output = appimage.resolve(), icon.resolve(), output.resolve()
    for path in (appimage, icon):
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"Missing or empty payload: {path}")
    if not icon.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("The installer icon must be a PNG")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="km-linux-setup-") as temporary:
        work = Path(temporary)
        root = work / ARCHIVE_ROOT
        payload = root / "payload"
        payload.mkdir(parents=True)
        shutil.copyfile(appimage, payload / "application.AppImage")
        shutil.copyfile(icon, payload / "icon.png")
        with (payload / "application.AppImage").open("rb") as source:
            app_hash = hashlib.file_digest(source, "sha256").hexdigest()
        values = {
            "KM_VERSION": version,
            "KM_APP_SHA256": app_hash,
            "KM_ICON_SHA256": hashlib.sha256((payload / "icon.png").read_bytes()).hexdigest(),
        }
        (work / "build-config.h").write_text(
            "".join(f"#define {key} {json.dumps(value)}\n" for key, value in values.items()),
            encoding="utf-8",
        )
        flags = shlex.split(
            subprocess.check_output(["pkg-config", "--cflags", "--libs", "gtk+-3.0"], text=True)
        )
        subprocess.run(
            [
                os.environ.get("CC", "cc"),
                "-std=c11",
                "-D_GNU_SOURCE",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-parameter",
                "-I",
                str(work),
                str(HERE / "installer.c"),
                "-o",
                str(root / SETUP_NAME),
                *flags,
            ],
            check=True,
        )
        (root / SETUP_NAME).chmod(0o755)
        shutil.copyfile(HERE / "README.txt", root / "README.txt")
        for path in payload.iterdir():
            path.chmod(0o644)
        with tarfile.open(output, "w:gz") as archive:
            archive.add(root, arcname=ARCHIVE_ROOT)
    print(f"Created graphical Linux setup: {output}")
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--appimage", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--icon", type=Path)
    build(**vars(parser.parse_args()))


if __name__ == "__main__":
    main()
