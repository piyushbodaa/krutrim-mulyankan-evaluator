# Krutrim Mulyankan desktop

A window for testing AI with your own questions, made for people who have never run an "eval". It has three steps and uses everyday words:

1. **Questions.** Pick a ready-made test, or write your own: type each question, its right answer, and how to check the reply ("mentions this", "is a number", "is exactly this"). Starter examples (shop maths in rupees, India general knowledge, Hindi and English words) can be saved and edited.
2. **AIs.** Pick up to six. Two free demo bots need no account: the Answer-key bot (always right) and the Parrot bot (repeats the question, so it is usually wrong). Real AIs show the most they can cost for this test, in rupees.
3. **Start.** One screen shows the total, a spending limit in rupees, and, for paid AIs, an "I agree" box next to the Start button.

Results open as a report card: one tick or cross per answer, time taken, cost, and a plain "too close to call" note when the gap could be luck. For tests you wrote, **Every answer** shows what each AI replied next to the right answer. **Charts and details** is the same At a glance view as before. The command line is unchanged for people who want it.

Your own tests are saved as ordinary test packs in the app's data folder (`runs/my-tests/`). They never leave the computer except as questions sent to the AIs you pick.

The window does not ask anyone to open a terminal. Installers from CI are unsigned; signing is a later step ([SIGNING.md](SIGNING.md)).

## Honesty labels

These stay visible, in English, in light and dark mode (in the footer):

- Unvalidated pilot
- Not a ranking
- Too close to call — the gap could be chance, not that the systems are equal

Ready-made pilot questions and answers stay hidden in the app; replies are shown only for tests people write themselves.

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

## Install on Linux, including Arch / Omarchy

Download `Krutrim-Mulyankan-linux-setup.tar.gz` from the project download page.
Extract it in Files, open the extracted folder, and double-click **Install Krutrim
Mulyankan**. Choose **Install & Open**. Setup installs for your account, adds an app
launcher and a **Manage installation** action, and opens the app. It includes the
application and needs no terminal, administrator password, or FUSE. A Linux x86-64
desktop with GTK 3 is required. The executable permission is preserved inside the
archive, so the installer must be extracted before opening it.

Native Debian/Ubuntu, RPM, and Arch packages are alternatives for system package
managers; see [NATIVE-LINUX.md](NATIVE-LINUX.md). The raw AppImage remains a portable
option that requires permission to execute. It is not the graphical installer.

The window turns off WebKit's DMA-BUF renderer unless you have already set
`WEBKIT_DISABLE_DMABUF_RENDERER`. On Hyprland it also turns off compositing mode so
the window is not blank. Keys use the Secret Service (gnome-keyring or equivalent).
If that service is locked, the window still opens and practice checks still run.

## Native builds and GitHub Releases

`.github/workflows/build-downloads.yml` is shared by desktop CI and releases.
It builds on Ubuntu 22.04 (x86-64) only. The runner builds the CLI, Rust engine,
and Tauri shell. No Python/PyInstaller engine is bundled.

Pull requests, pushes to `main`, and manual runs of **Desktop installers** upload
artifacts for review. Separately, merging synchronized package version changes to
`main` starts **Release**. It skips an already-tagged version; for a new version,
it runs all native builds and Python checks, then creates `v<version>` at that
exact tested commit and publishes the complete download set. An explicit matching
`v<version>` tag remains another way to start the same release flow.

Publication is serialized by version and rechecks the remote tag immediately
before publishing. Existing tags are never moved, including when retrying a failed
publish job. Availability means the files appear in a completed GitHub Release;
configuring a platform does not itself make it released.

| File | What it is |
|---|---|
| `Krutrim-Mulyankan-linux-setup.tar.gz` | Offline graphical Linux x86-64 installer; extract, then open Install Krutrim Mulyankan. |
| `Krutrim-Mulyankan-linux.AppImage` | Linux x86-64 desktop app. |
| `Krutrim-Mulyankan-linux.deb` | Debian/Ubuntu x86-64 desktop package. |
| `Krutrim-Mulyankan-linux.rpm` | Fedora/openSUSE x86-64 desktop package. |
| `Krutrim-Mulyankan-linux.pkg.tar.zst` | Arch/Omarchy x86-64 desktop package. |
| `km-desktop-linux.tar.gz` | Extracted Linux app for systems without FUSE. |
| `km-linux-x86_64.tar.gz` | Linux x86-64 static CLI. |
| `kritrim_mulyankan-<version>-py3-none-any.whl` | Python CLI package for `pip`. |
| `SHA256SUMS` | SHA-256 hashes for release files. |

Builds have no verified publisher certificate. See [SIGNING.md](SIGNING.md).

The engine is an executable with stdin/stdout pipes. Keys use the Linux
Secret Service.

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
