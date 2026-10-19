# Code signing (later)

The GitHub Actions workflows build **unsigned** Linux installers. Pull requests upload them as artifacts. A `v*` tag publishes the same builds, still unsigned, on GitHub Releases under the names in `.github/workflows/release.yml`.
Nobody has to enter a password or approve a certificate for those builds.

| Platform | What CI produces | What "unsigned" means |
|---|---|---|
| Arch / Omarchy | `.pkg.tar.zst` | No GPG signature on the package. |

## When you are ready to sign

**Arch (optional).** Sign the package with `makepkg --sign` and publish the public key.

Do not commit certificates or signing passwords. Until that work lands, ship the CI artifacts as unsigned downloads and say so on the download page.
