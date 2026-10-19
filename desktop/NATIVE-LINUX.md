# Arch / Omarchy package

The desktop app ships as one Arch Linux x86-64 package for Arch and
Omarchy: `Krutrim-Mulyankan-linux.pkg.tar.zst`. Install it with

```sh
sudo pacman -U Krutrim-Mulyankan-linux.pkg.tar.zst
```

then open **Krutrim Mulyankan** from the application menu (or run
`krutrim-mulyankan`). Pacman installs the executable, desktop launcher,
icons, resources, and declared system dependencies, and removes them
cleanly with `sudo pacman -R krutrim-mulyankan`.

Native package name: `krutrim-mulyankan`. Tauri's executable remains
`/usr/bin/krutrim-mulyankan-desktop`; the package also provides the
`krutrim-mulyankan` command. Its application ID is
`in.krutrimmulyankan.evaluator`. API keys use the desktop Secret Service
for persistence; GNOME Keyring is an optional provider.

## How it is built

The shared download workflow builds the shell on Ubuntu 22.04 and asks
Tauri for a DEB bundle only. That DEB is a staging payload, never a
published download: it is extracted, layout-checked, and its packaged
engine is smoke-tested, then Arch's `makepkg` packages the same verified
payload inside an Arch container. No end-user Python, Rust, or Node
installation is required.

To build the Arch package from a locally built staging DEB:

```sh
python3 scripts/package_downloads.py package-arch --deb dist/staging/Krutrim-Mulyankan-linux.deb
```

Run it as a normal user with `makepkg`, `fakeroot`, Python, libarchive
(`bsdtar`), zstd, and desktop-file-utils available. The output goes to
`dist/downloads/Krutrim-Mulyankan-linux.pkg.tar.zst`. The command does
not install the package on the build host.

Package input archives use sorted paths, root ownership, normalized
file modes, and `SOURCE_DATE_EPOCH` (default: the source commit time).
The Arch build records its packaging environment in `.BUILDINFO`.

These preview packages are unsigned. Release checksums establish which
files were published; they are not package signing certificates.

Packaging references: [Tauri's Arch packaging guide](https://v2.tauri.app/distribute/aur/)
and the [Arch PKGBUILD reference](https://man.archlinux.org/man/PKGBUILD.5.en).
