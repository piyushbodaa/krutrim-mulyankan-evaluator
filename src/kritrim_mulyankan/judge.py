"""Build a judge callable (`Callable[[str], str]`) from a `<provider>/<model>` spec.

Used both to mine failures from the user's history and to grade `llm_judge`
tasks during a run. Calls go through the same provider adapters as model
runs, with the same credential checks. Pricing is enforced by callers before
any call is made; this module only performs the call it was asked for.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Mapping

import httpx2

from kritrim_mulyankan.runners.providers import PROVIDERS, credentials_problem, make_adapter

JudgeFn = Callable[[str], str]

DEFAULT_JUDGE_MAX_TOKENS = 1024
DEFAULT_JUDGE_TIMEOUT_S = 120.0


def parse_judge_spec(spec: str) -> tuple[str, str]:
    provider, sep, model = spec.partition("/")
    if not (provider and sep and model):
        raise ValueError(f"judge spec must be <provider>/<model>, got {spec!r}")
    if provider not in PROVIDERS:
        raise ValueError(f"unknown provider {provider!r}; choose from {sorted(PROVIDERS)}")
    return provider, model


def judge_problems(spec: str, env: Mapping[str, str] | None = None) -> list[str]:
    """Why this judge cannot run yet (bad spec, missing credentials); empty means ready."""
    env = os.environ if env is None else env
    try:
        provider, _model = parse_judge_spec(spec)
    except ValueError as exc:
        return [str(exc)]
    problem = credentials_problem(provider, env)
    return [problem] if problem else []


def make_judge(
    spec: str,
    *,
    env: Mapping[str, str] | None = None,
    http_client: httpx2.Client | None = None,
    max_tokens: int = DEFAULT_JUDGE_MAX_TOKENS,
    timeout_s: float = DEFAULT_JUDGE_TIMEOUT_S,
) -> JudgeFn:
    provider, model = parse_judge_spec(spec)
    env = os.environ if env is None else env
    adapter = make_adapter(provider, env=env, http_client=http_client)

    def judge(prompt: str) -> str:
        result = adapter.complete(model, prompt, max_tokens=max_tokens, timeout=timeout_s, seed=0)
        if not result.text:
            raise ValueError(f"judge {spec!r} returned no text")
        return result.text

    return judge
