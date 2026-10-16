"""The Python `km` keeps itself up to date, like the standalone Rust `km`.

Once a day, after a command finishes, it reads the latest release manifest. When a
newer version exists it downloads that release's wheel, checks it against SHA256SUMS,
checks that list's signature against the key below, and reinstalls with the tool that
installed km (`uv tool` or pip). KM_NO_AUTO_UPDATE=1, CI, and editable development
installs skip the automatic check; `km update` always works.
"""

from __future__ import annotations

import hashlib
import importlib.metadata
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from kritrim_mulyankan import minisign

REPOSITORY = "piyushbodaa/krutrim-mulyankan-evaluator"
# Same key as the desktop updater and the Rust km.
PUBLIC_KEY = "RWRCwcUgkx0kxRFDfbkrHgrCLkOfFOe+zsUAwl7K0XB0YVWFBqEEeAl4"
CHECK_EVERY = 24 * 60 * 60
DISTRIBUTION = "kritrim-mulyankan"


class UpdateError(RuntimeError):
    pass


def current() -> str:
    try:
        return importlib.metadata.version(DISTRIBUTION)
    except importlib.metadata.PackageNotFoundError:
        from kritrim_mulyankan import __version__

        return __version__


def is_newer(candidate: str, installed: str) -> bool:
    def parts(version: str) -> list[int]:
        numbers = version.lstrip("v").replace("-", ".").replace("+", ".").split(".")[:3]
        return [int(n) if n.isdigit() else 0 for n in numbers]

    return parts(candidate) > parts(installed)


def _get(url: str, timeout: float) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": f"km/{current()}"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310
            return response.read(200 << 20)
    except OSError as error:
        raise UpdateError(f"could not reach GitHub ({error})") from error


def latest_version(timeout: float) -> str:
    url = os.environ.get(
        "KM_UPDATE_MANIFEST_URL",
        f"https://github.com/{REPOSITORY}/releases/latest/download/latest.json",
    )
    try:
        version = json.loads(_get(url, timeout))["version"]
    except (ValueError, KeyError, TypeError) as error:
        raise UpdateError("the release manifest is not readable") from error
    return str(version).lstrip("v")


def listed_digest(sums: str, asset: str, version: str) -> str:
    if f"kritrim_mulyankan-{version}-py3-none-any.whl" not in sums:
        raise UpdateError(f"the checksum list is not for version {version}")
    for line in sums.splitlines():
        digest, _, name = line.partition("  ")
        if name.strip() == asset:
            return digest.strip().lower()
    raise UpdateError(f"{asset} is not in the checksum list")


def _development_install() -> bool:
    try:
        direct = importlib.metadata.distribution(DISTRIBUTION).read_text("direct_url.json")
    except importlib.metadata.PackageNotFoundError:
        return True
    return bool(direct and json.loads(direct).get("dir_info", {}).get("editable"))


def _installer() -> list[str] | None:
    """The command that reinstalls km the way it was installed."""
    if "tools" in Path(sys.prefix).resolve().parts and (uv := shutil.which("uv")):
        return [uv, "tool", "install", "--force", "--quiet"]
    if importlib.util.find_spec("pip") is not None:
        return [sys.executable, "-m", "pip", "install", "--upgrade", "--quiet"]
    return None


def update_now(timeout: float) -> str | None:
    """Install a newer km if there is one. Returns the new version, or None if current."""
    latest = latest_version(timeout)
    if not is_newer(latest, current()):
        return None
    if _development_install():
        raise UpdateError(f"km {latest} is available, but this is a development install")
    command = _installer()
    if command is None:
        raise UpdateError(f"km {latest} is available, but neither uv nor pip can install it")
    base = os.environ.get(
        "KM_UPDATE_DOWNLOAD_BASE",
        f"https://github.com/{REPOSITORY}/releases/download/v{latest}",
    )
    sums = _get(f"{base}/SHA256SUMS", 300)
    signature = _get(f"{base}/SHA256SUMS.sig", 300).decode("utf-8", "replace")
    if not minisign.verify(sums, signature, PUBLIC_KEY):
        raise UpdateError("the download's signature does not match; nothing was changed")
    wheel = f"kritrim_mulyankan-{latest}-py3-none-any.whl"
    expected = listed_digest(sums.decode("utf-8", "replace"), wheel, latest)
    data = _get(f"{base}/{wheel}", 300)
    if hashlib.sha256(data).hexdigest() != expected:
        raise UpdateError("the download does not match its checksum; nothing was changed")
    with tempfile.TemporaryDirectory(prefix="km-update-") as folder:
        path = Path(folder) / wheel
        path.write_bytes(data)
        result = subprocess.run(
            [*command, str(path)], capture_output=True, text=True, timeout=600, check=False
        )
    if result.returncode != 0:
        raise UpdateError(
            f"the installer failed: {(result.stderr or result.stdout).strip()[-300:]}"
        )
    return latest


def _stamp() -> Path:
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home()))
    elif sys.platform == "darwin":
        base = Path.home() / "Library" / "Caches"
    else:
        base = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    return base / "krutrim-mulyankan" / "km-python-last-update-check"


def _disabled() -> bool:
    return any(os.environ.get(name, "0") not in ("", "0") for name in ("KM_NO_AUTO_UPDATE", "CI"))


def auto() -> None:
    """The once-a-day automatic update. Never raises and never fails the command."""
    try:
        if _disabled() or _development_install():
            return
        stamp = _stamp()
        try:
            last = float(stamp.read_text())
        except (OSError, ValueError):
            last = 0.0
        if time.time() - last < CHECK_EVERY:
            return
        stamp.parent.mkdir(parents=True, exist_ok=True)
        stamp.write_text(str(int(time.time())))
        new = update_now(timeout=4)
        if new:
            print(
                f"km updated itself to {new}. The new version runs from your next command.",
                file=sys.stderr,
            )
    except UpdateError as error:
        if "is available" in str(error):
            print(f"{error}.", file=sys.stderr)
    except Exception:  # noqa: BLE001 - an update problem must never break km
        pass
