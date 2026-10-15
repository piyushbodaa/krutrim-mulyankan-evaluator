# Rust evaluator (`km` + `km-desktop`)

One shared Rust evaluator crate powers both shipped binaries, so the desktop
app no longer bundles a Python/PyInstaller engine and the `km` download is a
single native binary (static on Linux). The Python package and wheel stay in place; behaviour
parity with them is tested (see below).

```
crates/evaluator   suites, engine, graders, store (same SQLite v3 file),
                   pricing, verdicts/statistics, report, export, provenance
crates/km          the `km` CLI (list, run, import-suite, export, report)
crates/km-desktop  the desktop engine: JSON-lines stdio protocol for the
                   Tauri shell, loopback results dashboard, `--http` preview
```

## Build

```bash
cargo build --release --manifest-path km-rs/Cargo.toml            # both binaries
cargo build --release --manifest-path km-rs/Cargo.toml -p km-cli \
    --target x86_64-unknown-linux-musl                            # static km (release asset)
cargo test --manifest-path km-rs/Cargo.toml
```

Local Linux x86-64 GNU release measurements for v0.3.0: `km` is 5,796,360
bytes and `km-desktop` is 4,912,968 bytes before desktop resources. Across 20
warm `km --help` process launches, median startup was 2.07 ms; one free
`example` oracle/echo run took 38.38 ms. These measure software overhead on one
machine, not model speed or Windows/macOS performance. Download sizes vary by
platform and system webview packaging.

## Parity

- `tests/test_rust_parity.py` (repo root) runs the Python and Rust `km` on
  the bundled suites and compares `results.json`, `manifest.json` and
  `report.md` after removing volatile fields.
- `crates/evaluator/tests/parity_units.rs` pins CPython-compatible pieces:
  task content hashes, derived seeds, and the seeded bootstrap intervals.

## Windows and macOS downloads

Native CI produces `km-windows-x86_64.zip`, `km-macos-aarch64.tar.gz` (Apple Silicon),
and `km-macos-x86_64.tar.gz` (Intel), alongside the Linux archive. Each contains the
binary and `suites/`; the shipped synthetic suites are also embedded in the binary.
The same evaluator crate powers every desktop installer.

After extracting the Windows ZIP, open PowerShell in that folder:

```powershell
.\km.exe --help
.\km.exe run example -r oracle -r echo
.\km.exe
```

On a Mac, extract the matching archive and run:

```sh
./km --help
./km run example -r oracle -r echo
./km
```

The final command opens the terminal interface when attached to a terminal.
No Python or Rust installation is needed. The mock controls above are free and
are software checks, not AI benchmark results.

## Current interfaces

- Model API runners use the same cost preflight and explicit paid-run approval as
  the Python engine. Tests use local mock endpoints and do not spend money.
- `harness:claude-draft` uses an installed Claude CLI and still requires fresh
  included-plan access proof. Generic harness/agent adapters remain stubs.
- `km ui` starts the local workbench; the desktop app uses the Rust engine bridge.
- `llm_judge` remains pending without a configured judge.
- Non-interactive help is plain text; the CLI also has an interactive terminal UI.

See [desktop build and release details](../desktop/README.md) for native packaging,
artifact names, signing limitations, and how builds are checked.
