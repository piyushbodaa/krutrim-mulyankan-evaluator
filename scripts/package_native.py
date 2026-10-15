"""Offline native smoke tests and stable release packaging; runs on each target OS."""

import argparse
import json
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def checked(command, **kwargs):
    return subprocess.run(
        command, check=True, capture_output=True, text=True, encoding="utf-8", timeout=120, **kwargs
    )


def package_cli(platform):
    suffix = ".exe" if platform.startswith("windows") else ""
    build = ROOT / "km-rs" / "target" / "release"
    cli = build / f"km{suffix}"
    if platform == "linux-x86_64":
        cli = ROOT / "km-rs/target/x86_64-unknown-linux-musl/release/km"
    engine = build / f"km-desktop{suffix}"
    checked([str(cli), "--help"])
    # A fresh cwd proves resources are embedded, not accidentally found in the checkout.
    with tempfile.TemporaryDirectory() as temp:
        checked([str(cli), "run", "example", "-r", "oracle", "-r", "echo"], cwd=temp)
        response = checked(
            [str(engine)],
            input='{"id":"ci","method":"ping","params":{}}\n',
            cwd=temp,
        )
        messages = [json.loads(line) for line in response.stdout.splitlines() if line.strip()]
        assert any(m.get("id") == "ci" and m.get("ok") is True for m in messages), messages
    output = ROOT / "dist" / "native"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        stage = Path(temp)
        shutil.copy2(cli, stage / cli.name)
        shutil.copytree(ROOT / "suites", stage / "suites")
        shutil.copy2(ROOT / "LICENSE", stage / "LICENSE")
        if suffix:
            with zipfile.ZipFile(
                output / f"km-{platform}.zip", "w", zipfile.ZIP_DEFLATED
            ) as archive:
                for file in sorted(stage.rglob("*")):
                    if file.is_file():
                        archive.write(file, file.relative_to(stage))
        else:
            with tarfile.open(output / f"km-{platform}.tar.gz", "w:gz") as archive:
                for file in sorted(stage.iterdir()):
                    archive.add(file, arcname=file.name)


def collect_installers(platform):
    bundle = ROOT / "desktop" / "src-tauri" / "target" / "release" / "bundle"
    output = ROOT / "dist" / "native"
    output.mkdir(parents=True, exist_ok=True)
    if platform.startswith("windows"):
        pairs = [
            ("nsis/*.exe", f"Krutrim-Mulyankan-{platform}.exe"),
            ("msi/*.msi", f"Krutrim-Mulyankan-{platform}.msi"),
        ]
    elif platform.startswith("macos"):
        pairs = [("dmg/*.dmg", f"Krutrim-Mulyankan-{platform}.dmg")]
        apps = list((bundle / "macos").glob("*.app"))
        assert len(apps) == 1, apps
        checked(["codesign", "--verify", "--deep", "--strict", str(apps[0])])
        packaged_engine = apps[0] / "Contents/Resources/engine/km-desktop"
        assert packaged_engine.is_file(), packaged_engine
        checked([str(packaged_engine)], input='{"id":"ci","method":"ping","params":{}}\n')
    else:
        pairs = [
            ("appimage/*.AppImage", "Krutrim-Mulyankan-linux.AppImage"),
            ("deb/*.deb", "Krutrim-Mulyankan-linux.deb"),
        ]
    for pattern, name in pairs:
        matches = list(bundle.glob(pattern))
        assert len(matches) == 1, (pattern, matches)
        assert matches[0].stat().st_size > 0
        shutil.copy2(matches[0], output / name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "platform", choices=["linux-x86_64", "windows-x86_64", "macos-aarch64", "macos-x86_64"]
    )
    parser.add_argument("--installers", action="store_true")
    args = parser.parse_args()
    if args.installers:
        collect_installers(args.platform)
    else:
        package_cli(args.platform)
