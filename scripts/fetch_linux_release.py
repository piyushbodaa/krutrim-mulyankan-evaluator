"""Fetch checksum-verified inputs for adding installers to an existing release.

This never changes the released application binaries. Publishing is a separate,
credential-bearing workflow job, after the offline installer tests have passed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import urllib.request
from pathlib import Path

REPOSITORY = "piyushbodaa/krutrim-mulyankan-evaluator"
INPUTS = ("Krutrim-Mulyankan-linux.AppImage", "Krutrim-Mulyankan-linux.deb")


def fetch_release(directory: Path, tag: str) -> dict:
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError("Expected a stable vMAJOR.MINOR.PATCH release tag")
    request = urllib.request.Request(
        f"https://api.github.com/repos/{REPOSITORY}/releases/tags/{tag}",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "km-linux-installer-ci"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        release = json.load(response)
    if (
        release.get("tag_name") != tag
        or release.get("draft") is not False
        or release.get("prerelease") is not False
    ):
        raise ValueError("Expected the requested public stable release")
    directory.mkdir(parents=True, exist_ok=True)
    inputs = {}
    for name in INPUTS:
        matches = [asset for asset in release.get("assets", []) if asset.get("name") == name]
        if len(matches) != 1:
            raise ValueError(f"Expected one released input: {name}")
        asset = matches[0]
        expected_url = f"https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"
        digest = asset.get("digest", "")
        if (
            asset.get("browser_download_url") != expected_url
            or asset.get("state") != "uploaded"
            or not isinstance(asset.get("size"), int)
            or asset["size"] <= 0
            or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest)
        ):
            raise ValueError(f"Invalid release input metadata: {name}")
        path = directory / name
        temporary = directory / (name + ".part")
        try:
            with (
                urllib.request.urlopen(expected_url, timeout=120) as source,
                temporary.open("wb") as destination,
            ):
                while chunk := source.read(1024 * 1024):
                    destination.write(chunk)
            with temporary.open("rb") as source:
                actual = "sha256:" + hashlib.file_digest(source, "sha256").hexdigest()
            if temporary.stat().st_size != asset["size"] or actual != digest:
                raise ValueError(f"Released input failed checksum verification: {name}")
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
        inputs[name] = {"sha256": digest.removeprefix("sha256:"), "size": asset["size"]}
        print(f"Verified released input: {name}")
    provenance = {"tag": tag, "version": tag[1:], "inputs": inputs}
    (directory / "release-inputs.json").write_text(
        json.dumps(provenance, indent=2) + "\n", encoding="utf-8"
    )
    return provenance


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    fetch_release(args.directory, args.tag)
