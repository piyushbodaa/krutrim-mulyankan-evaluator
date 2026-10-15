"""Guards for desktop packaging choices that are easy to lose in an edit.

These read the committed config/UI/workflow files. They do not build
anything; the desktop workflow does the real building.
"""

import hashlib
import json
import plistlib
import runpy
import tarfile
import tomllib
from pathlib import Path
from types import SimpleNamespace

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
PACKAGING = runpy.run_path(str(ROOT / "scripts/package_downloads.py"))


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


def test_ci_and_release_use_the_same_native_download_builds():
    for filename in ("desktop.yml", "release.yml"):
        workflow = yaml.safe_load(_read(f".github/workflows/{filename}"))
        assert workflow["jobs"]["downloads"]["uses"] == "./.github/workflows/build-downloads.yml"
        assert workflow["jobs"]["downloads"]["needs"] == "test"
    release = yaml.safe_load(_read(".github/workflows/release.yml"))
    assert set(release["jobs"]["publish"]["needs"]) == {"prepare", "wheel", "downloads"}
    conf = json.loads(_read("desktop/src-tauri/tauri.conf.json"))
    assert conf["bundle"]["shortDescription"]


def test_rust_engine_is_built_for_cli_and_desktop_without_python():
    # The deployed binaries are Rust now (km-rs/). The Python package/wheel
    # stays, but no PyInstaller engine may creep back into the installers.
    workflow = yaml.safe_load(_read(".github/workflows/build-downloads.yml"))
    commands = "\n".join(step.get("run", "") for step in workflow["jobs"]["build"]["steps"])
    assert "cargo build --locked --release" in commands
    assert "-p km-cli" in commands and "-p km-desktop-engine" in commands
    assert "cargo test --locked" in commands
    assert "npm ci" in commands
    assert "PyInstaller" not in commands
    workspace = tomllib.loads(_read("km-rs/Cargo.toml"))
    assert "crates/evaluator" in workspace["workspace"]["members"]
    cli = tomllib.loads(_read("km-rs/crates/km/Cargo.toml"))
    engine = tomllib.loads(_read("km-rs/crates/km-desktop/Cargo.toml"))
    assert cli["bin"][0]["name"] == "km"
    assert engine["bin"][0]["name"] == "km-desktop"


def test_native_matrix_uses_matching_architecture_and_installer_formats():
    workflow = yaml.safe_load(_read(".github/workflows/build-downloads.yml"))
    rows = workflow["jobs"]["build"]["strategy"]["matrix"]["include"]
    actual = {
        (row["platform"], row["os"], row["cli-target"], row["desktop-target"], row["bundles"])
        for row in rows
    }
    assert actual == {
        (
            "linux-x86_64",
            "ubuntu-22.04",
            "x86_64-unknown-linux-musl",
            "x86_64-unknown-linux-gnu",
            "appimage,deb,rpm",
        ),
        (
            "windows-x86_64",
            "windows-2022",
            "x86_64-pc-windows-msvc",
            "x86_64-pc-windows-msvc",
            "nsis",
        ),
        (
            "macos-aarch64",
            "macos-15",
            "aarch64-apple-darwin",
            "aarch64-apple-darwin",
            "dmg",
        ),
        (
            "macos-x86_64",
            "macos-15-intel",
            "x86_64-apple-darwin",
            "x86_64-apple-darwin",
            "dmg",
        ),
    }


def test_release_assets_match_the_download_contract():
    expected = {
        "Krutrim-Mulyankan-linux.AppImage",
        "Krutrim-Mulyankan-linux.deb",
        "Krutrim-Mulyankan-linux.rpm",
        "Krutrim-Mulyankan-linux.pkg.tar.zst",
        "Krutrim-Mulyankan-linux-setup.tar.gz",
        "km-desktop-linux.tar.gz",
        "Krutrim-Mulyankan-windows.exe",
        "Krutrim-Mulyankan-mac-arm64.dmg",
        "Krutrim-Mulyankan-mac-x86_64.dmg",
        "km-linux-x86_64.tar.gz",
        "km-windows-x86_64.zip",
        "km-macos-aarch64.tar.gz",
        "km-macos-x86_64.tar.gz",
    }
    assert PACKAGING["ASSETS"] == expected
    release = yaml.safe_load(_read(".github/workflows/release.yml"))
    steps = release["jobs"]["publish"]["steps"]
    publish = next(step for step in steps if step.get("uses", "").startswith("softprops/"))
    files = {Path(path).name for path in publish["with"]["files"].splitlines()}
    assert files == expected | {"kritrim_mulyankan-*.whl", "SHA256SUMS"}


