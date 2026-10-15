"""Build and verify the same download payloads on Linux, Windows, and macOS.

Uses only Python's standard library plus the native installer tools. Every smoke
test starts outside the checkout, uses free runners, and stores results in a
temporary directory. No Python installation is required by the shipped apps.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOWNLOADS = ROOT / "dist" / "downloads"
ENGINE = ROOT / "desktop" / "src-tauri" / "resources" / "engine"
PLATFORMS = ("linux-x86_64", "windows-x86_64", "macos-aarch64", "macos-x86_64")
ASSETS = {
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


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def one(paths) -> Path:
    found = list(paths)
    require(len(found) == 1, f"Expected exactly one matching artifact, found {found}")
    return found[0]


def run(command: list[str | Path] | str, **kwargs) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            command if isinstance(command, str) else [str(part) for part in command],
            check=True,
            text=True,
            encoding="utf-8",
            timeout=300,
            **kwargs,
        )
    except subprocess.CalledProcessError as error:
        # Captured engine diagnostics must remain visible in native build logs.
        if error.stdout:
            print(error.stdout)
        if error.stderr:
            print(error.stderr)
        raise


def isolated_environment(directory: Path) -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if not key.startswith(("MJ_", "KM_"))}
    env.update(
        MJ_RUNS_DIR=str(directory / "runs"),
        MJ_DB=str(directory / "runs" / "results.db"),
        NO_COLOR="1",
    )
    return env


def smoke_cli(binary: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="km-cli-smoke-") as temporary:
        work = Path(temporary)
        env = isolated_environment(work)
        for args in (
            ["--help"],
            ["list", "suites"],
            ["run", "example", "-r", "oracle", "-r", "echo"],
        ):
            result = run([binary.resolve(), *args], cwd=work, env=env, capture_output=True)
            require(bool(result.stdout.strip()), f"CLI produced no output for {args}")
        exports = list((work / "runs").glob("*/results.json"))
        require(bool(exports), "CLI did not create an evaluation export")
        check_grades(json.loads(exports[0].read_text(encoding="utf-8")))
    print(f"CLI smoke passed: {binary.name}")


def check_grades(export: dict) -> None:
    graded = {
        (row["task_id"], row["runner_id"]): row["passed"]
        for row in export["results"]
        if row["grade_status"] == "graded"
    }
    require(graded.get(("reasoning.bat-and-ball", "oracle")) is True, "Oracle grade failed")
    require(graded.get(("reasoning.bat-and-ball", "echo")) is False, "Echo grade failed")


def smoke_engine(root: Path) -> None:
    root = root.resolve()
    binary = root / ("km-desktop.exe" if os.name == "nt" else "km-desktop")
    for relative in (
        binary.name,
        "suites/example.yaml",
        "preview/index.html",
        "desktop/ui/dashboard-shim.js",
    ):
        require((root / relative).is_file(), f"Missing packaged resource: {root / relative}")
    params = {"suite": "example", "runners": ["oracle", "echo"], "max_usd": 1, "repeats": 1}
    requests = [
        {"id": "ping", "method": "ping", "params": {}},
        {"id": "catalog", "method": "catalog", "params": {}},
        {"id": "estimate", "method": "estimate", "params": params},
        {"id": "run", "method": "run", "params": {**params, "confirmed": True, "seed": 0}},
    ]
    with tempfile.TemporaryDirectory(prefix="km-engine-smoke-") as temporary:
        work = Path(temporary)
        result = run(
            [binary],
            cwd=work,
            env=isolated_environment(work),
            input="".join(json.dumps(request) + "\n" for request in requests),
            capture_output=True,
        )
        replies = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        responses = {reply["id"]: reply for reply in replies if "ok" in reply}
        for request in requests:
            reply = responses.get(request["id"], {})
            require(reply.get("ok") is True, f"Engine request failed: {request['id']}: {reply}")
        require(responses["ping"]["result"]["engine"] == "kritrim_mulyankan", "Wrong engine")
        packs = {pack["id"] for pack in responses["catalog"]["result"]["packs"]}
        require({"example", "india-pilot", "railway-pilot"} <= packs, "Bundled suites missing")
        estimate = responses["estimate"]["result"]
        require(estimate["ok"] is True and estimate["paid"] is False, "Free preflight failed")
        completed = responses["run"]["result"]
        require(completed["status"] == "completed", "Evaluation did not complete")
        check_grades(json.loads(Path(completed["export_path"]).read_text(encoding="utf-8")))
        require(Path(completed["card_path"]).is_file(), "Result card missing")
        require("/board/" in completed["dashboard_url"], "Dashboard URL missing")
    print(f"Engine smoke passed: {root}")


def stage_engine(target: str) -> None:
    name = "km-desktop.exe" if "windows" in target else "km-desktop"
    ENGINE.mkdir(parents=True, exist_ok=True)
    # Clear old architecture outputs while preserving the tracked placeholder.
    for entry in ENGINE.iterdir():
        if entry.name != ".gitkeep":
            shutil.rmtree(entry) if entry.is_dir() else entry.unlink()
    shutil.copy2(ROOT / "km-rs" / "target" / target / "release" / name, ENGINE / name)
    for directory in ("preview", "suites"):
        shutil.copytree(ROOT / directory, ENGINE / directory)
    (ENGINE / "desktop" / "ui").mkdir(parents=True)
    shutil.copy2(ROOT / "desktop/ui/dashboard-shim.js", ENGINE / "desktop/ui/dashboard-shim.js")


def package_cli(target: str, platform: str) -> None:
    windows = platform.startswith("windows")
    name = "km.exe" if windows else "km"
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    archive = DOWNLOADS / f"km-{platform}{'.zip' if windows else '.tar.gz'}"
    with tempfile.TemporaryDirectory(prefix="km-cli-package-") as temporary:
        work = Path(temporary)
        payload = work / "payload"
        payload.mkdir()
        shutil.copy2(ROOT / "km-rs/target" / target / "release" / name, payload / name)
        shutil.copytree(ROOT / "suites", payload / "suites")
        shutil.copy2(ROOT / "LICENSE", payload / "LICENSE")
        shutil.copy2(ROOT / "NOTICE.txt", payload / "NOTICE.txt")
        if windows:
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
                for path in sorted(payload.rglob("*")):
                    if path.is_file():
                        output.write(path, path.relative_to(payload).as_posix())
            with zipfile.ZipFile(archive) as output:
                require(output.testzip() is None, "Corrupt CLI ZIP")
                output.extractall(work / "unpacked")
        else:
            with tarfile.open(archive, "w:gz") as output:
                output.add(payload / name, arcname=name)
                output.add(payload / "suites", arcname="suites")
                output.add(payload / "LICENSE", arcname="LICENSE")
                output.add(payload / "NOTICE.txt", arcname="NOTICE.txt")
            with tarfile.open(archive) as output:
                output.extractall(work / "unpacked", filter="data")
        smoke_cli(work / "unpacked" / name)
        # Also verify the promised single executable works using embedded suites.
        standalone = work / "standalone"
        standalone.mkdir()
        shutil.copy2(work / "unpacked" / name, standalone / name)
        smoke_cli(standalone / name)
    print(f"Created {archive.name}")


def packaged_engine(directory: Path) -> Path:
    name = "km-desktop.exe" if os.name == "nt" else "km-desktop"
    return one(path for path in directory.rglob(name) if path.parent.name == "engine").parent


def package_desktop(target: str, platform: str) -> None:
    bundle = ROOT / "desktop/src-tauri/target" / target / "release/bundle"
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="km-desktop-package-") as temporary:
        work = Path(temporary)
        if platform.startswith("linux"):
            appimage = one((bundle / "appimage").glob("*.AppImage"))
            run([appimage, "--appimage-extract"], cwd=work, capture_output=True)
            extracted = work / "squashfs-root"
            smoke_engine(packaged_engine(extracted))
            shutil.copy2(appimage, DOWNLOADS / "Krutrim-Mulyankan-linux.AppImage")
            with tarfile.open(DOWNLOADS / "km-desktop-linux.tar.gz", "w:gz") as archive:
                archive.add(extracted, arcname=".")
            deb = one((bundle / "deb").glob("*.deb"))
            run(["dpkg-deb", "--extract", deb, work / "deb"])
            smoke_engine(packaged_engine(work / "deb"))
            shutil.copy2(deb, DOWNLOADS / "Krutrim-Mulyankan-linux.deb")
        elif platform.startswith("windows"):
            installer = one((bundle / "nsis").glob("*.exe"))
            # NSIS consumes /D through the end of the raw command line, without
            # quoting even when its path contains spaces. Exercise that case.
            installed = work / "installed app"
            command = subprocess.list2cmdline([str(installer), "/S", "/NS"])
            run(f"{command} /D={installed}")
            smoke_engine(packaged_engine(installed))
            shutil.copy2(installer, DOWNLOADS / "Krutrim-Mulyankan-windows.exe")
        else:
            dmg = one((bundle / "dmg").glob("*.dmg"))
            mount = work / "mount"
            run(["hdiutil", "attach", dmg, "-readonly", "-nobrowse", "-mountpoint", mount])
            try:
                app = one(mount.glob("*.app"))
                run(["codesign", "--verify", "--deep", "--strict", app])
                smoke_engine(packaged_engine(app))
            finally:
                run(["hdiutil", "detach", mount])
            arch = "arm64" if platform == "macos-aarch64" else "x86_64"
            shutil.copy2(dmg, DOWNLOADS / f"Krutrim-Mulyankan-mac-{arch}.dmg")
    print(f"Desktop packages verified: {platform}")


def check_versions(version: str) -> None:
    manifests = {
        "pyproject.toml": ("project", "version"),
        "km-rs/Cargo.toml": ("workspace", "package", "version"),
        "desktop/src-tauri/Cargo.toml": ("package", "version"),
        "desktop/package.json": ("version",),
        "desktop/src-tauri/tauri.conf.json": ("version",),
    }
    for filename, keys in manifests.items():
        content = (ROOT / filename).read_text(encoding="utf-8")
        actual = json.loads(content) if filename.endswith(".json") else tomllib.loads(content)
        for key in keys:
            actual = actual[key]
        require(actual == version, f"{filename}: {actual} does not match tag {version}")
    print(f"All package versions match {version}")


def verify_release(directory: Path, version: str) -> None:
    expected = ASSETS | {f"kritrim_mulyankan-{version}-py3-none-any.whl"}
    actual = {path.name for path in directory.iterdir()}
    require(
        actual == expected,
        f"Release mismatch: missing {expected - actual}; extra {actual - expected}",
    )
    checksums = []
    for name in sorted(expected):
        path = directory / name
        require(path.is_file() and path.stat().st_size > 0, f"Empty or missing asset: {name}")
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        checksums.append(f"{digest}  {name}\n")
    (directory / "SHA256SUMS").write_text("".join(checksums), encoding="utf-8")
    print(f"Verified {len(expected)} release assets and wrote SHA256SUMS")


def remote_tag_commit(tag: str) -> str | None:
    require(bool(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag)), "Expected a stable version tag")
    ref = f"refs/tags/{tag}"
    result = run(
        ["git", "ls-remote", "--tags", "origin", ref, ref + "^{}"], cwd=ROOT, capture_output=True
    )
    refs = {line.split()[1]: line.split()[0] for line in result.stdout.splitlines()}
    # An annotated tag points to a tag object; its peeled ref identifies the commit.
    return refs.get(ref + "^{}", refs.get(ref))


def release_plan(
    version: str, ref_type: str, ref_name: str, commit: str, existing: str | None
) -> dict[str, str]:
    require(
        bool(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version)), "Expected a stable release version"
    )
    require(bool(re.fullmatch(r"[0-9a-f]{40}", commit)), "Expected the exact tested commit SHA")
    tag = f"v{version}"
    if ref_type == "tag":
        require(ref_name == tag, f"Tag {ref_name} does not match package version {version}")
        require(existing == commit, f"Tag {tag} no longer points at the tested commit {commit}")
        publish = True
    else:
        require(ref_type == "branch" and ref_name == "main", "Automatic releases require main")
        publish = existing is None
    return {"version": version, "tag": tag, "commit": commit, "publish": str(publish).lower()}


def prepare_release(ref_type: str, ref_name: str, commit: str) -> None:
    version = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))["project"][
        "version"
    ]
    check_versions(version)
    checked_out = run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True).stdout.strip()
    require(checked_out == commit, "Checkout does not match the workflow's tested commit")
    plan = release_plan(version, ref_type, ref_name, commit, remote_tag_commit(f"v{version}"))
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        output.writelines(f"{key}={value}\n" for key, value in plan.items())
    decision = "build and publish" if plan["publish"] == "true" else "already tagged; skip"
    print(f"{plan['tag']} at {commit}: {decision}")


def guard_release_tag(tag: str, commit: str) -> None:
    checked_out = run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True).stdout.strip()
    require(checked_out == commit, "Publish checkout differs from the tested commit")
    existing = remote_tag_commit(tag)
    require(
        existing in (None, commit),
        f"Refusing to replace {tag}: it points at {existing}, not {commit}",
    )
    print(f"Tag guard passed: {tag} will refer to {commit}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("stage-engine", "package-cli", "package-desktop"):
        command = commands.add_parser(name)
        command.add_argument("--target", required=True)
        if name != "stage-engine":
            command.add_argument("--platform", choices=PLATFORMS, required=True)
    command = commands.add_parser("smoke-engine")
    command.add_argument("--root", type=Path, required=True)
    command = commands.add_parser("check-versions")
    command.add_argument("--version", required=True)
    command = commands.add_parser("prepare-release")
    command.add_argument("--ref-type", required=True)
    command.add_argument("--ref-name", required=True)
    command.add_argument("--commit", required=True)
    command = commands.add_parser("guard-release-tag")
    command.add_argument("--tag", required=True)
    command.add_argument("--commit", required=True)
    command = commands.add_parser("verify-release")
    command.add_argument("--directory", type=Path, required=True)
    command.add_argument("--version", required=True)
    args = vars(parser.parse_args())
    command = args.pop("command")
    globals()[command.replace("-", "_")](**args)


if __name__ == "__main__":
    main()
