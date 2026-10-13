"""ModelRunner: calls a model directly through its provider API."""

from __future__ import annotations

import os
import time
from collections.abc import Callable, Mapping
from typing import Literal
from urllib.parse import urlsplit

import httpx2
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from kritrim_mulyankan.pricing import Price, PriceTable, bundled_prices, max_input_tokens
from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage
from kritrim_mulyankan.runners.providers import (
    PROVIDERS,
    call_with_retries,
    credentials_problem,
    make_adapter,
)
from kritrim_mulyankan.suite import Task

Effort = Literal["minimal", "low", "medium", "high", "xhigh", "max"]


class ModelOptions(BaseModel):
    """Per-runner options after the model id, e.g. `model:anthropic/claude-opus-5-5,effort=high`."""

    model_config = ConfigDict(extra="forbid", frozen=True)

    max_tokens: int | None = Field(None, ge=1, description="Overrides task max_output_tokens.")
    effort: Effort | None = Field(None, description="Anthropic effort / OpenAI reasoning_effort.")
    temperature: float | None = Field(None, ge=0, le=2, description="OpenAI-compatible only.")
    retries: int = Field(
        0, ge=0, le=2, description="Opt-in retries; uncertain billing reserves budget."
    )

    def suffix(self) -> str:
        set_fields = self.model_dump(exclude_none=True, exclude_defaults=True)
        return "".join(f",{k}={v}" for k, v in sorted(set_fields.items()))


class ModelRunner(Runner):
    """Spec: `model:<provider>/<model>[,key=value...]`, e.g. `model:anthropic/claude-opus-5-5`,
    `model:openai/gpt-6,effort=low`, `model:ollama/llama3.2:3b`.

    Providers: anthropic, openai, xai, gemini, openrouter, ollama. Keys come from the environment
    (or .env). A model with no price is refused, and every attempt is checked against an upper-bound
    cost (prompt bytes in, max_tokens out) before it is sent.
    """

    kind = "model"

    def __init__(
        self,
        provider: str,
        model: str,
        options: ModelOptions | None = None,
        *,
        prices: PriceTable | None = None,
        api_key: str | None = None,
        env: Mapping[str, str] | None = None,
        http_client: httpx2.Client | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        if provider not in PROVIDERS:
            raise ValueError(f"unknown provider {provider!r}; choose from {sorted(PROVIDERS)}")
        self.options = options or ModelOptions()
        super().__init__(f"model:{provider}/{model}{self.options.suffix()}")
        self.provider = provider
        self.model = model
        self.price: Price | None = (prices or bundled_prices()).lookup(provider, model)
        self._api_key = api_key
        self._env = os.environ if env is None else env
        self._http_client = http_client
        self._sleep = sleep
        self._adapter = None

    @classmethod
    def from_spec(cls, spec: str, *, prices: PriceTable | None = None) -> ModelRunner:
        target, *option_parts = spec.split(",")
        provider, sep, model = target.partition("/")
        if not (provider and sep and model):
            raise ValueError(f"model runner spec must be <provider>/<model>, got {spec!r}")
        raw: dict[str, str] = {}
        for part in option_parts:
            key, eq, value = part.partition("=")
            if not (key and eq and value):
                raise ValueError(f"runner option must be key=value, got {part!r} in {spec!r}")
            raw[key.strip()] = value.strip()
        try:
            options = ModelOptions.model_validate(raw)
        except ValidationError as exc:
            details = "; ".join(f"{'.'.join(map(str, e['loc']))}: {e['msg']}" for e in exc.errors())
            raise ValueError(f"bad options in {spec!r}: {details}") from None
        return cls(provider, model, options, prices=prices)

    def max_tokens(self, task: Task) -> int:
        return self.options.max_tokens or task.max_output_tokens

    def problems(self) -> list[str]:
        found = []
        if self.price is None:
            found.append(
                f"no price for {self.provider}/{self.model}; add it to prices.yaml "
                "(see `km list prices`)"
            )
        if self._api_key is None and (problem := credentials_problem(self.provider, self._env)):
            found.append(problem)
        if self.provider == "anthropic" and self.options.temperature is not None:
            found.append("Anthropic adapter does not support temperature")
        endpoint = PROVIDERS[self.provider].resolve_base_url(self._env)
        if endpoint:
            url = urlsplit(endpoint)
            if url.scheme not in {"http", "https"} or not url.hostname:
                found.append("endpoint must be an HTTP(S) URL")
            if url.username or url.password or url.query or url.fragment:
                found.append("endpoint must not contain credentials, query parameters or fragments")
        elif self.provider == "compatible":
            found.append("COMPATIBLE_BASE_URL is not set")
        return found

    def estimate_cost_usd(self, task: Task, prompt: str) -> float | None:
        if self.price is None:
            return None
        return self.price.cost(max_input_tokens(prompt), self.max_tokens(task)) * (
            1 + self.options.retries
        )

    def configuration(self) -> dict:
        cfg = PROVIDERS[self.provider]
        mock = self._http_client is not None and isinstance(
            getattr(self._http_client, "_transport", None), httpx2.MockTransport
        )
        return {
            **super().configuration(),
            "evaluation_mode": "mock" if mock else "live",
            "provider": self.provider,
            "requested_model": self.model,
            "options": self.options.model_dump(),
            "endpoint": cfg.resolve_base_url(self._env),
            "seed_supported": cfg.supports_seed,
            "price": self.price.model_dump(mode="json") if self.price else None,
            "pricing_currency": "USD",
            "cost_scope": "API charges; excludes hardware/subscription",
        }

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        if self.price is None:
            raise RuntimeError(f"refusing to call {self.name}: no price, so no budget check")
        if self._adapter is None:
            self._adapter = make_adapter(
                self.provider, api_key=self._api_key, env=self._env, http_client=self._http_client
            )
        adapter = self._adapter

        def call():
            return adapter.complete(
                self.model,
                prompt,
                max_tokens=self.max_tokens(task),
                timeout=ctx.timeout_s,
                seed=ctx.seed,
                effort=self.options.effort,
                temperature=self.options.temperature,
            )

        result, retries = call_with_retries(
            call, max_retries=self.options.retries, sleep=self._sleep
        )
        known_usage = result.input_tokens is not None and result.output_tokens is not None
        cost = (
            self.price.cost(
                result.input_tokens,
                result.output_tokens,
                cache_read_tokens=result.cache_read_tokens,
                cache_write_tokens=result.cache_write_tokens,
            )
            if known_usage
            else (0.0 if PROVIDERS[self.provider].local else None)
        )
        if retries:
            cost = None  # Earlier 5xx requests may have been billed; never claim free retries.
        return RunOutput(
            output=result.text,
            usage=Usage(
                input_tokens=(
                    result.input_tokens + result.cache_read_tokens + result.cache_write_tokens
                    if result.input_tokens is not None
                    else None
                ),
                output_tokens=result.output_tokens,
                cost_usd=cost,
            ),
            steps=1,
            tool_calls=0,
            error="output truncated by token limit"
            if result.stop_reason in {"length", "max_tokens"}
            else "provider returned no answer"
            if not result.text
            else None,
            raw={
                "provider": self.provider,
                "resolved_model": result.resolved_model,
                "request": result.request,
                "stop_reason": result.stop_reason,
                "retries": retries,
                "usage": {
                    "input_tokens": result.input_tokens,
                    "output_tokens": result.output_tokens,
                    "cache_read_tokens": result.cache_read_tokens,
                    "cache_write_tokens": result.cache_write_tokens,
                },
                "price_per_mtok": self.price.model_dump(exclude_none=True),
                "response": result.response,
            },
        )