def test_arch_payload_has_stable_metadata_and_keeps_executable_bits(tmp_path):
    payload = tmp_path / "payload"
    binaries = payload / "usr/bin"
    binaries.mkdir(parents=True)
    binary = binaries / "krutrim-mulyankan-desktop"
    binary.write_bytes(b"native executable")
    binary.chmod(0o775)
    data = payload / "usr/NOTICE.txt"
    data.write_text("notice", encoding="utf-8")
    data.chmod(0o664)
    (binaries / "krutrim-mulyankan").symlink_to(binary.name)
    first = tmp_path / "first.tar"
    second = tmp_path / "second.tar"
    PACKAGING["archive_payload"](payload, first, 1700000000)
    # A different umask and modified file times must not alter package input.
    binary.chmod(0o755)
    data.chmod(0o644)
    PACKAGING["archive_payload"](payload, second, 1700000000)
    assert first.read_bytes() == second.read_bytes()
    with tarfile.open(first) as archive:
        members = {member.name: member for member in archive.getmembers()}
        assert members["usr/bin/krutrim-mulyankan-desktop"].mode == 0o755
        assert members["usr/NOTICE.txt"].mode == 0o644
        assert members["usr/bin/krutrim-mulyankan"].issym()
        assert members["usr/bin/krutrim-mulyankan"].linkname == binary.name
        assert all(member.uid == member.gid == 0 for member in members.values())
        assert all(member.mtime == 1700000000 for member in members.values())


@pytest.mark.parametrize(
    "version,digest", [("1.2.3; touch /tmp/injected", "a" * 64), ("1.2.3", "not a digest")]
)
def test_arch_recipe_rejects_unsafe_version_or_payload_checksum(version, digest):
    with pytest.raises(RuntimeError):
        PACKAGING["arch_pkgbuild"](version, digest)


def test_native_linux_builds_declare_runtime_dependencies_and_build_arch_package():
    linux = json.loads(_read("desktop/src-tauri/tauri.linux.conf.json"))
    rpm = linux["bundle"]["linux"]["rpm"]
    # RPM has no Debian-style automatic dependency inference. The WebKit ABI
    # capability works with both Fedora and openSUSE package naming.
    assert "libwebkit2gtk-4.1.so.0()(64bit)" in rpm["depends"]
    assert "libgtk-3.so.0()(64bit)" in rpm["depends"]
    assert "xdg-utils" in rpm["depends"]
    workflow = yaml.safe_load(_read(".github/workflows/build-downloads.yml"))
    steps = workflow["jobs"]["build"]["steps"]
    arch = next(
        step for step in steps if step.get("name") == "Build and verify the native Arch package"
    )
    assert "runuser -u builder -- python scripts/package_downloads.py package-arch" in arch["run"]
    assert "SOURCE_DATE_EPOCH" in arch["run"]


def test_release_verification_rejects_incomplete_downloads_and_hashes_all_assets(tmp_path):
    expected = PACKAGING["ASSETS"] | {"kritrim_mulyankan-1.2.3-py3-none-any.whl"}
    verify = PACKAGING["verify_release"]
    with pytest.raises(RuntimeError, match="missing"):
        verify(tmp_path, "1.2.3")
    for name in expected:
        (tmp_path / name).write_bytes(name.encode())
    extra = tmp_path / "unexpected.exe"
    extra.write_bytes(b"unexpected")
    with pytest.raises(RuntimeError, match="extra"):
        verify(tmp_path, "1.2.3")
    extra.unlink()
    asset = tmp_path / "km-windows-x86_64.zip"
    asset.write_bytes(b"")
    with pytest.raises(RuntimeError, match="Empty"):
        verify(tmp_path, "1.2.3")
    asset.write_bytes(asset.name.encode())
    verify(tmp_path, "1.2.3")
    actual = (tmp_path / "SHA256SUMS").read_text().splitlines()
    assert actual == [
        f"{hashlib.sha256(name.encode()).hexdigest()}  {name}" for name in sorted(expected)
    ]


def test_release_versions_stay_in_sync():
    version = tomllib.loads(_read("pyproject.toml"))["project"]["version"]
    PACKAGING["check_versions"](version)
    npm_lock = json.loads(_read("desktop/package-lock.json"))
    assert npm_lock["version"] == version
    assert npm_lock["packages"][""]["version"] == version


def test_windows_cli_and_engine_include_the_visual_cpp_runtime():
    config = tomllib.loads(_read(".cargo/config.toml"))
    flags = config["target"]["x86_64-pc-windows-msvc"]["rustflags"]
    assert "target-feature=+crt-static" in flags


@pytest.mark.parametrize("fmt", [plistlib.FMT_XML, plistlib.FMT_BINARY])
def test_macos_bundle_allows_only_the_loopback_dashboard(tmp_path, fmt):
    conf = json.loads(_read("desktop/src-tauri/tauri.conf.json"))
    assert conf["bundle"]["macOS"]["exceptionDomain"] == "127.0.0.1"
    app = tmp_path / "Krutrim Mulyankan.app"
    contents = app / "Contents"
    contents.mkdir(parents=True)
    info = {
        "NSAppTransportSecurity": {
            "NSExceptionDomains": {
                "127.0.0.1": {
                    "NSExceptionAllowsInsecureHTTPLoads": True,
                    "NSIncludesSubdomains": True,
                }
            }
        }
    }
    (contents / "Info.plist").write_bytes(plistlib.dumps(info, fmt=fmt))
    PACKAGING["verify_macos_transport"](app)


