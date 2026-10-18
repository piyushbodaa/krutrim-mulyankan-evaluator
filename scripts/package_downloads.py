"""Build and verify the Linux download payloads.

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
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOWNLOADS = ROOT / "dist" / "downloads"
ENGINE = ROOT / "desktop" / "src-tauri" / "resources" / "engine"
PLATFORMS = ("linux-x86_64",)
ASSETS = {
    "Krutrim-Mulyankan-linux.AppImage",
    "Krutrim-Mulyankan-linux.deb",
    "Krutrim-Mulyankan-linux.rpm",
    "Krutrim-Mulyankan-linux.pkg.tar.zst",
    "Krutrim-Mulyankan-linux-setup.tar.gz",
    "km-desktop-linux.tar.gz",
    "km-linux-x86_64.tar.gz",
}
ARCH_DEPENDS = (
    "cairo",
    "dbus",
    "gdk-pixbuf2",
    "glib2",
    "gtk3",
    "hicolor-icon-theme",
    "libsoup3",
    "pango",
    "webkit2gtk-4.1",
    "xdg-utils",
)
DESKTOP_ID = "in.krutrimmulyankan.evaluator"


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
    binary = root / "km-desktop"
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
    name = "km-desktop"
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
    name = "km"
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    archive = DOWNLOADS / f"km-{platform}.tar.gz"
    with tempfile.TemporaryDirectory(prefix="km-cli-package-") as temporary:
        work = Path(temporary)
        payload = work / "payload"
        payload.mkdir()
        shutil.copy2(ROOT / "km-rs/target" / target / "release" / name, payload / name)
        shutil.copytree(ROOT / "suites", payload / "suites")
        shutil.copy2(ROOT / "LICENSE", payload / "LICENSE")
        shutil.copy2(ROOT / "NOTICE.txt", payload / "NOTICE.txt")
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
    name = "km-desktop"
    return one(path for path in directory.rglob(name) if path.parent.name == "engine").parent


def verify_linux_payload(directory: Path) -> None:
    """Check the installed layout, without installing anything on the build host."""
    binary = directory / "usr/bin/krutrim-mulyankan-desktop"
    require(binary.is_file() and os.access(binary, os.X_OK), "Desktop binary is not executable")
    desktop = one((directory / "usr/share/applications").glob("*.desktop"))
    run(["desktop-file-validate", desktop])
    fields = dict(
        line.split("=", 1)
        for line in desktop.read_text(encoding="utf-8").splitlines()
        if "=" in line and not line.startswith("#")
    )
    require(fields.get("Type") == "Application", "Desktop launcher has no application type")
    require("krutrim-mulyankan" in fields.get("Exec", ""), "Desktop launcher has the wrong command")
    require(fields.get("Terminal", "false") == "false", "Desktop launcher must not open a terminal")
    icon = fields.get("Icon", "")
    require(bool(icon), "Desktop launcher has no icon")
    require(
        bool(list((directory / "usr/share/icons/hicolor").glob(f"*/apps/{icon}.png"))),
        "Desktop launcher's icon is not installed",
    )
    smoke_engine(packaged_engine(directory))


def extract_deb(archive: Path, destination: Path) -> dict[str, str]:
    """libarchive works on Arch as well as Ubuntu; dpkg is not required."""
    destination.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="km-deb-extract-") as temporary:
        work = Path(temporary)
        run(["bsdtar", "-xf", archive.resolve(), "-C", work])
        with tarfile.open(one(work.glob("control.tar.*"))) as source:
            control = one(
                member for member in source.getmembers() if member.name.lstrip("./") == "control"
            )
            content = source.extractfile(control).read().decode("utf-8")
        with tarfile.open(one(work.glob("data.tar.*"))) as source:
            source.extractall(destination, filter="data")
    return dict(
        line.split(": ", 1)
        for line in content.splitlines()
        if ": " in line and not line.startswith(" ")
    )


def archive_payload(payload: Path, archive: Path, epoch: int) -> None:
    """Use stable paths, ownership, modes and times for the native package input."""

    def normalize(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.uid = info.gid = 0
        info.uname = info.gname = "root"
        info.mtime = epoch
        if not info.issym():
            info.mode = 0o755 if info.isdir() or info.mode & 0o111 else 0o644
        return info

    with tarfile.open(archive, "w", format=tarfile.PAX_FORMAT) as output:
        for path in sorted(payload.rglob("*")):
            output.add(path, arcname=path.relative_to(payload), recursive=False, filter=normalize)


def arch_pkgbuild(version: str, digest: str) -> str:
    require(
        bool(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version)), "Expected a stable package version"
    )
    require(bool(re.fullmatch(r"[0-9a-f]{64}", digest)), "Expected a SHA-256 payload checksum")
    dependencies = " ".join(f"'{name}'" for name in ARCH_DEPENDS)
    return f"""# Generated from the exact native desktop payload; no build-time downloads.
