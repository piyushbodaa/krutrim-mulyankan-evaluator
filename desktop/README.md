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

You need Python 3.12, Node, Rust, and the Tauri system libraries. People who install a built app do not need these.

```sh
uv sync
cd desktop
npm install
# Point at the project virtualenv so the window can start the engine.
KM_PYTHON="$(cd .. && pwd)/.venv/bin/python" npm run dev
```

On Windows, set `KM_PYTHON` to `..\.venv\Scripts\python.exe` in PowerShell before `npm run dev`.

`npm run dev` opens the window. It serves the screens from `ui/` and starts `python -m kritrim_mulyankan.desktop_bridge`. Results are loaded from `preview/` in an iframe. That folder is not modified; a script tag is inserted only in the served copy, so design work on `preview/` still applies.

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

## GitHub Releases

Pushing a tag `v` plus the version in `pyproject.toml` (for example `v0.1.0`) runs `.github/workflows/release.yml`. It does not create the tag. The release assets, which the site can link as `/releases/latest/download/<file>`, are:

| File | What it is |
|---|---|
| `Krutrim-Mulyankan-linux.AppImage` | Desktop app for Linux, including Arch and Omarchy. |
| `Krutrim-Mulyankan-windows.exe` | Desktop app installer for Windows. |
| `Krutrim-Mulyankan-mac.dmg` | Desktop app disk image from the macOS runner. |
| `kritrim_mulyankan-<version>-py3-none-any.whl` | The `km` package for `pip`. |
| `km-linux-x86_64.tar.gz` | Single-file `km` with `suites/` beside it and packed inside. |

## Installers

`.github/workflows/desktop.yml` builds an unsigned installer on Linux, Windows, and macOS. Each job bundles a PyInstaller engine (`desktop/sidecar/entry.py`) and runs `tauri build`. Artifacts are named `km-desktop-Linux`, `km-desktop-Windows`, and `km-desktop-macOS`.

The release binary is a normal executable with stdin/stdout pipes. On Windows the window starts it with `CREATE_NO_WINDOW`, so no console appears. Build the sidecar as a console program; a windowed PyInstaller build would drop those pipes.

## Language and appearance

The shell is English and Hindi. The embedded results page keeps its own wording. Appearance follows System, Light, or Dark, using the same palette and the approved Chakra Engine logo files (copied byte-for-byte; do not redraw them).
