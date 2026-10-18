# Krutrim Mulyankan — evaluator

A local-first tool to test and compare AI models, CLI harnesses and personal agents on
the same frozen tasks, with deterministic grading, cost preflight, progress,
cancellation/retry and evidence export. It powers the Krutrim Mulyankan India pilot.

> **Status: unvalidated prototype.** The bundled tasks are **synthetic smoke
> fixtures**. Nothing in this repository is a model ranking or a benchmark result.

**Hosted dashboard preview:** see the link in the repository's About section. It is a
static, synthetic-data demo; nothing executes there (details below).

## What it does

- **Your own micro-eval**: `km history` inspects your ChatGPT/Claude export or
  Claude Code sessions offline (counts only). `km micro-eval` then has an LLM
  judge automatically re-check every prompt you asked, keeps the ones your model
  answered wrongly (confident verdicts only), and freezes them into a personal
  suite with expected answers, rubrics and full provenance. Run that suite
  against frontier and open-source models to find the best model *for your own
  work*. See "Build a micro-eval from your own history" below.
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

## Build a micro-eval from your own history

Bring your own data; it never leaves this machine during import.

- **ChatGPT**: Settings → Data controls → Export. Pass the whole `.zip`
  (or the `conversations.json` inside it).
- **Claude**: Settings → Privacy → Export data. Pass the `.zip` or its JSON.
- **Claude Code**: pass `~/.claude/projects` (or any `*.jsonl` session file).

```sh
# 0. Needs a judge. Free and local: any Ollama model. Or set ANTHROPIC_API_KEY
#    (or `ant auth login`) and use a priced Anthropic model like claude-haiku-4-5.
#    OpenRouter judges need an entry in prices.yaml first (see `km list prices`).

# 1. Look first — offline, counts only, prints no prompt content.
uv run km history ~/Downloads/chatgpt-export.zip ~/.claude/projects

# 2. Price the judging without calling anything.
uv run km micro-eval ~/Downloads/chatgpt-export.zip \
  --judge ollama/llama3.2:3b --dry-run

# 3. Mine failures into suites/my-micro-eval.yaml (+ private evidence sidecar).
uv run km micro-eval ~/Downloads/chatgpt-export.zip ~/.claude/projects \
  --judge anthropic/claude-haiku-4-5 --allow-paid --max-usd 2

# 4. Free sanity check that grading works (oracle = reference answer, echo = floor).
uv run km run my-micro-eval -r echo -r oracle --judge ollama/llama3.2:3b

# 5. Real comparison on your own failed prompts. Costs are shown before anything runs.
uv run km run my-micro-eval \
  -r model:anthropic/claude-sonnet-4-6 -r model:ollama/llama3.2:3b \
  --judge anthropic/claude-haiku-4-5 --allow-paid --max-usd 5
uv run km report   # report card: right/wrong, speed and cost on YOUR questions
```

How mining decides: a judge model re-checks each of your prompt/answer pairs.
Only a **wrong** verdict with confidence ≥ `--min-confidence` (default 0.7)
*and* a stated expected answer becomes a task; unclear prompts are dropped, and
judge failures are skipped, never guessed. Every task records its source
conversation, the model that originally answered, the judge spec, the judge
confidence/reason and the original answer's hash. The full original Q&A goes to
`my-micro-eval.evidence.json` next to the suite — **keep that file private and
out of git** (`suites/*.evidence.json` belongs in your `.gitignore` when in a
repo). Caveat stated plainly: the judge can be wrong, so these scores are
judge-graded evidence about your prompts, not ground truth.

The generated suite also appears in `km ui` like any other suite. The desktop
app and `km ui` import screens for this flow are the next step; today import and
mining run from the CLI above.

## Desktop app

A window for the same engine, so a terminal is not required. Keys stay in the OS keychain.
The pilot set is offered first, with a cost check in INR and USD before anything runs. Results
reuse the At a glance dashboard in `preview/`. Development and unsigned installers:
[desktop/README.md](desktop/README.md). Code signing is a later step
([desktop/SIGNING.md](desktop/SIGNING.md)). The `km` command is unchanged.

The native build workflow produces the CLI and desktop app for Linux x86-64
only. Pull requests and manual
runs upload build artifacts. Merging a version bump to `main`, or pushing a matching
`v<version>` tag, publishes on GitHub Releases only after every platform passes.
Check the release assets for current availability.

| Platform | Desktop | CLI archive |
|---|---|---|
| Linux x86-64 | `Krutrim-Mulyankan-linux.AppImage`, `Krutrim-Mulyankan-linux.deb` | `km-linux-x86_64.tar.gz` |

CLI archives contain `km` (or `km.exe`) and `suites/`. They need neither Python nor
Rust installed. The Python wheel remains a separate option. Releases also include
`SHA256SUMS`. Installers have no verified publisher certificate. See [desktop/SIGNING.md](desktop/SIGNING.md).

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