pkgname=krutrim-mulyankan
pkgver={version}
pkgrel=1
pkgdesc='Check AI models on your own computer'
arch=('x86_64')
url='https://krutrimmulyankan.in'
license=('MIT')
depends=({dependencies})
optdepends=('gnome-keyring: save API keys with the desktop Secret Service')
options=('!strip' '!debug' '!lto')
source=('payload.tar')
sha256sums=('{digest}')
package() {{
  cp -a --no-preserve=ownership "$srcdir/usr" "$pkgdir/"
}}
"""


def stage_native_deb(deb: Path, payload: Path) -> str:
    version = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))["project"][
        "version"
    ]
    control = extract_deb(deb, payload)
    require(control.get("Package") == "krutrim-mulyankan", "Unexpected source DEB package")
    require(control.get("Version") == version, "DEB version differs from the source version")
    require(control.get("Architecture") == "amd64", "Native download requires an x86-64 DEB")
    desktop = one((payload / "usr/share/applications").glob("*.desktop"))
    desktop.unlink()
    (desktop.parent / f"{DESKTOP_ID}.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=Krutrim Mulyankan\n"
        "Comment=Check AI models on your own computer\n"
        "Exec=krutrim-mulyankan-desktop\nIcon=krutrim-mulyankan-desktop\n"
        "Terminal=false\nCategories=Utility;Development;\nStartupNotify=true\n",
        encoding="utf-8",
    )
    (payload / "usr/bin/krutrim-mulyankan").symlink_to("krutrim-mulyankan-desktop")
    for name in ("LICENSE", "NOTICE.txt"):
        destination = payload / "usr/share/licenses/krutrim-mulyankan" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, destination)
    verify_linux_payload(payload)
    return version


def source_epoch() -> int:
    return int(
        os.environ.get("SOURCE_DATE_EPOCH")
        or run(["git", "log", "-1", "--format=%ct"], cwd=ROOT, capture_output=True).stdout.strip()
    )


def package_arch(deb: Path) -> None:
    """Build with Arch's makepkg, preserving Tauri's native resource layout."""
    epoch = source_epoch()
    with tempfile.TemporaryDirectory(prefix="km-arch-package-") as temporary:
        work = Path(temporary)
        payload = work / "payload"
        version = stage_native_deb(deb, payload)
        archive = work / "payload.tar"
        archive_payload(payload, archive, epoch)
        with archive.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        (work / "PKGBUILD").write_text(arch_pkgbuild(version, digest), encoding="utf-8")
        env = {**os.environ, "SOURCE_DATE_EPOCH": str(epoch), "PKGDEST": str(work / "packages")}
        run(["makepkg", "--force", "--nodeps", "--nocheck", "--noconfirm"], cwd=work, env=env)
        package = one((work / "packages").glob("*.pkg.tar.zst"))
        installed = work / "installed"
        installed.mkdir()
        run(["bsdtar", "-xf", package, "-C", installed])
        info = (installed / ".PKGINFO").read_text(encoding="utf-8")
        require(
            f"pkgver = {version}-1\n" in info and "arch = x86_64\n" in info,
            "Wrong Arch package metadata",
        )
        require(
            all(f"depend = {name}\n" in info for name in ARCH_DEPENDS),
            "Missing Arch runtime dependency",
        )
        verify_linux_payload(installed)
        DOWNLOADS.mkdir(parents=True, exist_ok=True)
        shutil.copy2(package, DOWNLOADS / "Krutrim-Mulyankan-linux.pkg.tar.zst")
    print("Arch package built and verified")


def package_rpm_from_deb(deb: Path) -> None:
    """Add RPM packaging to an existing release without recompiling its binaries."""
    with tempfile.TemporaryDirectory(prefix="km-rpm-package-") as temporary:
        work = Path(temporary)
        payload = work / "payload"
        version = stage_native_deb(deb, payload)
        for name in ("BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"):
            (work / name).mkdir()
        archive_payload(payload, work / "SOURCES/payload.tar", source_epoch())
        config = json.loads((ROOT / "desktop/src-tauri/tauri.linux.conf.json").read_text())
        dependencies = config["bundle"]["linux"]["rpm"]["depends"]
        spec = work / "SPECS/krutrim-mulyankan.spec"
        spec.write_text(
            "%global _build_id_links none\n%global __os_install_post %{nil}\n"
            f"Name: krutrim-mulyankan\nVersion: {version}\nRelease: 1\n"
            "Summary: Check AI models on your own computer\nLicense: MIT\n"
            "URL: https://krutrimmulyankan.in\nBuildArch: x86_64\nSource0: payload.tar\n"
            + "".join(f"Requires: {dependency}\n" for dependency in dependencies)
            + "Recommends: gnome-keyring\n\n%description\n"
            "Local AI evaluation. Unvalidated pilot. Not a ranking.\n\n"
            "%prep\n%setup -q -c -n payload\n\n%build\n\n%install\n"
            "mkdir -p %{buildroot}\ncp -a --no-preserve=ownership usr %{buildroot}/\n\n%files\n"
            "/usr/bin/krutrim-mulyankan\n/usr/bin/krutrim-mulyankan-desktop\n"
            '"/usr/lib/Krutrim Mulyankan"\n'
            f"/usr/share/applications/{DESKTOP_ID}.desktop\n"
            "/usr/share/icons/hicolor/*/apps/krutrim-mulyankan-desktop.png\n"
            "/usr/share/licenses/krutrim-mulyankan\n",
            encoding="utf-8",
        )
        run(
            ["rpmbuild", "-bb", "--define", f"_topdir {work}", spec],
            env={
                **os.environ,
                "SOURCE_DATE_EPOCH": str(source_epoch()),
            },
        )
        package = one((work / "RPMS/x86_64").glob("*.rpm"))
        metadata = run(
            ["rpm", "-qp", "--queryformat", "%{NAME} %{VERSION} %{ARCH}", package],
            capture_output=True,
        ).stdout
        require(metadata == f"krutrim-mulyankan {version} x86_64", "Wrong RPM package metadata")
        requires = run(
            ["rpm", "-qp", "--requires", package], capture_output=True
        ).stdout.splitlines()
        require(set(dependencies) <= set(requires), "Missing RPM runtime dependency")
        installed = work / "installed"
        installed.mkdir()
        run(["bsdtar", "-xf", package, "-C", installed])
        verify_linux_payload(installed)
        DOWNLOADS.mkdir(parents=True, exist_ok=True)
        shutil.copy2(package, DOWNLOADS / "Krutrim-Mulyankan-linux.rpm")
    print("RPM package built and verified from the released DEB")


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
            version = json.loads((ROOT / "desktop/src-tauri/tauri.conf.json").read_text())[
                "version"
            ]
            run(
                [
                    "python3",
                    ROOT / "desktop/linux-installer/build.py",
                    "--appimage",
                    appimage,
                    "--output",
                    DOWNLOADS / "Krutrim-Mulyankan-linux-setup.tar.gz",
                    "--version",
                    version,
                    "--icon",
                    ROOT / "desktop/src-tauri/icons/128x128@2x.png",
                ]
            )
            with tarfile.open(DOWNLOADS / "km-desktop-linux.tar.gz", "w:gz") as archive:
                archive.add(extracted, arcname=".")
            deb = one((bundle / "deb").glob("*.deb"))
            extract_deb(deb, work / "deb")
            verify_linux_payload(work / "deb")
            shutil.copy2(deb, DOWNLOADS / "Krutrim-Mulyankan-linux.deb")
            rpm = one((bundle / "rpm").glob("*.rpm"))
            metadata = run(
                ["rpm", "-qp", "--queryformat", "%{NAME} %{VERSION} %{ARCH}", rpm],
                capture_output=True,
            ).stdout
            require(metadata == f"krutrim-mulyankan {version} x86_64", "Wrong RPM package metadata")
            dependencies = run(
                ["rpm", "-qp", "--requires", rpm], capture_output=True
            ).stdout.splitlines()
            config = json.loads((ROOT / "desktop/src-tauri/tauri.linux.conf.json").read_text())
            require(
                set(config["bundle"]["linux"]["rpm"]["depends"]) <= set(dependencies),
                "Missing RPM runtime dependency",
            )
            (work / "rpm").mkdir()
            run(["bsdtar", "-xf", rpm, "-C", work / "rpm"])
            verify_linux_payload(work / "rpm")
            shutil.copy2(rpm, DOWNLOADS / "Krutrim-Mulyankan-linux.rpm")
    print(f"Desktop packages verified: {platform}")


def check_versions(version: str) -> None:
    manifests = {
        "pyproject.toml": ("project", "version"),
        "km-rs/Cargo.toml": ("workspace", "package", "version"),
        "desktop/src-tauri/Cargo.toml": ("package", "version"),
        "desktop/package.json": ("version",),
        "desktop/src-tauri/tauri.conf.json": ("version",),
    }
    # The Python package constant feeds manifests/results and once drifted
    # from pyproject during a bump; pin it to the same version.
    init_text = (ROOT / "src/kritrim_mulyankan/__init__.py").read_text(encoding="utf-8")
    require(
        f'__version__ = "{version}"' in init_text,
        "src/kritrim_mulyankan/__init__.py __version__ is out of sync",
    )
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
    command = commands.add_parser("package-arch")
    command.add_argument("--deb", type=Path, default=DOWNLOADS / "Krutrim-Mulyankan-linux.deb")
    for name in ("package-arch-from-deb", "package-rpm-from-deb"):
        command = commands.add_parser(name)
        command.add_argument("--deb", type=Path, required=True)
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
    if command == "package-arch-from-deb":
        command = "package-arch"
    globals()[command.replace("-", "_")](**args)


if __name__ == "__main__":
    main()
