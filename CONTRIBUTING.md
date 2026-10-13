# Contributing

Thanks for helping improve Krutrim Mulyankan.

## Setup

```sh
uv sync                 # Python 3.12+, installs runtime + dev dependencies
TERM=xterm uv run pytest
uv run ruff check . && uv run ruff format --check .
```

## Ground rules

- **Never commit secrets or private data.** No `.env`, API keys, tokens, `runs/`,
  `*.db`, recordings, screenshots of accounts, or real people's answers. `.gitignore`
  already blocks the common cases; check `git status` before every commit.
- **Keep tracks separate.** Mock controls, CLI harness runs, model-API runs and
  recorded consumer-app observations must never be mixed into one ranking.
- **Label synthetic data.** Any fixture that is not a real, reviewed observation must
  say so in its title/notes and use `test_fixture` / synthetic kinds.
- **No paid calls in tests.** Tests must run offline with the oracle/echo mock runners.
- Keep the local server bound to loopback and keep its token/origin checks intact.

## Pull requests

1. Open an issue describing the change for anything non-trivial.
2. Add or update tests; the full suite must pass.
3. Describe what you ran and on what data in the PR.

By contributing you agree your contribution is licensed under the MIT License.
