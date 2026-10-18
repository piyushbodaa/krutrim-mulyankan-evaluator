# Rust evaluator (`km` + `km-desktop`)

One shared Rust evaluator crate powers both shipped binaries, so the desktop
app no longer bundles a Python/PyInstaller engine and the `km` download is a
single native binary (static on Linux). The Python package and wheel stay in place; behaviour
parity with them is tested (see below).

```
crates/evaluator   suites, engine, graders, store (same SQLite v3 file),
                   pricing, verdicts/statistics, report, export, provenance,
                   history import, failure miner, micro-eval builder, judge
crates/km          the `km` CLI (history, micro-eval, list, run [--judge],
                   import-suite, export, report)
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

Local Linux x86-64 GNU release measurements, v0.4.0 with the history/mining
pipeline, 2026-10-18, median of repeated warm runs on one machine (software
overhead only, not model speed):

| Measurement | Rust `km` | Python `km` |
|---|---|---|
| Binary / environment size | 7.3 MB single binary | ~96 MB virtualenv |
| `km --help` startup | 0.8 ms | 1,106 ms |
| `km run example` (oracle+echo) | 16 ms | — |
| `km history` on 337 MB of Claude Code sessions | 586 ms | 2,547 ms |

The release profile uses `opt-level = 2` (measured fastest on the history
parse; `"z"` was 5.91 MB but 26% slower there, `3` was 8.38 MB and no
faster), fat LTO, one codegen unit, and stripping. The shipped Linux CLI is
the Musl static build; CI builds and smoke-tests it on every change.

## Parity

- `tests/test_rust_parity.py` (repo root) runs the Python and Rust `km` on
  the bundled suites and compares `results.json`, `manifest.json` and
  `report.md` after removing volatile fields.
- `crates/evaluator/tests/parity_units.rs` pins CPython-compatible pieces:
  task content hashes, derived seeds, and the seeded bootstrap intervals.

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
