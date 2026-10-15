# Native Linux packages

The release provides x86-64 packages for Debian/Ubuntu (`.deb`), Fedora/openSUSE
(`.rpm`), and Arch/Omarchy (`.pkg.tar.zst`). Package managers install the executable,
desktop launcher, icons, resources, and declared system dependencies. Opening a
downloaded package graphically requires a package installer on that desktop.
The separate Linux setup archive provides installation to a user's home folder.

Release filenames:

- `Krutrim-Mulyankan-linux.deb`
- `Krutrim-Mulyankan-linux.rpm`
- `Krutrim-Mulyankan-linux.pkg.tar.zst`
- `Krutrim-Mulyankan-linux-setup.tar.gz`

Native package name: `krutrim-mulyankan`. Tauri's executable remains
`/usr/bin/krutrim-mulyankan-desktop`; Arch and converted RPM packages also provide
the `krutrim-mulyankan` command. Their application ID is
`in.krutrimmulyankan.evaluator`. Native packages use the desktop Secret Service
for persistent API keys; GNOME Keyring is an optional provider.

## Normal release builds

The shared download workflow builds the shell on Ubuntu 22.04 and asks Tauri for
DEB, RPM, and AppImage bundles. The RPM configuration declares shared-library
capabilities so Fedora and openSUSE can resolve their respective package names.
Arch's `makepkg` packages the same verified DEB payload inside an Arch container.
No end-user Python, Rust, or Node installation is required.

All packages are extracted for validation. The checks validate launcher syntax,
icon presence, executable permissions, metadata, and the packaged engine's free
example run. They do not substitute for an interactive install on each target OS.

## Add native packages to an existing release

Start with the checksum-verified DEB from that release and a matching source
version. These commands preserve its shell and engine binaries:

```sh
python3 scripts/package_downloads.py package-arch-from-deb --deb /path/to/released.deb
python3 scripts/package_downloads.py package-rpm-from-deb --deb /path/to/released.deb
```

The Arch command runs as a normal user and needs `makepkg`, `fakeroot`, Python,
libarchive (`bsdtar`), zstd, and desktop-file-utils. The RPM command needs Python,
`rpmbuild`, `rpm`, libarchive, and desktop-file-utils. The GitHub runners provide
these tools. Outputs go to `dist/downloads/` with the stable release names above.

Package input archives use sorted paths, root ownership, normalized file modes,
and `SOURCE_DATE_EPOCH` (default: the source commit time). The Arch build records
its packaging environment in `.BUILDINFO`; RPM additionally infers ELF runtime
requirements. Neither command installs the package on the build host.

These preview packages are unsigned. Release checksums establish which files
were published; they are not package signing certificates.

Packaging references: [Tauri RPM](https://v2.tauri.app/distribute/rpm/),
[Tauri's Arch packaging guide](https://v2.tauri.app/distribute/aur/), and
[Arch PKGBUILD reference](https://man.archlinux.org/man/PKGBUILD.5.en).
