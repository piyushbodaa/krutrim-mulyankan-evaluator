# Rust evaluator (`km` + `km-desktop`)

One shared Rust evaluator crate powers both shipped binaries, so the desktop
app no longer bundles a Python/PyInstaller engine and the `km` download is a
single static binary. The Python package and wheel stay in place; behaviour
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

Locally measured (this machine): static musl `km` 3,746,304 bytes (the
previous PyInstaller bundle was 23.5 MB after slimming); `km-desktop`
3,530,696 bytes before the preview/suites resources copied beside it.

## Parity

- `tests/test_rust_parity.py` (repo root) runs the Python and Rust `km` on
  the bundled suites and compares `results.json`, `manifest.json` and
  `report.md` after removing volatile fields.
- `crates/evaluator/tests/parity_units.rs` pins CPython-compatible pieces:
  task content hashes, derived seeds, and the seeded bootstrap intervals.

## Not ported yet

- `model:` runners (network calls) are refused at preflight with a clear
  message; `harness:`/`agent:` remain stubs, as in Python. Paid runs still
  need the Python `km` for now.
- `km ui` (local workbench) is not in the Rust CLI; use the desktop app or
  the Python `km ui`.
- `llm_judge` behaves exactly like the Python CLI default: pending without
  a configured judge.
- Help text is plain; Typer's rich boxed help layout is not reproduced.
  Commands, flags and run output follow the Python CLI.
