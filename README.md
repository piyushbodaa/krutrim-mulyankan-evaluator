# Krutrim Mulyankan — evaluator

A local-first tool to test and compare AI models, CLI harnesses and personal agents on
the same frozen tasks, with deterministic grading, cost preflight, progress,
cancellation/retry and evidence export. It powers the Krutrim Mulyankan India pilot.

> **Status: unvalidated prototype.** The bundled tasks are **synthetic smoke
> fixtures**. Nothing in this repository is a model ranking or a benchmark result.

**Hosted dashboard preview:** see the link in the repository's About section. It is a
static, synthetic-data demo; nothing executes there (details below).

## What it does

- `km` CLI: list suites/tasks/runners/prices, run suites, grade, report, export JSON.
- `km ui`: a loopback-only dashboard (http://127.0.0.1:8765/) to choose tasks and
  adapters, review the run plan, start/cancel/retry jobs, watch progress, inspect
  results and evidence, compare recorded app answers, and record app observations.
- **At a glance**: the view both UIs open on. It explains results for people new to AI with
  everyday instruments: a report card with phone-signal bars, a speedometer, a chat replay of the
  measured waits and a phone-style comparison table. It reads outcomes and timings only.
- Versioned machine API (`/api/v1`) with JSON schemas in `docs/`.
- Separate tracks that are never mixed into one ranking: **mock controls**
  (oracle/echo, zero cost), **CLI harnesses** (e.g. a tool-free Claude Code draft
  using your own installed, signed-in CLI), **model APIs** (paid; off unless you pass
  `--allow-paid` and a budget) and **recorded consumer-app observations**.

## Quick start

```sh
uv sync
uv run km run example -r echo -r oracle   # free mock controls
uv run km ui --port 8765                  # open http://127.0.0.1:8765/
uv run pytest                              # offline tests
```

## Desktop app

A window for the same engine, so a terminal is not required. Keys stay in the OS keychain.
The pilot set is offered first, with a cost check in INR and USD before anything runs. Results
reuse the At a glance dashboard in `preview/`. Development and unsigned installers:
[desktop/README.md](desktop/README.md). Code signing is a later step
([desktop/SIGNING.md](desktop/SIGNING.md)). The `km` command is unchanged.

The native build workflow produces the CLI and desktop app for Linux x86-64,
Windows x86-64, and both Apple Silicon and Intel Macs. Pull requests and manual
runs upload build artifacts. Merging a version bump to `main`, or pushing a matching
`v<version>` tag, publishes on GitHub Releases only after every platform passes.
Check the release assets for current availability.

| Platform | Desktop | CLI archive |
|---|---|---|
| Linux x86-64 | `Krutrim-Mulyankan-linux.AppImage`, `Krutrim-Mulyankan-linux.deb` | `km-linux-x86_64.tar.gz` |
| Windows x86-64 | `Krutrim-Mulyankan-windows.exe` | `km-windows-x86_64.zip` |
| macOS Apple Silicon | `Krutrim-Mulyankan-mac-arm64.dmg` | `km-macos-aarch64.tar.gz` |
| macOS Intel | `Krutrim-Mulyankan-mac-x86_64.dmg` | `km-macos-x86_64.tar.gz` |

CLI archives contain `km` (or `km.exe`) and `suites/`. They need neither Python nor
Rust installed. The Python wheel remains a separate option. Releases also include
`SHA256SUMS`. Installers have no verified publisher certificate; macOS builds use
an ad-hoc signature and are not notarized. See [desktop/SIGNING.md](desktop/SIGNING.md).

For a new version merged to `main`, the release workflow creates its tag at the
exact tested commit after all checks pass. Existing versions are skipped and tags
are never moved. See [desktop/README.md](desktop/README.md) for native build commands
and installer checks.

More: [docs/DEPLOY.md](docs/DEPLOY.md) (launch + hosted preview), [CONTRIBUTING.md](CONTRIBUTING.md).

## Online vs local-only

| Feature | Hosted preview | Local (`km ui`) |
|---|---|---|
| Browse synthetic tasks, choose adapters, review run plan | ✅ | ✅ |
| Inspect sample oracle/echo results (labelled synthetic) | ✅ | ✅ |
| Run mock-control jobs, cancel, retry | ❌ | ✅ |
| Claude/Muse/other CLI harness runs | ❌ | ✅ (your own signed-in CLI) |
| Model-API runs (paid, budget-gated) | ❌ | ✅ |
| Record app observations, compare/import answers, saved evidence | ❌ | ✅ |

The hosted preview has no backend, makes no network requests and holds no secrets.

## Data and privacy

This is a clean public snapshot with fresh history. It contains no credentials, `.env`
files, databases, run outputs, recordings, personal data, private Git history or the
original pilot datasets/gold answers. Your own `runs/` and `.env` are git-ignored;
keep them private. See [NOTICE.txt](NOTICE.txt).

## Licence

MIT for the original code — see [LICENSE](LICENSE). Dependencies keep their own
licences (MIT / Apache-2.0 / BSD), listed in NOTICE.txt.
