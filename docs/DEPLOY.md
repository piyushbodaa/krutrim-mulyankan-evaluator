# Launch and deployment

## Run locally (full evaluator)

```sh
git clone https://github.com/piyushbodaa/krutrim-mulyankan-evaluator.git
cd krutrim-mulyankan-evaluator
uv sync
uv run km --help                       # CLI
uv run km run example -r echo -r oracle   # zero-cost mock controls
uv run km ui --port 8765               # dashboard at http://127.0.0.1:8765/
# or: KM_PYTHON=.venv/bin/python bash scripts/run-local.sh 8765
```

The dashboard server binds to `127.0.0.1` only, rejects non-local Host headers, and
requires a per-launch token plus same-origin checks for every write. Do not expose it
to a network or put it behind a public proxy: it can launch local CLI harnesses.

Provider API keys (optional) go in a local `.env` copied from `.env.example`; paid
calls need `--allow-paid` and an explicit budget. Nothing is called by default.

## Hosted preview (static, Vercel)

`preview/` is a static build of the same dashboard wired to synthetic fixtures. It
makes no network requests, calls no model and cannot execute anything. Deploy it as a
plain static site, e.g. with the Vercel CLI:

```sh
cd preview && npx vercel deploy --prod   # framework: Other, no build command
```

What stays **local-only**: Claude/Muse/other CLI harness runs, model-API runs,
recording app observations, comparisons/imports, saved results and evidence.
Never deploy `local_ui.py` as a public server.