@pytest.mark.parametrize(
    "defect",
    [
        "missing_policy",
        "missing_loopback",
        "http_blocked",
        "external_domain",
        "NSAllowsArbitraryLoads",
        "NSAllowsArbitraryLoadsInWebContent",
        "NSAllowsArbitraryLoadsForMedia",
        "NSAllowsLocalNetworking",
    ],
)
def test_macos_bundle_rejects_blocked_dashboard_or_broad_http_policy(tmp_path, defect):
    app = tmp_path / "Krutrim Mulyankan.app"
    contents = app / "Contents"
    contents.mkdir(parents=True)
    loopback = {"NSExceptionAllowsInsecureHTTPLoads": True}
    exceptions = {"127.0.0.1": loopback}
    security = {"NSExceptionDomains": exceptions}
    info = {"NSAppTransportSecurity": security}
    if defect == "missing_policy":
        info.clear()
    elif defect == "missing_loopback":
        exceptions.clear()
    elif defect == "http_blocked":
        loopback["NSExceptionAllowsInsecureHTTPLoads"] = False
    elif defect == "external_domain":
        exceptions["example.com"] = dict(loopback)
    else:
        security[defect] = True
    (contents / "Info.plist").write_bytes(plistlib.dumps(info))
    with pytest.raises(RuntimeError, match="macOS app must"):
        PACKAGING["verify_macos_transport"](app)


def test_release_triggers_and_publisher_use_the_validated_version_and_commit():
    # BaseLoader preserves YAML's `on` as a string rather than a YAML 1.1 boolean.
    release = yaml.load(_read(".github/workflows/release.yml"), Loader=yaml.BaseLoader)
    trigger = release["on"]["push"]
    assert trigger["branches"] == ["main"]
    assert trigger["tags"] == ["v*"]
    assert "pyproject.toml" in trigger["paths"]
    jobs = release["jobs"]
    assert jobs["test"]["needs"] == "prepare"
    assert jobs["test"]["if"] == "needs.prepare.outputs.publish == 'true'"
    publish = jobs["publish"]
    assert publish["concurrency"]["group"] == "release-publish-${{ needs.prepare.outputs.tag }}"
    steps = publish["steps"]
    action = next(step for step in steps if step.get("uses", "").startswith("softprops/"))
    assert action["with"]["tag_name"] == "${{ needs.prepare.outputs.tag }}"
    assert action["with"]["target_commitish"] == "${{ needs.prepare.outputs.commit }}"
    assert any("guard-release-tag" in step.get("run", "") for step in steps[: steps.index(action)])


@pytest.mark.parametrize("existing", [None, "a" * 40, "b" * 40])
def test_main_version_release_skips_any_existing_tag(existing):
    plan = PACKAGING["release_plan"]("1.2.3", "branch", "main", "a" * 40, existing)
    assert plan == {
        "version": "1.2.3",
        "tag": "v1.2.3",
        "commit": "a" * 40,
        "publish": "true" if existing is None else "false",
    }


def test_explicit_tag_release_and_retry_require_the_original_commit():
    plan = PACKAGING["release_plan"]("1.2.3", "tag", "v1.2.3", "a" * 40, "a" * 40)
    assert plan["publish"] == "true"
    for existing in (None, "b" * 40):
        with pytest.raises(RuntimeError, match="no longer points"):
            PACKAGING["release_plan"]("1.2.3", "tag", "v1.2.3", "a" * 40, existing)


@pytest.mark.parametrize(
    ("version", "ref_type", "ref_name", "commit"),
    [
        ("1.2.3", "tag", "v1.2.2", "a" * 40),
        ("1.2.3", "branch", "feature", "a" * 40),
        ("1.2.3", "branch", "main", "main"),
        ("1.2.3-beta", "branch", "main", "a" * 40),
    ],
)
def test_release_rejects_wrong_versions_branches_or_ambiguous_commits(
    version, ref_type, ref_name, commit
):
    with pytest.raises(RuntimeError):
        PACKAGING["release_plan"](version, ref_type, ref_name, commit, "a" * 40)


def test_remote_annotated_tag_is_compared_by_its_peeled_commit(monkeypatch):
    lookup = PACKAGING["remote_tag_commit"]
    stdout = f"{'b' * 40}\trefs/tags/v1.2.3\n{'a' * 40}\trefs/tags/v1.2.3^{{}}\n"
    monkeypatch.setitem(lookup.__globals__, "run", lambda *a, **k: SimpleNamespace(stdout=stdout))
    assert lookup("v1.2.3") == "a" * 40


def test_publish_guard_rejects_tag_moved_during_the_build(monkeypatch):
    guard = PACKAGING["guard_release_tag"]
    monkeypatch.setitem(guard.__globals__, "run", lambda *a, **k: SimpleNamespace(stdout="a" * 40))
    monkeypatch.setitem(guard.__globals__, "remote_tag_commit", lambda tag: "b" * 40)
    with pytest.raises(RuntimeError, match="Refusing to replace"):
        guard("v1.2.3", "a" * 40)
