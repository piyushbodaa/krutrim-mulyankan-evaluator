# Krutrim Mulyankan desktop

A window for the evaluator. People add provider keys, pick models and a test pack, see the worst-case cost in rupees and dollars, confirm, and run. Results open in the same At a glance view (report card, speedometer, third umpire). The command line is unchanged for people who want it.

The window does not ask anyone to open a terminal. Installers from CI are unsigned; signing is a later step ([SIGNING.md](SIGNING.md)).

## Honesty labels

These stay visible, in English, in light and dark mode:

- Unvalidated pilot
- Not a ranking
- Too close to call — the gap could be chance, not that the systems are equal

The pilot set is the first test pack. Practice checks (correct-answer and echo) cost nothing and are not AI models.

## Keys

Keys are saved in the operating system keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service). The engine process receives them in memory. The app does not write a `.env` or any other secret file. Each provider card has a **Get a key** link:

- OpenAI — <https://platform.openai.com/api-keys>
- Anthropic — <https://console.anthropic.com/settings/keys>
- Google — <https://aistudio.google.com/apikey>
- xAI — <https://console.x.ai/>
- OpenRouter — <https://openrouter.ai/keys>

Models with no checked price in the bundled price table stay disabled. A paid run is refused until you confirm a spending cap that covers the worst case.

## Run it locally (development)

You need Node, Rust, the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/), and Python for the development web server. People who install a built app do not need these build tools.

```sh
cargo build --locked --manifest-path km-rs/Cargo.toml -p km-desktop-engine
cd desktop
npm ci
npm run dev
```

The shell finds the native engine in `km-rs/target/debug/` or `release/`, including `km-desktop.exe` on Windows. For Python-engine comparison only, run `uv sync` and set `KM_PYTHON` to the absolute virtualenv Python path before starting the app.

`npm run dev` opens the window. It serves the screens from `ui/` and starts the Rust `km-desktop` engine. Results are loaded from `preview/` in an iframe. That folder is not modified; a script tag is inserted only in the served copy, so design work on `preview/` still applies.

A loopback page, useful for screenshots and for clicking through without Rust:

```sh
KM_RESOURCE_ROOT="$PWD" uv run python -m kritrim_mulyankan.desktop_bridge --http 17322
# open http://127.0.0.1:17322/
```

Do not expose that port. It is bound to `127.0.0.1`.

## Tests

```sh
uv run pytest tests/test_desktop_bridge.py
uv run ruff check src/kritrim_mulyankan/desktop_bridge.py tests/test_desktop_bridge.py
```

The rest of `uv run pytest` stays required. The CLI (`uv run km …`) is the same tool it was.

## Try it on Arch / Omarchy (Hyprland, Wayland)

Install the system library the window draws with, then run either package. No terminal is needed after the first launch; the two commands below are only how you start the file.

```sh
sudo pacman -S webkit2gtk-4.1 gtk3
chmod +x Krutrim-Mulyankan_*.AppImage
./Krutrim-Mulyankan_*.AppImage
```

If AppImage reports that FUSE is missing, unpack the `.tar.gz` and run `AppRun` in that folder, or start the AppImage with `--appimage-extract-and-run`. The window turns off WebKit's DMA-BUF renderer unless you have already set `WEBKIT_DISABLE_DMABUF_RENDERER`. On Hyprland it also turns off compositing mode so the window is not blank. Keys use the Secret Service (gnome-keyring or equivalent). If that service is locked, the window still opens and practice checks still run.

## Native builds and GitHub Releases

`.github/workflows/build-downloads.yml` is shared by desktop CI and tagged releases.
It builds on Ubuntu 22.04 (x86-64), Windows Server 2022 (x86-64), macOS 15
(Apple Silicon), and macOS 15 Intel. Each runner builds its own CLI, Rust engine,
and Tauri shell. No Python/PyInstaller engine is bundled.

Pull requests, pushes to `main`, and manual runs of **Desktop installers** upload
artifacts for review without publishing a release. Pushing a `v<version>` tag
matching the project versions runs the same builds and publishes only after all
platforms and the Python checks succeed. Availability means the files appear in a
completed GitHub Release; configuring a platform does not itself make it released.

| File | What it is |
|---|---|
| `Krutrim-Mulyankan-linux.AppImage` | Linux x86-64 desktop app. |
| `Krutrim-Mulyankan-linux.deb` | Debian/Ubuntu x86-64 desktop package. |
| `km-desktop-linux.tar.gz` | Extracted Linux app for systems without FUSE. |
| `Krutrim-Mulyankan-windows.exe` | Windows x86-64 NSIS installer. |
| `Krutrim-Mulyankan-mac-arm64.dmg` | Apple Silicon Mac app disk image. |
| `Krutrim-Mulyankan-mac-x86_64.dmg` | Intel Mac app disk image. |
| `km-linux-x86_64.tar.gz` | Linux x86-64 static CLI. |
| `km-windows-x86_64.zip` | Windows x86-64 CLI (`km.exe`). |
| `km-macos-aarch64.tar.gz` | Apple Silicon Mac CLI. |
| `km-macos-x86_64.tar.gz` | Intel Mac CLI. |
| `kritrim_mulyankan-<version>-py3-none-any.whl` | Python CLI package for `pip`. |
| `SHA256SUMS` | SHA-256 hashes for release files. |

For Windows, run the `.exe` installer. For macOS, open the disk image and drag the
app into Applications. Choose Apple Silicon for M-series Macs; choose Intel for
Intel Macs. Windows builds have no Authenticode certificate; macOS builds have an
ad-hoc signature and are not notarized. See [SIGNING.md](SIGNING.md).

The engine is an executable with stdin/stdout pipes. Windows uses
`CREATE_NO_WINDOW` for the engine so opening the desktop app does not open a
console window. Keys use macOS Keychain, Windows Credential Manager, or Linux
Secret Service. Windows CLI keys use the user's application-data directory.

Each native job runs Rust tests, extracts and exercises the CLI archive, checks the
engine's JSON-lines protocol, and verifies the engine and resources inside the
built desktop package. These checks do not replace an interactive check of the
window, system keychain, and installer prompts on a real machine.

## Build an installer on your own platform

From the repository root, build and stage the engine before building the shell:

```sh
cargo build --release --locked --manifest-path km-rs/Cargo.toml -p km-desktop-engine
```

Follow the staging and bundling commands in `.github/workflows/build-downloads.yml`;
they copy the engine, suites, dashboard, and bridge resources before `npm ci` and
`npm run build`. Use `--bundles nsis` on Windows, `--bundles dmg` on macOS, or
`--bundles appimage,deb` on Linux. Build each Mac architecture on its matching
runner so the shell and engine always match.

## Language and appearance

The shell is English and Hindi. The embedded results page keeps its own wording. Appearance follows System, Light, or Dark, using the same palette and the approved Chakra Engine logo files (copied byte-for-byte; do not redraw them).
