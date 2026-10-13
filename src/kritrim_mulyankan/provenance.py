"""Record local source/runtime provenance without collecting environment secrets."""

from __future__ import annotations

import hashlib
import importlib.metadata
import os
import subprocess
from pathlib import Path
from typing import Any


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_metadata() -> dict[str, Any]:
    root = Path(__file__).resolve().parents[2]

    def git(*args: str) -> str | None:
        try:
            return subprocess.check_output(
                ["git", "-C", str(root), *args], text=True, stderr=subprocess.DEVNULL, timeout=5
            ).strip()
        except (OSError, subprocess.SubprocessError):
            return None

    tracked = git("ls-files", "src", "pyproject.toml")
    # Hash all source files, including new uncommitted modules, rather than exposing diffs.
    paths = sorted((root / "src").rglob("*.py")) + sorted((root / "src").rglob("*.yaml"))
    paths += [root / "pyproject.toml", root / "uv.lock"]
    digest = hashlib.sha256()
    for path in paths:
        if path.is_file():
            digest.update(str(path.relative_to(root)).encode())
            digest.update(path.read_bytes())
    status = git("status", "--porcelain", "--untracked-files=normal")
    dependencies = {}
    for package in ("anthropic", "openai", "httpx2", "pydantic", "pyyaml", "typer"):
        try:
            dependencies[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            dependencies[package] = None
    return {
        "git_commit": git("rev-parse", "HEAD"),
        "dirty": bool(status) if status is not None else None,
        "source_sha256": digest.hexdigest() if tracked is not None else None,
        "dependencies": dependencies,
    }


def redact(value: Any) -> Any:
    secrets = [
        v
        for k, v in os.environ.items()
        if len(v) >= 8
        and any(marker in k.upper() for marker in ("API_KEY", "AUTH_TOKEN", "SECRET", "PASSWORD"))
    ]
    if isinstance(value, str):
        for secret in secrets:
            value = value.replace(secret, "[REDACTED]")
        return value
    if isinstance(value, dict):
        return {k: redact(v) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    return value
