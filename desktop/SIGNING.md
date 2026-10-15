# Code signing (later)

The GitHub Actions workflows build **unsigned** installers. Pull requests upload them as artifacts. A `v*` tag publishes the same builds, still unsigned, on GitHub Releases under the names in `.github/workflows/release.yml`.
Nobody has to enter a password or approve a certificate for those builds.

| Platform | What CI produces | What "unsigned" means |
|---|---|---|
| Linux | `.deb` and `.AppImage` | No GPG signature on the package. |
| Windows | NSIS `.exe` | No Authenticode certificate. SmartScreen will warn. |
| macOS | `.dmg` | Ad-hoc signature only (`signingIdentity` is `"-"` in `src-tauri/tauri.conf.json`). Not notarized; macOS may require approval in Privacy & Security. This is not a verified publisher signature. |

## When you are ready to sign

1. **Apple.** Enroll in the Apple Developer Program. Create a Developer ID Application certificate. Replace the macOS `signingIdentity` of `"-"` with that identity, set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` as repository secrets, and notarize with `notarytool`. Tauri's bundler can do this when those secrets exist.
2. **Windows.** Buy an Authenticode certificate (or use Azure Trusted Signing). Set `certificateThumbprint` in the Tauri Windows bundle config, or sign the `.exe` after the build with `signtool`.
3. **Linux.** Optional. Sign the `.deb` with `dpkg-sig` and publish a repo key, or sign the AppImage with `gpg`.

Do not commit certificates, `.p12` files, or notarization passwords. Until that work lands, ship the CI artifacts as unsigned downloads and say so on the download page.

Official guidance: [Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/) and [Windows signing](https://v2.tauri.app/distribute/sign/windows/).
