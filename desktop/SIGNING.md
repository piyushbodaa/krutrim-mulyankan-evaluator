# Code signing (later)

The GitHub Actions workflows build **unsigned** Linux installers. Pull requests upload them as artifacts. A `v*` tag publishes the same builds, still unsigned, on GitHub Releases under the names in `.github/workflows/release.yml`.
Nobody has to enter a password or approve a certificate for those builds.

| Platform | What CI produces | What "unsigned" means |
|---|---|---|
| Linux | `.deb`, `.rpm`, `.AppImage`, `.pkg.tar.zst` | No GPG signature on the package. |

## When you are ready to sign

**Linux (optional).** Sign the `.deb` with `dpkg-sig` and publish a repo key, or sign the AppImage with `gpg`.

Do not commit certificates or signing passwords. Until that work lands, ship the CI artifacts as unsigned downloads and say so on the download page.
