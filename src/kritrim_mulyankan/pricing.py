"""Model price table and the cost maths behind estimates, budget caps and recorded spend."""

from __future__ import annotations

from importlib.resources import files
from pathlib import Path
from typing import Any

import yaml
from pydantic import BaseModel, ConfigDict, Field

MTOK = 1_000_000

# Chat formatting and role tokens on top of the prompt text. Generous on purpose.
MESSAGE_OVERHEAD_TOKENS = 64


class Price(BaseModel):
    """USD per million tokens."""

    model_config = ConfigDict(extra="forbid", frozen=True, allow_inf_nan=False)

    input: float = Field(ge=0)
    output: float = Field(ge=0)
    cache_read: float | None = Field(None, ge=0, description="Default: the input price.")
    cache_write: float | None = Field(None, ge=0, description="Default: 1.25 x the input price.")
    long_context_threshold: int | None = Field(None, gt=0)
    long_input: float | None = Field(None, ge=0)
    long_output: float | None = Field(None, ge=0)
    source: str | None = None
    as_of: str | None = None

    def cost(
        self,
        input_tokens: int,
        output_tokens: int,
        *,
        cache_read_tokens: int = 0,
        cache_write_tokens: int = 0,
    ) -> float:
        prompt_tokens = input_tokens + cache_read_tokens + cache_write_tokens
        threshold = self.long_context_threshold
        long = threshold is not None and prompt_tokens > threshold
        inp = self.long_input if long and self.long_input is not None else self.input
        out = self.long_output if long and self.long_output is not None else self.output
        read = self.cache_read if self.cache_read is not None else inp
        write = self.cache_write if self.cache_write is not None else inp * 1.25
        return (
            input_tokens * inp
            + output_tokens * out
            + cache_read_tokens * read
            + cache_write_tokens * write
        ) / MTOK


class PriceTable:
    """Prices keyed by `provider/model`; a `provider/*` entry covers the whole provider."""

    def __init__(self, prices: dict[str, Price] | None = None) -> None:
        self.prices = dict(prices or {})

    def lookup(self, provider: str, model: str) -> Price | None:
        return self.prices.get(f"{provider}/{model}") or self.prices.get(f"{provider}/*")

    def merged(self, other: PriceTable) -> PriceTable:
        return PriceTable({**self.prices, **other.prices})

    def __len__(self) -> int:
        return len(self.prices)

    @classmethod
    def parse(cls, data: Any, origin: str = "prices") -> PriceTable:
        if data is None:
            return cls()
        if not isinstance(data, dict):
            raise ValueError(f"{origin}: expected a mapping of providers")
        prices: dict[str, Price] = {}
        for provider, block in data.items():
            if not isinstance(block, dict) or not isinstance(block.get("models"), dict):
                raise ValueError(f"{origin}: provider {provider!r} needs a `models` mapping")
            shared = {k: str(block[k]) for k in ("source", "as_of") if block.get(k) is not None}
            for model, entry in block["models"].items():
                if not isinstance(entry, dict):
                    raise ValueError(f"{origin}: {provider}/{model} must be a mapping")
                prices[f"{provider}/{model}"] = Price.model_validate({**shared, **entry})
        return cls(prices)


def bundled_prices() -> PriceTable:
    text = files("kritrim_mulyankan").joinpath("prices.yaml").read_text(encoding="utf-8")
    return PriceTable.parse(yaml.safe_load(text), "bundled prices.yaml")


def openrouter_prices(models: Any) -> PriceTable:
    """Prices from OpenRouter's public model list (`GET /api/v1/models`).

    `models` is the parsed JSON body (or its `data` list). Per-token prices
    become USD per million tokens, keyed `openrouter/<model id>`.
    """
    rows = models.get("data") if isinstance(models, dict) else models
    prices: dict[str, Price] = {}
    if not isinstance(rows, list):
        return PriceTable(prices)
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("id"), str):
            continue
        pricing = row.get("pricing") or {}
        try:
            prompt = float(pricing.get("prompt"))
            completion = float(pricing.get("completion"))
        except (TypeError, ValueError):
            continue
        if prompt < 0 or completion < 0:
            continue
        entry: dict[str, Any] = {
            "input": prompt * MTOK,
            "output": completion * MTOK,
            "source": "https://openrouter.ai/api/v1/models (live)",
        }
        try:
            cache_read = pricing.get("input_cache_read")
            if cache_read is not None:
                entry["cache_read"] = float(cache_read) * MTOK
        except (TypeError, ValueError):
            pass
        for override in pricing.get("overrides") or []:
            try:
                entry["long_context_threshold"] = int(override["min_prompt_tokens"])
                entry["long_input"] = float(override["prompt"]) * MTOK
                entry["long_output"] = float(override["completion"]) * MTOK
            except (KeyError, TypeError, ValueError):
                continue
            break
        prices[f"openrouter/{row['id']}"] = Price.model_validate(entry)
    return PriceTable(prices)


def load_prices(override: Path | None = None) -> PriceTable:
    """Bundled prices, with entries from the override file (if it exists) taking precedence."""
    table = bundled_prices()
    if override is not None and override.is_file():
        data = yaml.safe_load(override.read_text(encoding="utf-8"))
        table = table.merged(PriceTable.parse(data, str(override)))
    return table


def max_input_tokens(prompt: str) -> int:
    """Upper bound on prompt tokens: byte-level tokenizers emit at most one token per byte."""
    return len(prompt.encode("utf-8")) + MESSAGE_OVERHEAD_TOKENS
