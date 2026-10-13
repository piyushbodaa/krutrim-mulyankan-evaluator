"""Provider adapters: one call to a model API, normalised into a Completion.

Anthropic uses the official `anthropic` SDK. OpenAI, xAI, Gemini, OpenRouter and Ollama all speak
the OpenAI Chat Completions protocol, so they share the official `openai` SDK with a per-provider
base URL.

ModelRunner disables retries by default. Opt-in retries cover only HTTP 429/5xx and reserve
all attempts in preflight. Billing for a failed request can be uncertain; timeouts and dropped
connections are never retried.
"""

from __future__ import annotations

import os
import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

import anthropic
import httpx2
import openai

Protocol = Literal["anthropic", "openai"]


@dataclass(frozen=True)
class ProviderConfig:
    protocol: Protocol
    key_env: str | None
    base_url: str | None = None
    base_url_env: str | None = None
    max_tokens_param: str = "max_tokens"
    supports_seed: bool = False
    local: bool = False

    def resolve_base_url(self, env: Mapping[str, str]) -> str | None:
        return env.get(self.base_url_env, self.base_url) if self.base_url_env else self.base_url


PROVIDERS: dict[str, ProviderConfig] = {
    "anthropic": ProviderConfig("anthropic", "ANTHROPIC_API_KEY"),
    "openai": ProviderConfig(
        "openai",
        "OPENAI_API_KEY",
        "https://api.openai.com/v1",
        max_tokens_param="max_completion_tokens",
        supports_seed=True,
    ),
    "xai": ProviderConfig("openai", "XAI_API_KEY", "https://api.x.ai/v1"),
    "gemini": ProviderConfig(
        "openai", "GEMINI_API_KEY", "https://generativelanguage.googleapis.com/v1beta/openai/"
    ),
    "openrouter": ProviderConfig("openai", "OPENROUTER_API_KEY", "https://openrouter.ai/api/v1"),
    "ollama": ProviderConfig(
        "openai",
        None,
        "http://localhost:11434/v1",
        base_url_env="OLLAMA_BASE_URL",
        supports_seed=True,
        local=True,
    ),
    "vllm": ProviderConfig(
        "openai",
        None,
        "http://localhost:8000/v1",
        base_url_env="VLLM_BASE_URL",
        local=True,
        supports_seed=True,
    ),
    "compatible": ProviderConfig(
        "openai", "COMPATIBLE_API_KEY", base_url_env="COMPATIBLE_BASE_URL"
    ),
}


@dataclass
class Completion:
    text: str
    input_tokens: int | None  # absent provider usage stays unknown
    output_tokens: int | None
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    stop_reason: str | None = None
    request: dict[str, Any] = field(default_factory=dict)
    response: dict[str, Any] = field(default_factory=dict)
    resolved_model: str | None = None


def credentials_problem(provider: str, env: Mapping[str, str]) -> str | None:
    cfg = PROVIDERS[provider]
    if provider == "compatible" and not cfg.resolve_base_url(env):
        return "COMPATIBLE_BASE_URL is not set"
    if cfg.local or cfg.key_env is None:
        return None
    if env.get(cfg.key_env):
        return None
    if provider == "anthropic":
        # The SDK also accepts an auth token or an `ant auth login` profile.
        if env.get("ANTHROPIC_AUTH_TOKEN") or env.get("ANTHROPIC_PROFILE"):
            return None
        config_home = Path(env.get("XDG_CONFIG_HOME") or Path.home() / ".config")
        if (config_home / "anthropic").is_dir():
            return None
        return "no Anthropic credentials (set ANTHROPIC_API_KEY or run `ant auth login`)"
    return f"{cfg.key_env} is not set"


def is_retryable(exc: BaseException) -> bool:
    if isinstance(exc, anthropic.APIStatusError | openai.APIStatusError):
        return exc.status_code == 429 or exc.status_code >= 500
    return False


def retry_delay(exc: BaseException, attempt: int, cap: float = 30.0) -> float:
    response = getattr(exc, "response", None)
    header = response.headers.get("retry-after") if response is not None else None
    try:
        if header is not None:
            return min(cap, max(0.0, float(header)))
    except ValueError:
        pass
    return min(cap, 2.0**attempt)


def call_with_retries[T](
    fn: Callable[[], T], *, max_retries: int = 2, sleep: Callable[[float], None] = time.sleep
) -> tuple[T, int]:
    """Run fn with explicitly bounded 429/5xx retries. Returns (result, retries used)."""
    for attempt in range(max_retries + 1):
        try:
            return fn(), attempt
        except Exception as exc:
            if attempt == max_retries or not is_retryable(exc):
                raise
            sleep(retry_delay(exc, attempt))
    raise AssertionError("unreachable")


class AnthropicAdapter:
    def __init__(self, api_key: str | None, http_client: httpx2.Client | None = None) -> None:
        # api_key=None lets the SDK resolve credentials itself (env var, auth token, profile).
        # SDK retries are off: call_with_retries decides what is safe to repeat.
        self.client = anthropic.Anthropic(api_key=api_key, max_retries=0, http_client=http_client)

    def complete(
        self,
        model: str,
        prompt: str,
        *,
        max_tokens: int,
        timeout: float,
        seed: int,
        effort: str | None = None,
        temperature: float | None = None,
    ) -> Completion:
        if temperature is not None:
            raise ValueError("current Claude models do not accept temperature; drop the option")
        request: dict[str, Any] = {
            "model": model,
            "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": prompt}],
        }
        if effort:
            request["output_config"] = {"effort": effort}
        message = self.client.messages.create(**request, timeout=timeout)
        usage = message.usage
        return Completion(
            text="".join(block.text for block in message.content if block.type == "text"),
            input_tokens=usage.input_tokens,
            output_tokens=usage.output_tokens,
            cache_read_tokens=usage.cache_read_input_tokens or 0,
            cache_write_tokens=usage.cache_creation_input_tokens or 0,
            stop_reason=message.stop_reason,
            request=request,
            response=message.to_dict(),
            resolved_model=message.model,
        )


class OpenAICompatibleAdapter:
    def __init__(
        self,
        cfg: ProviderConfig,
        api_key: str | None,
        env: Mapping[str, str],
        http_client: httpx2.Client | None = None,
    ) -> None:
        self.cfg = cfg
        self.client = openai.OpenAI(
            api_key=api_key or ("ollama" if cfg.local else None),
            base_url=cfg.resolve_base_url(env),
            max_retries=0,
            http_client=http_client,
        )

    def complete(
        self,
        model: str,
        prompt: str,
        *,
        max_tokens: int,
        timeout: float,
        seed: int,
        effort: str | None = None,
        temperature: float | None = None,
    ) -> Completion:
        request: dict[str, Any] = {
            "model": model,
            self.cfg.max_tokens_param: max_tokens,
            "messages": [{"role": "user", "content": prompt}],
        }
        if self.cfg.supports_seed:
            request["seed"] = seed
        if effort:
            request["reasoning_effort"] = effort
        if temperature is not None:
            request["temperature"] = temperature
        response = self.client.chat.completions.create(**request, timeout=timeout)
        choice = response.choices[0] if response.choices else None
        usage = response.usage
        details = usage.prompt_tokens_details if usage else None
        cached = (details.cached_tokens or 0) if details else 0
        return Completion(
            text=(choice.message.content or "") if choice else "",
            input_tokens=(usage.prompt_tokens - cached) if usage else None,
            output_tokens=usage.completion_tokens if usage else None,
            cache_read_tokens=cached,
            stop_reason=choice.finish_reason if choice else None,
            request=request,
            response=response.model_dump(mode="json"),
            resolved_model=response.model,
        )


def make_adapter(
    provider: str,
    *,
    api_key: str | None = None,
    env: Mapping[str, str] | None = None,
    http_client: httpx2.Client | None = None,
) -> AnthropicAdapter | OpenAICompatibleAdapter:
    env = os.environ if env is None else env
    cfg = PROVIDERS[provider]
    if cfg.protocol == "anthropic":
        return AnthropicAdapter(api_key, http_client)
    key = api_key or (env.get(cfg.key_env) if cfg.key_env else None)
    return OpenAICompatibleAdapter(cfg, key, env, http_client)
