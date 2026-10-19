"""Desktop engine bridge. The Tauri shell speaks JSON lines; this process runs the evaluator.

API keys arrive in the process environment (the shell reads them from the OS keychain) or via
``set_keys``, which keeps them in memory. This module never writes a ``.env`` or any other
secret file. Paid runs start only after the shell sends ``confirmed: true``.
"""

from __future__ import annotations

import json
import math
import os
import re
import secrets
import sys
import threading
from collections.abc import Callable, Mapping, MutableMapping
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, quote, urlsplit

from kritrim_mulyankan import __version__
from kritrim_mulyankan.engine import PreflightError, preflight, run_suite
from kritrim_mulyankan.export import result_export, write_export
from kritrim_mulyankan.history import load_history
from kritrim_mulyankan.judge import judge_problems, make_judge, parse_judge_spec
from kritrim_mulyankan.local_ui import TITLES
from kritrim_mulyankan.microeval import write_micro_eval
from kritrim_mulyankan.miner import (
    VerdictCache,
    estimate_judge_cost_usd,
    mine_failures,
    select_pairs,
)
from kritrim_mulyankan.pricing import PriceTable, load_prices, max_input_tokens, openrouter_prices
from kritrim_mulyankan.provenance import redact
from kritrim_mulyankan.runners import make_runner
from kritrim_mulyankan.runners.model import ModelRunner
from kritrim_mulyankan.settings import Settings
from kritrim_mulyankan.store import Store
from kritrim_mulyankan.suite import Suite, discover_suites, load_suite

HONESTY = {
    "pilot": "Unvalidated pilot",
    "ranking": "Not a ranking",
    "close": "Too close to call",
    "close_means": ("Too close to call means the gap could be chance, not that they are equal."),
}
KEY_ENVS = {
    "openai": "OPENAI_API_KEY",
    "anthropic": "ANTHROPIC_API_KEY",
    "gemini": "GEMINI_API_KEY",
    "xai": "XAI_API_KEY",
    "openrouter": "OPENROUTER_API_KEY",
    "compatible": "COMPATIBLE_API_KEY",
}
URL_ENVS = ("OLLAMA_BASE_URL", "VLLM_BASE_URL", "COMPATIBLE_BASE_URL")
PROVIDERS = [
    {
        "id": "openrouter",
        "name": "OpenRouter",
        "blurb": (
            "One key for AIs from OpenAI, Anthropic, Google, Meta and more. "
            "You are billed per token."
        ),
        "blurb_hi": (
            "OpenAI, Anthropic, Google, Meta और अन्य के एआई के लिए एक कुंजी। शुल्क प्रति टोकन लगता है।"
        ),
        "signup": "https://openrouter.ai/keys",
        "recommended": True,
    },
    {
        "id": "openai",
        "name": "OpenAI",
        "blurb": "For GPT models. You are billed per token.",
        "blurb_hi": "GPT मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
        "signup": "https://platform.openai.com/api-keys",
    },
    {
        "id": "anthropic",
        "name": "Anthropic",
        "blurb": "For Claude models. You are billed per token.",
        "blurb_hi": "Claude मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
        "signup": "https://console.anthropic.com/settings/keys",
    },
    {
        "id": "gemini",
        "name": "Google",
        "blurb": "For Gemini models. You are billed per token.",
        "blurb_hi": "Gemini मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
        "signup": "https://aistudio.google.com/apikey",
    },
    {
        "id": "xai",
        "name": "xAI",
        "blurb": "For Grok models. You are billed per token.",
        "blurb_hi": "Grok मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
        "signup": "https://console.x.ai/",
    },
    {
        "id": "compatible",
        "name": "Other compatible API",
        "blurb": "A service that speaks the OpenAI API. You also set its address.",
        "blurb_hi": "कोई सेवा जो OpenAI एपीआई बोलती हो। उसका पता भी देना होगा।",
        "signup": "",
    },
]
FREE_MODELS = [
    {
        "spec": "oracle",
        "name": "Correct-answer check",
        "name_hi": "सही उत्तर वाली जाँच",
        "provider": "practice",
        "blurb": "Returns the expected answer so you can see the scorer work. Not an AI model.",
        "blurb_hi": "अपेक्षित उत्तर देता है, ताकि स्कोर जाँचा जा सके। यह एआई मॉडल नहीं है।",
        "free": True,
        "suggested": True,
    },
    {
        "spec": "echo",
        "name": "Echo check",
        "name_hi": "दोहराने वाली जाँच",
        "provider": "practice",
        "blurb": "Repeats the question and should fail. That shows the scorer is awake.",
        "blurb_hi": "प्रश्न को दोहराता है और फेल होना चाहिए। इससे पता चलता है कि स्कोर जाग रहा है।",
        "free": True,
        "suggested": True,
    },
]
SUGGESTED = {
    "anthropic/claude-haiku-4-5",
    "anthropic/claude-sonnet-4-6",
    "anthropic/claude-opus-4-6",
    "openai/gpt-5",
    "openai/gpt-5-mini",
    "openai/gpt-4o-mini",
    "gemini/gemini-2.5-pro",
    "gemini/gemini-2.5-flash",
    "xai/grok-4.20",
    "openrouter/anthropic/claude-sonnet-4.6",
    "openrouter/anthropic/claude-haiku-4.5",
    "openrouter/openai/gpt-5",
    "openrouter/openai/gpt-5-mini",
    "openrouter/google/gemini-2.5-flash",
    "openrouter/google/gemini-2.5-pro",
    "openrouter/deepseek/deepseek-chat",
    "openrouter/meta-llama/llama-4-maverick",
    "openrouter/x-ai/grok-4.20",
}
MODEL_BLURB = {
    "claude-haiku-4-5": "A smaller Claude model. Lower price per token.",
    "claude-haiku-5-5": "A smaller Claude model. Lower price per token.",
    "claude-sonnet-4-6": "A mid-size Claude model.",
    "claude-sonnet-5": "A mid-size Claude model.",
    "claude-sonnet-5-5": "A mid-size Claude model.",
    "claude-opus-4-6": "A larger Claude model. Higher price per token.",
    "claude-opus-4-7": "A larger Claude model. Higher price per token.",
    "claude-opus-4-8": "A larger Claude model. Higher price per token.",
    "claude-opus-5": "A larger Claude model. Higher price per token.",
    "claude-opus-5-5": "A larger Claude model. Higher price per token.",
}
PACK_ORDER = ["india-pilot", "railway-pilot", "example"]
PACK_COPY = {
    "india-pilot": {
        "name": "India pilot",
        "name_hi": "भारत पायलट",
        "badge": "Pilot set",
        "summary": (
            "Start here. Short synthetic tasks about rupees, records and a missing date. "
            "Safe to share. Unvalidated pilot. Not a ranking."
        ),
        "summary_hi": (
            "यहीं से शुरू करें। रुपये, रिकॉर्ड और एक गायब तारीख पर छोटे कृत्रिम प्रश्न। "
            "बाँटने योग्य। जाँच प्रमाणित नहीं। यह रैंकिंग नहीं है।"
        ),
    },
    "railway-pilot": {
        "name": "Railway smoke test",
        "name_hi": "रेलवे जाँच",
        "badge": "Extra fixture",
        "summary": "One extra synthetic task. Not a ranking.",
        "summary_hi": "एक अतिरिक्त कृत्रिम प्रश्न। यह रैंकिंग नहीं है।",
    },
    "example": {
        "name": "Quick example",
        "name_hi": "छोटा उदाहरण",
        "badge": "Pipeline check",
        "summary": "Five tiny tasks to see the pipeline finish. Not a ranking.",
        "summary_hi": "पाइपलाइन देखने के लिए पाँच छोटे प्रश्न। यह रैंकिंग नहीं है।",
    },
}
DEMO_TITLES = {
    "DEMO01": "Add two rupee amounts",
    "DEMO02": "Drop a repeated identifier",
    "DEMO03": "Say when a date is missing",
    "DEMO04": "Say when a date is missing",
}
PREVIEW_FILES = {
    "style.css": "text/css; charset=utf-8",
    "theme.js": "text/javascript; charset=utf-8",
    "app.js": "text/javascript; charset=utf-8",
    "showcase.js": "text/javascript; charset=utf-8",
    "compare.js": "text/javascript; charset=utf-8",
    "jobs.js": "text/javascript; charset=utf-8",
    "preview-fixtures.js": "text/javascript; charset=utf-8",
}
SHELL_FILES = {
    "index.html": "text/html; charset=utf-8",
    "app.js": "text/javascript; charset=utf-8",
    "app.css": "text/css; charset=utf-8",
    "theme.js": "text/javascript; charset=utf-8",
    "i18n.js": "text/javascript; charset=utf-8",
    "bridge.js": "text/javascript; charset=utf-8",
    "dashboard-shim.js": "text/javascript; charset=utf-8",
    "side.js": "text/javascript; charset=utf-8",
}
_RUN_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,80}$")
_SCRIPT_TAG = '<script src="preview-fixtures.js" defer></script>'
MAX_REPEATS = 5
MAX_RUNNERS = 6


def resource_root() -> Path:
    """Repo root in development, the PyInstaller bundle when the sidecar is frozen."""
    if getattr(sys, "frozen", False):
        return Path(sys._MEIPASS)
    override = os.environ.get("KM_RESOURCE_ROOT")
    if override:
        return Path(override)
    return Path(__file__).resolve().parents[2]


def ui_dir() -> Path:
    override = os.environ.get("KM_UI_ROOT")
    if override:
        return Path(override)
    return resource_root() / "desktop" / "ui"


def default_settings(env: Mapping[str, str] | None = None) -> Settings:
    env = os.environ if env is None else env
    root = resource_root()
    runs = Path(env.get("MJ_RUNS_DIR", root / "runs"))
    prices = env.get("MJ_PRICES")
    return Settings(
        runs_dir=runs,
        db_path=Path(env.get("MJ_DB", runs / "results.db")),
        suites_dir=Path(env.get("MJ_SUITES_DIR", root / "suites")),
        prices_path=Path(prices) if prices else root / "prices.yaml",
        max_usd=float(env.get("MJ_MAX_USD", "1.0")),
        usd_to_inr=float(env.get("MJ_USD_INR", "88.0")),
    )


def _plain_problem(text: str) -> str:
    if "is not set" in text or "no Anthropic credentials" in text:
        return (
            "This model needs an API key. Add it on the keys screen, or pick a practice check. "
            "Nothing was called."
        )
    if "no price" in text:
        return (
            "There is no checked price for this model, so a paid run is refused. "
            "Nothing was called."
        )
    if "exceeds" in text and "cap" in text:
        return (
            "The worst-case cost is above your spending cap. Raise the cap or run a smaller pack. "
            "Nothing was called."
        )
    return text


def _check_base_url(value: str) -> str:
    parts = urlsplit(value.strip())
    if parts.scheme not in {"http", "https"} or not parts.hostname:
        raise ValueError("The address must start with http:// or https://.")
    if parts.username or parts.password or parts.query or parts.fragment:
        raise ValueError("The address must not include a password, query, or fragment.")
    return value.strip()


class DesktopSession:
    """One local evaluator session. Safe to call from the UI thread and a cancel request."""

    def __init__(
        self,
        settings: Settings | None = None,
        env: MutableMapping[str, str] | None = None,
    ) -> None:
        self.settings = settings or default_settings()
        self.env: MutableMapping[str, str] = os.environ if env is None else env
        self.root = resource_root()
        self.preview_dir = self.root / "preview"
        self._cancel = threading.Event()
        self._busy = False
        self._lock = threading.Lock()
        self._token = secrets.token_urlsafe(24)
        self._httpd: ThreadingHTTPServer | None = None
        self._external_base: str | None = None
        self._names: dict[str, str] = {}
        self._live_prices: PriceTable | None = None
        self._live_prices_at: float = 0.0

    def close(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd = None

    def scrub(self, text: str | None) -> str | None:
        if not text:
            return text
        for value in self.env.values():
            if isinstance(value, str) and len(value) >= 8 and value in text:
                text = text.replace(value, "[REDACTED]")
        return redact(text)

    def ping(self) -> dict[str, Any]:
        return {
            "ok": True,
            "version": __version__,
            "honesty": HONESTY,
            "engine": "kritrim_mulyankan",
        }

    def set_keys(self, keys: Mapping[str, str]) -> dict[str, Any]:
        saved: list[str] = []
        for name, value in keys.items():
            if name not in KEY_ENVS.values():
                raise ValueError("Unknown key name.")
            if not isinstance(value, str) or len(value) > 4096:
                raise ValueError("A key must be a short string.")
            if any(char in value for char in "\r\n\x00"):
                raise ValueError("A key cannot contain a line break.")
            if value:
                self.env[name] = value
                saved.append(name)
            else:
                self.env.pop(name, None)
        return {"saved": saved}

    def set_urls(self, urls: Mapping[str, str]) -> dict[str, Any]:
        applied = []
        for name, value in urls.items():
            if name not in URL_ENVS:
                raise ValueError("Unknown address.")
            if value:
                self.env[name] = _check_base_url(value)
            else:
                self.env.pop(name, None)
            applied.append(name)
        return {"applied": applied}

    def key_status(self) -> dict[str, Any]:
        return {
            provider: {"env": env_name, "saved": bool(self.env.get(env_name))}
            for provider, env_name in KEY_ENVS.items()
        }

    def _prices(self):
        path = self.settings.prices_path
        return load_prices(path if path.is_file() else None)

    def _fetch_openrouter_prices(self) -> PriceTable:
        """OpenRouter's public price list, fetched at most every 10 minutes."""
        import time as _time
        import urllib.request

        if self._live_prices is not None and _time.time() - self._live_prices_at < 600:
            return self._live_prices
        base = self.env.get("KM_OPENROUTER_BASE") or "https://openrouter.ai/api/v1"
        request = urllib.request.Request(
            f"{base.rstrip('/')}/models",
            headers={"HTTP-Referer": "https://krutrimmulyankan.in", "X-Title": "Krutrim Mulyankan"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310 - fixed host
            table = openrouter_prices(json.loads(response.read()))
        self._live_prices = table
        self._live_prices_at = _time.time()
        return table

    def _prices_for(self, specs: list[str]):
        """Bundled prices, plus OpenRouter's live list when a requested
        `openrouter/<model>` spec has no bundled price yet. A model that
        stays unpriced is still refused before any call."""
        table = self._prices()
        missing = False
        for spec in specs:
            bare = spec.removeprefix("model:")
            provider, _, model = bare.partition("/")
            if provider == "openrouter" and model and table.lookup(provider, model) is None:
                missing = True
        if missing:
            try:
                live = self._fetch_openrouter_prices()
                table = PriceTable({**live.prices, **table.prices})
            except Exception:  # noqa: BLE001 - offline: bundled prices still apply
                pass
        return table

    def my_tests_dir(self) -> Path:
        override = self.env.get("MJ_MY_TESTS_DIR")
        return Path(override) if override else self.settings.runs_dir / "my-tests"

    def _is_mine(self, suite: Suite) -> bool:
        return suite.path is not None and suite.path.parent == self.my_tests_dir()

    def _judge_for(self, suite: Suite, judge: str | None = None) -> str | None:
        if judge:
            return judge
        if self.env.get("MJ_JUDGE"):
            return self.env["MJ_JUDGE"]
        for task in suite.tasks:
            if task.grader.type == "llm_judge" and task.grader.judge_model:
                return task.grader.judge_model
        return None

    def _judge_check(self, suite, runners_count, repeats, prices, judge=None):
        """(judge spec, worst-case grading cost, problems, warnings).

        A suite where only some tasks need a judge (the example pack) keeps
        the CLI's behaviour: without a usable judge those answers stay
        pending. A suite where every task needs one (a micro-eval) is
        refused instead of spending runner money on unmarkable answers.
        """
        from kritrim_mulyankan.judge import DEFAULT_JUDGE_MAX_TOKENS

        judged = [t for t in suite.tasks if t.grader.type == "llm_judge"]
        if not judged:
            return None, 0.0, [], []
        required = len(judged) == len(suite.tasks)
        pending = ["Some answers need an AI judge to mark them; without one they stay unmarked."]
        spec = self._judge_for(suite, judge)
        if not spec:
            if required:
                return (
                    None,
                    None,
                    [
                        "These questions are marked by an AI judge, and no judge is set. "
                        "Connect a key, then try again. Nothing was called."
                    ],
                    [],
                )
            return None, 0.0, [], pending
        problems = judge_problems(spec, self.env)
        if problems:
            return (spec, None, problems, []) if required else (None, 0.0, [], pending)
        provider, model = parse_judge_spec(spec)
        price = prices.lookup(provider, model)
        if price is None:
            return (spec, None, [], []) if required else (None, 0.0, [], pending)
        per_task = sum(
            price.cost(
                max_input_tokens(f"{t.prompt}\n{t.rubric or ''}\n{t.reference or ''}"),
                DEFAULT_JUDGE_MAX_TOKENS,
            )
            for t in judged
        )
        return spec, per_task * runners_count * repeats, [], []

    def _suites(self) -> list[Suite]:
        suites = [load_suite(path) for path in discover_suites(self.settings.suites_dir)]
        rank = {name: index for index, name in enumerate(PACK_ORDER)}
        suites = sorted(suites, key=lambda suite: (rank.get(suite.name, len(rank)), suite.name))
        # Tests built in the app (hand-written or mined micro-evals) live
        # apart; a broken file there must never hide the ready-made packs.
        builtin = {suite.name for suite in suites}
        mine: list[Suite] = []
        for path in discover_suites(self.my_tests_dir()):
            try:
                suite = load_suite(path)
            except Exception:  # noqa: BLE001 - skip, like the Rust engine
                continue
            if suite.name.startswith("my-") and suite.name not in builtin:
                mine.append(suite)
        return suites + mine

    def _load_named(self, name: str) -> Suite:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,80}", name):
            raise ValueError("Choose a test pack from the list.")
        for suite in self._suites():
            if suite.name == name:
                return suite
        raise ValueError(f"No test pack named {name}.")

    def _runner(self, spec: str, prices):
        if spec not in {"echo", "oracle"} and not spec.startswith("model:"):
            raise ValueError(
                "This app runs practice checks and model APIs. "
                "Other adapters stay on the command line."
            )
        runner = make_runner(spec, prices=prices)
        if isinstance(runner, ModelRunner):
            runner._env = self.env
        return runner

    def _clean_specs(self, specs: list[str]) -> list[str]:
        if not isinstance(specs, list) or not specs:
            raise ValueError("Pick at least one model or practice check.")
        if len(specs) > MAX_RUNNERS:
            raise ValueError(f"Pick at most {MAX_RUNNERS} models.")
        if len(set(specs)) != len(specs):
            raise ValueError("The same model was picked twice.")
        if not all(isinstance(spec, str) for spec in specs):
            raise ValueError("Pick models from the list.")
        return specs

    def catalog(self) -> dict[str, Any]:
        prices = self._prices()
        models: list[dict[str, Any]] = []
        for card in FREE_MODELS:
            models.append({**card, "runnable": True, "price_usd": {"input": 0, "output": 0}})
        for key, price in sorted(prices.prices.items()):
            provider, _, model = key.partition("/")
            if provider in {"ollama", "vllm"} or model == "*":
                continue
            blurb = MODEL_BLURB.get(model, "Priced from the checked price table.")
            models.append(
                {
                    "spec": f"model:{provider}/{model}",
                    "name": model.rsplit("/", 1)[-1],
                    "name_hi": model.rsplit("/", 1)[-1],
                    "model": model,
                    "provider": provider,
                    "blurb": blurb,
                    "blurb_hi": blurb,
                    "free": False,
                    "suggested": key in SUGGESTED,
                    "runnable": True,
                    "price_usd": {"input": price.input, "output": price.output},
                    "price_as_of": price.as_of,
                }
            )
        for provider, title in (
            ("openai", "OpenAI models"),
            ("gemini", "Google Gemini models"),
            ("xai", "xAI Grok models"),
            ("openrouter", "OpenRouter models"),
        ):
            if any(card["provider"] == provider and card["runnable"] for card in models):
                continue
            models.append(
                {
                    "spec": None,
                    "name": title,
                    "name_hi": title,
                    "provider": provider,
                    "blurb": (
                        "No checked price is on file, so a paid run is refused. "
                        "Add a price, then pick a model. Nothing is called until you do."
                    ),
                    "blurb_hi": (
                        "जाँची हुई कीमत दर्ज नहीं है, इसलिए भुगतान वाला रन रुक जाता है। "
                        "कीमत जोड़ने के बाद मॉडल चुनें।"
                    ),
                    "free": False,
                    "suggested": False,
                    "runnable": False,
                    "price_usd": None,
                }
            )
        packs = []
        for suite in self._suites():
            copy = PACK_COPY.get(suite.name, {})
            languages = sorted({task.language for task in suite.tasks})
            mine = self._is_mine(suite)
            judged = any(task.grader.type == "llm_judge" for task in suite.tasks)
            if mine:
                display = (
                    suite.description
                    if len(suite.description) <= 80
                    else suite.name.removeprefix("my-").replace("-", " ")
                )
                name = name_hi = display
                badge = "From my chats" if judged else "My test"
                summary = summary_hi = ""
            else:
                name = copy.get("name", suite.name)
                name_hi = copy.get("name_hi", suite.name)
                badge = copy.get("badge", suite.validation_status)
                summary = copy.get("summary", suite.description)
                summary_hi = copy.get("summary_hi", suite.description)
            packs.append(
                {
                    "id": suite.name,
                    "name": name,
                    "name_hi": name_hi,
                    "badge": badge,
                    "summary": summary,
                    "summary_hi": summary_hi,
                    "recommended": suite.name == "india-pilot",
                    "mine": mine,
                    "judged": judged,
                    "tasks": len(suite.tasks),
                    "languages": languages,
                    "validation_status": suite.validation_status,
                }
            )
        self._names = {card["spec"]: card["name"] for card in models if card["spec"]}
        rate = self.settings.usd_to_inr
        return {
            "version": __version__,
            "honesty": HONESTY,
            "fx": {
                "usd_to_inr": rate,
                "note": "Rupee amounts use the rate stored in the app, not a live bank rate.",
            },
            "providers": [{**provider, "env": KEY_ENVS[provider["id"]]} for provider in PROVIDERS],
            "keys": self.key_status(),
            "models": models,
            "packs": packs,
            "default_pack": "india-pilot",
            "default_models": ["oracle", "echo"],
            "default_cap_usd": self.settings.max_usd,
        }

    def _fx(self, usd_to_inr: float | None) -> Settings:
        if usd_to_inr is None:
            return self.settings
        if not math.isfinite(usd_to_inr) or usd_to_inr <= 0:
            raise ValueError("The rupee rate must be a positive number.")
        return self.settings.model_copy(update={"usd_to_inr": float(usd_to_inr)})

    def estimate(
        self,
        *,
        suite: str,
        runners: list[str],
        repeats: int = 1,
        max_usd: float = 1.0,
        usd_to_inr: float | None = None,
    ) -> dict[str, Any]:
        settings = self._fx(usd_to_inr)
        loaded = self._load_named(suite)
        specs = self._clean_specs(runners)
        if not isinstance(repeats, int) or not 1 <= repeats <= MAX_REPEATS:
            raise ValueError(f"Tries must be between 1 and {MAX_REPEATS}.")
        if not math.isfinite(max_usd) or max_usd < 0:
            raise ValueError("The spending cap must be zero or more.")
        priced_specs = list(specs)
        judge_hint = self._judge_for(loaded)
        if judge_hint:
            priced_specs.append(judge_hint)
        prices = self._prices_for(priced_specs)
        built = [self._runner(spec, prices) for spec in specs]
        check = preflight(loaded.tasks, built, float(max_usd), repeats=repeats)
        judge_spec, judge_cost, judge_problems, judge_warnings = self._judge_check(
            loaded, len(built), repeats, prices
        )
        all_problems = list(check.problems) + judge_problems
        if judge_spec and judge_cost is None and not judge_problems:
            all_problems.append(
                f"no price for judge {judge_spec}; grading cost is unknown, so the run is refused"
            )
        total_worst = check.worst_case_usd + (judge_cost or 0.0)
        if judge_cost is not None and check.ok and total_worst > float(max_usd):
            all_problems.append("worst-case cost including the marking judge exceeds the cap")
        ok = check.ok and len(all_problems) == len(check.problems) and judge_cost is not None
        rate = settings.usd_to_inr
        rows = []
        for runner in built:
            mine = [item for item in check.estimates if item.runner == runner.name]
            unknown = any(item.cost_usd is None for item in mine)
            known = repeats * sum(item.cost_usd or 0 for item in mine)
            problems = runner.problems()
            rows.append(
                {
                    "spec": runner.name,
                    "label": self._names.get(runner.name, runner.name),
                    "attempts": len(mine) * repeats,
                    "worst_case_usd": None if unknown else known,
                    "worst_case_inr": None if unknown else known * rate,
                    "ready": not problems and not unknown,
                    "problems": problems,
                    "plain_problems": [_plain_problem(item) for item in problems],
                }
            )
        return {
            "ok": ok,
            "problems": all_problems,
            "plain_problems": [_plain_problem(item) for item in all_problems],
            "warnings": list(check.warnings) + judge_warnings,
            "worst_case_usd": total_worst,
            "worst_case_inr": total_worst * rate,
            "judge": (
                {
                    "spec": judge_spec,
                    "worst_case_usd": judge_cost,
                    "worst_case_inr": None if judge_cost is None else judge_cost * rate,
                }
                if judge_spec
                else None
            ),
            "cap_usd": float(max_usd),
            "cap_inr": float(max_usd) * rate,
            "usd_to_inr": rate,
            "fx_note": "Configured rate, not a live bank rate.",
            "attempts": len(loaded.tasks) * len(built) * repeats,
            "repeats": repeats,
            "paid": total_worst > 0,
            "runners": rows,
            "honesty": HONESTY,
            "pack": loaded.name,
        }

    def run(
        self,
        *,
        suite: str,
        runners: list[str],
        repeats: int = 1,
        max_usd: float = 1.0,
        confirmed: bool = False,
        seed: int = 0,
        usd_to_inr: float | None = None,
        on_event: Callable[[dict[str, Any]], None] | None = None,
    ) -> dict[str, Any]:
        if confirmed is not True:
            raise ValueError(
                "Confirm the cost on the previous screen before running. Nothing was called."
            )
        estimate = self.estimate(
            suite=suite,
            runners=runners,
            repeats=repeats,
            max_usd=max_usd,
            usd_to_inr=usd_to_inr,
        )
        if not estimate["ok"]:
            raise ValueError(
                " ".join(estimate["plain_problems"]) or "The run was refused. Nothing was called."
            )
        settings = self._fx(usd_to_inr)
        loaded = self._load_named(suite)
        judge_info = estimate.get("judge") or {}
        judge_spec = judge_info.get("spec")
        judge_usd = judge_info.get("worst_case_usd") or 0.0
        priced_specs = list(runners) + ([judge_spec] if judge_spec else [])
        prices = self._prices_for(priced_specs)
        built = [self._runner(spec, prices) for spec in runners]
        judge_fn = make_judge(judge_spec, env=self.env) if judge_spec else None
        titles = {**TITLES, **DEMO_TITLES}
        total = estimate["attempts"]
        done = {"n": 0}
        if not self._names:
            self.catalog()
        with self._lock:
            if self._busy:
                raise ValueError("A check is already running.")
            self._busy = True
            self._cancel = threading.Event()
            cancel_event = self._cancel

        def emit(payload: dict[str, Any]) -> None:
            if on_event:
                on_event(payload)

        def on_start(run_id: str) -> None:
            emit({"type": "start", "run_id": run_id, "total": total})

        def on_attempt(task_id: str, runner: str) -> None:
            emit(
                {
                    "type": "asking",
                    "task_id": task_id,
                    "title": titles.get(task_id, task_id),
                    "runner": runner,
                    "runner_label": self._names.get(runner, runner),
                    "index": done["n"],
                    "total": total,
                }
            )

        def on_result(record) -> None:
            done["n"] += 1
            emit(
                {
                    "type": "attempt",
                    "index": done["n"],
                    "total": total,
                    "task_id": record.task_id,
                    "title": titles.get(record.task_id, record.task_id),
                    "runner": record.runner,
                    "runner_label": self._names.get(record.runner, record.runner),
                    "passed": record.passed,
                    "status": record.status,
                    "grade_status": record.grade_status,
                    "latency_s": record.latency_s,
                    "error": self.scrub(record.error),
                }
            )

        try:
            with Store(settings.db_path) as store:
                try:
                    summary = run_suite(
                        loaded,
                        built,
                        store=store,
                        settings=settings,
                        seed=int(seed),
                        repeats=repeats,
                        max_usd=max(0.0, float(max_usd) - float(judge_usd)),
                        judge=judge_fn,
                        on_result=on_result,
                        on_start=on_start,
                        on_attempt=on_attempt,
                        cancel_event=cancel_event,
                    )
                except PreflightError as exc:
                    raise ValueError(self.scrub(str(exc)) or "The run was refused.") from exc
                destination = Path(summary.run_dir) / "results.json"
                write_export(store, summary.run_id, destination)
            card = self.share_card(summary.run_id)
            emit(
                {
                    "type": "done",
                    "run_id": summary.run_id,
                    "status": summary.status,
                    "total": total,
                }
            )
            return {
                "run_id": summary.run_id,
                "status": summary.status,
                "spent_usd": summary.spent_usd,
                "spent_inr": summary.spent_usd * settings.usd_to_inr,
                "export_path": str(destination),
                "card_path": card["path"],
                "dashboard_url": self.dashboard_url(summary.run_id),
                "honesty": HONESTY,
            }
        finally:
            with self._lock:
                self._busy = False

    def _history_paths(self, raw: Any) -> list[Path]:
        paths: list[Path] = []
        for item in raw if isinstance(raw, list) else []:
            if not isinstance(item, str) or not item.strip():
                continue
            text = item.strip()
            if text.startswith("~/"):
                text = str(Path(self.env.get("HOME", "")) / text[2:])
            paths.append(Path(text))
        if not paths:
            raise ValueError("Choose at least one export file first.")
        if len(paths) > 8:
            raise ValueError("Choose at most 8 files at a time.")
        for path in paths:
            if not path.exists():
                raise ValueError(f"That file is not on this computer: {path}")
        return paths

    def _history_pairs(self, raw: Any):
        paths = self._history_paths(raw)
        try:
            history = load_history(paths)
        except Exception as exc:  # noqa: BLE001 - shown to the person picking files
            raise ValueError(f"That export could not be read. {exc}") from exc
        return paths, history, history.pairs

    def history_preview(self, paths: Any) -> dict[str, Any]:
        """Counts only, like `km history`: no prompt text leaves this call."""
        _paths, history, pairs = self._history_pairs(paths)
        return {
            "conversations": len(history.conversations),
            "prompts": len(pairs),
            "by_source": history.by_source(),
        }

    def micro_eval_estimate(
        self, *, paths: Any, judge: str, limit: int | None = None
    ) -> dict[str, Any]:
        _paths, _history, pairs = self._history_pairs(paths)
        judge = (judge or "").strip()
        if not judge:
            raise ValueError("Pick the AI that should mark your old answers.")
        prices = self._prices_for([judge])
        problems = judge_problems(judge, self.env)
        cache = VerdictCache(self.settings.runs_dir / "judge-cache.jsonl")
        worst = (
            None
            if problems
            else estimate_judge_cost_usd(pairs, judge, prices, limit=limit, cache=cache)
        )
        selection = select_pairs(pairs, limit)
        rate = self.settings.usd_to_inr
        return {
            "prompts": len(pairs),
            "selected": len(selection.pairs),
            "duplicates": selection.duplicates,
            "oversized": selection.oversized,
            "judge": judge,
            "problems": problems,
            "plain_problems": [_plain_problem(item) for item in problems],
            "worst_case_usd": worst,
            "worst_case_inr": None if worst is None else worst * rate,
            "paid": bool(worst),
            "usd_to_inr": rate,
        }

    def micro_eval(
        self,
        *,
        paths: Any,
        judge: str,
        name: str = "",
        max_usd: float = 1.0,
        confirmed: bool = False,
        limit: int | None = None,
        min_confidence: float = 0.7,
        on_event: Callable[[dict[str, Any]], None] | None = None,
    ) -> dict[str, Any]:
        if confirmed is not True:
            raise ValueError("Confirm the cost first. Nothing was called.")
        estimate = self.micro_eval_estimate(paths=paths, judge=judge, limit=limit)
        worst = estimate["worst_case_usd"]
        if worst is None:
            plain = " ".join(estimate["plain_problems"])
            raise ValueError(plain or "The marking AI has no checked price, so nothing was called.")
        if not math.isfinite(max_usd) or max_usd < 0 or worst > max_usd:
            raise ValueError(
                "The worst-case marking cost is above your spending cap. Nothing was called."
            )
        judge_spec = estimate["judge"]
        return self._micro_eval_with(
            paths=paths,
            judge_spec=judge_spec,
            judge_fn=make_judge(judge_spec, env=self.env),
            name=name,
            limit=limit,
            min_confidence=min_confidence,
            on_event=on_event,
        )

    def _micro_eval_with(
        self, *, paths, judge_spec, judge_fn, name="", limit=None, min_confidence=0.7, on_event=None
    ) -> dict[str, Any]:
        _paths, _history, pairs = self._history_pairs(paths)
        if not pairs:
            raise ValueError("No questions with answers were found in that export.")
        with self._lock:
            if self._busy:
                raise ValueError("A check is already running.")
            self._busy = True
        try:
            cache = VerdictCache(self.settings.runs_dir / "judge-cache.jsonl")

            def progress(done: int, total: int, _pair: Any) -> None:
                if on_event:
                    on_event({"type": "mining", "done": done, "total": total})

            mined = mine_failures(
                pairs,
                judge_fn,
                judge_spec=judge_spec,
                min_confidence=min_confidence,
                limit=limit,
                on_progress=progress,
                cache=cache,
            )
            counts = {
                "checked": mined.checked,
                "correct": mined.correct,
                "unclear": mined.unclear,
                "below_threshold": mined.below_threshold,
                "errors": mined.errors,
                "cache_hits": mined.cache_hits,
                "duplicates": mined.duplicates,
                "oversized": mined.oversized,
            }
            if not mined.failures:
                return {"built": False, **counts}
            directory = self.my_tests_dir()
            directory.mkdir(parents=True, exist_ok=True)
            slug = "-".join(
                part for part in re.sub(r"[^a-z0-9]+", "-", (name or "").lower()).split("-") if part
            )
            base = f"my-{slug}" if slug else "my-micro-eval"
            suite_id = base
            if (directory / f"{suite_id}.yaml").exists():
                suite_id = f"{base}-{secrets.token_hex(3)}"
            suite, _evidence = write_micro_eval(
                mined.failures,
                directory / f"{suite_id}.yaml",
                name=suite_id,
                version="1",
                judge_spec=judge_spec,
            )
            return {
                "built": True,
                "suite_id": suite.name,
                "tasks": len(suite.tasks),
                "judge": judge_spec,
                **counts,
            }
        finally:
            with self._lock:
                self._busy = False

    def cancel(self) -> dict[str, bool]:
        self._cancel.set()
        return {"cancelling": True}

    def _run_or_raise(self, run_id: str):
        if not _RUN_ID.fullmatch(run_id or ""):
            raise ValueError("Unknown run.")
        if not self.settings.db_path.is_file():
            raise ValueError("No results yet.")
        with Store(self.settings.db_path) as store:
            run = store.get_run(run_id)
            if run is None:
                raise ValueError("That run is not on this computer.")
            payload = result_export(store, run_id, include_outputs=False).model_dump(mode="json")
        return run, payload

    def _decorate(self, payload: dict[str, Any]) -> dict[str, Any]:
        if not self._names:
            self.catalog()
        for model in payload.get("models", []):
            runner_id = model.get("runner_id")
            if not model.get("configuration_name"):
                model["configuration_name"] = self._names.get(runner_id, runner_id)
        return payload

    def report(self, run_id: str) -> dict[str, Any]:
        _run, payload = self._run_or_raise(run_id)
        return self._decorate(payload)

    def list_runs(self) -> list[dict[str, Any]]:
        if not self.settings.db_path.is_file():
            return []
        with Store(self.settings.db_path) as store:
            runs = store.list_runs(limit=30)
        rate = self.settings.usd_to_inr
        return [
            {
                "id": run.id,
                "suite": run.suite_name,
                "status": run.status,
                "runners": run.runners,
                "spent_usd": run.spent_usd,
                "spent_inr": run.spent_usd * rate,
                "started_at": run.started_at,
                "repeats": run.repeats,
            }
            for run in runs
        ]

    def share_card(self, run_id: str) -> dict[str, Any]:
        run, payload = self._run_or_raise(run_id)
        payload = self._decorate(payload)
        html = render_share_card(payload, run.spent_usd, run.usd_to_inr)
        path = Path(run.artifacts_dir) / "share-card.html"
        path.write_text(html, encoding="utf-8")
        summary = share_summary(payload)
        return {"path": str(path), "html": html, "summary": summary, "honesty": HONESTY}

    def dashboard_payload(self, run_id: str) -> dict[str, Any]:
        run, payload = self._run_or_raise(run_id)
        payload = self._decorate(payload)
        suite = self._load_named(run.suite_name)
        titles = {**TITLES, **DEMO_TITLES}
        tasks = [
            {
                "id": task.id,
                "title": titles.get(task.id, task.id),
                "category": task.category,
                "language": task.language,
                "suite": suite.name,
                "track": task.capability_track,
                "instruction": "",
                "context": "",
                "expected": {},
                "grading": "Scored on this computer. Questions and answers stay in the run folder.",
                "full_prompt": "",
                "prompt_sha256": "",
            }
            for task in suite.tasks
        ]
        results = payload["results"]
        graded = [row for row in results if row.get("grade_status") == "graded"]
        job = {
            "id": run.id,
            "status": "completed" if run.status == "completed" else run.status,
            "processed": len(results),
            "total": len(results),
            "graded": len(graded),
            "passed": sum(row.get("passed") is True for row in graded),
            "execution_errors": sum(row.get("status") == "error" for row in results),
            "grader_errors": sum(row.get("grade_status") == "error" for row in results),
            "pending": sum(row.get("grade_status") == "pending" for row in results),
            "skipped": sum(row.get("status") == "skipped" for row in results),
            "events": [],
            "error": None,
            "run_id": run.id,
            "created_at": run.started_at,
            "request": {
                "adapter_ids": list(run.runners),
                "suite": run.suite_name,
                "repeats": run.repeats,
            },
        }
        adapters = [
            {
                "id": "desktop:" + model["runner_id"],
                "name": model.get("configuration_name") or model["runner_id"],
                "ready": True,
                "track": model.get("evaluation_track") or "unverified",
                "cost": "",
                "reason": "",
            }
            for model in payload["models"]
        ]
        return {
            "tasks": tasks,
            "capabilities": {"adapters": adapters},
            "job": job,
            "export": payload,
            "runs": self.list_runs(),
        }

    def attach_http(self, base_url: str) -> None:
        self._external_base = base_url.rstrip("/")

    def dashboard_url(self, run_id: str, theme: str = "system") -> str:
        if theme not in {"system", "light", "dark"}:
            theme = "system"
        if not _RUN_ID.fullmatch(run_id):
            raise ValueError("Unknown run.")
        if self._external_base is None:
            self._ensure_dashboard_server()
        base = self._external_base or f"http://127.0.0.1:{self._httpd.server_port}"
        return f"{base}/board/{self._token}/?run={quote(run_id)}&theme={quote(theme)}"

    def _ensure_dashboard_server(self) -> None:
        if self._httpd is not None:
            return
        handler = make_handler(self, shell=False)
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, name="km-dashboard", daemon=True)
        thread.start()
        self._httpd = server


def render_share_card(payload: dict[str, Any], spent_usd: float, usd_to_inr: float) -> str:
    """A single HTML file someone can save or pass on. No prompts and no model answers."""
    logo = resource_root() / "preview" / "brand" / "km-chakra-wordmark.svg"
    mark = ""
    if logo.is_file():
        raw = logo.read_text(encoding="utf-8")
        if "<script" not in raw.lower():
            mark = raw
    rows = []
    by_runner: dict[str, dict[str, Any]] = {}
    for row in payload["results"]:
        bucket = by_runner.setdefault(row["runner_id"], {"right": 0, "graded": 0, "waits": []})
        if row.get("grade_status") == "graded":
            bucket["graded"] += 1
            if row.get("passed") is True:
                bucket["right"] += 1
        if isinstance(row.get("latency_s"), int | float):
            bucket["waits"].append(row["latency_s"])
    names = {
        model["runner_id"]: model.get("configuration_name") or model["runner_id"]
        for model in payload["models"]
    }
    for runner_id, bucket in by_runner.items():
        waits = sorted(bucket["waits"])
        median = waits[len(waits) // 2] if waits else None
        wait = "not measured" if median is None else f"{median:.2f} s"
        rows.append(
            f"<tr><td>{_escape(names.get(runner_id, runner_id))}</td>"
            f"<td>{bucket['right']} / {bucket['graded']}</td><td>{wait}</td></tr>"
        )
    verdicts = []
    for verdict in payload.get("verdicts") or []:
        if verdict["verdict"] == "clearly_different":
            label = "Clearly different"
        else:
            label = "Too close to call"
        left = names.get(verdict["runner_a"], verdict["runner_a"])
        right = names.get(verdict["runner_b"], verdict["runner_b"])
        pair = f"{left} vs {right}"
        item = f"<li>{_escape(pair)} · {_escape(verdict['metric'])}: {_escape(label)}</li>"
        verdicts.append(item)
    if not verdicts:
        verdicts.append(
            "<li>One system was checked, so there is no comparison and no ranking.</li>"
        )
    spent = f"${spent_usd:.4f} (₹{spent_usd * usd_to_inr:.2f})"
    title = _escape(payload.get("benchmark", {}).get("name") or "Run")
    return f"""<!doctype html>
<html lang="en-IN"><head><meta charset="utf-8"><title>Krutrim Mulyankan · {title}</title>
<style>
body{{margin:0;background:#F3F3F1;color:#171717;
font:15px/1.5 "Sora","Noto Sans Devanagari",sans-serif}}
main{{max-width:720px;margin:32px auto;padding:28px;background:#fff;border:1px solid #D8D8D5}}
.logo svg{{width:240px;height:auto}}
.chips span{{display:inline-block;margin:0 8px 8px 0;padding:6px 8px;border:1px solid #171717;
font-size:11px;letter-spacing:.08em;text-transform:uppercase}}
table{{border-collapse:collapse;width:100%;margin:16px 0}}
td,th{{text-align:left;padding:8px;border-bottom:1px solid #D8D8D5}}
</style></head><body><main>
<div class="logo">{mark}</div>
<p>Krutrim Mulyankan · result card</p>
<div class="chips"><span>{HONESTY["pilot"]}</span>
<span>{HONESTY["ranking"]}</span><span>{HONESTY["close"]}</span></div>
<h1>{title}</h1>
<p>{_escape(HONESTY["close_means"])}</p>
<table><tr><th>System</th><th>Right</th><th>Typical wait</th></tr>{"".join(rows)}</table>
<ul>{"".join(verdicts)}</ul>
<p>Known spend {spent}. This is not a model ranking. Questions and answers are not on this card.</p>
</main></body></html>
"""


def share_summary(payload: dict[str, Any]) -> str:
    name = payload.get("benchmark", {}).get("name") or "a run"
    lines = [
        f"Krutrim Mulyankan · {name}",
        HONESTY["pilot"],
        HONESTY["ranking"],
        HONESTY["close_means"],
    ]
    return "\n".join(lines)


def _escape(value: object) -> str:
    text = str(value)
    return (
        text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")
    )


def render_dashboard_html(session: DesktopSession, run_id: str) -> str:
    """Serve preview/index.html with a script tag inserted. The file on disk is not modified."""
    path = session.preview_dir / "index.html"
    html = path.read_text(encoding="utf-8")
    if _SCRIPT_TAG not in html:
        raise RuntimeError(
            "preview/index.html no longer has the script tag the desktop app attaches to. "
            "The file was not modified."
        )
    payload = json.dumps(session.dashboard_payload(run_id), ensure_ascii=False)
    payload = payload.replace("<", "\\u003c")
    theme_boot = (
        "<script>try{var t=new URLSearchParams(location.search).get('theme');"
        "if(t==='light'||t==='dark')localStorage.setItem('km.theme.v1',t);}catch(e){}</script>"
    )
    html = html.replace(
        '<script src="theme.js"></script>',
        theme_boot + '<script src="theme.js"></script>',
        1,
    )
    html = html.replace(
        _SCRIPT_TAG,
        _SCRIPT_TAG + '<script src="/desktop/dashboard-shim.js" defer></script>',
        1,
    )
    html = html.replace("<head>", f'<head><base href="/board/{session._token}/">', 1)
    html = html.replace("</head>", f"<script>window.__KM_DASHBOARD__={payload};</script></head>", 1)
    return html


def _safe_join(root: Path, relative: str) -> Path | None:
    if not relative or relative.startswith("/") or ".." in Path(relative).parts:
        return None
    candidate = (root / relative).resolve()
    try:
        candidate.relative_to(root.resolve())
    except ValueError:
        return None
    return candidate if candidate.is_file() else None


def make_handler(session: DesktopSession, *, shell: bool):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def log_message(self, fmt: str, *args: object) -> None:
            return

        def _send(self, status: int, body: bytes, content_type: str) -> None:
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.end_headers()
            self.wfile.write(body)

        def _token_ok(self, supplied: str) -> bool:
            expected = session._token
            if len(supplied) != len(expected):
                return False
            return secrets.compare_digest(supplied, expected)

        def do_GET(self) -> None:  # noqa: N802
            path, _, raw_query = self.path.partition("?")
            query = parse_qs(raw_query)
            if shell and path in {"/", "/index.html"}:
                file = ui_dir() / "index.html"
                self._send(200, file.read_bytes(), "text/html; charset=utf-8")
                return
            if shell and path.startswith("/brand/"):
                target = _safe_join(ui_dir(), path.lstrip("/"))
                if target is None:
                    self._send(404, b"Not found", "text/plain; charset=utf-8")
                    return
                kind = "font/woff2" if target.suffix == ".woff2" else _mime(target.suffix)
                self._send(200, target.read_bytes(), kind)
                return
            if shell and path.lstrip("/") in SHELL_FILES and path != "/index.html":
                name = path.lstrip("/")
                self._send(200, (ui_dir() / name).read_bytes(), SHELL_FILES[name])
                return
            if path == "/desktop/dashboard-shim.js":
                file = ui_dir() / "dashboard-shim.js"
                self._send(200, file.read_bytes(), "text/javascript; charset=utf-8")
                return
            if path.startswith("/board/"):
                rest = path[len("/board/") :]
                token, _, relative = rest.partition("/")
                if not self._token_ok(token):
                    self._send(403, b"Open this page from the app.", "text/plain; charset=utf-8")
                    return
                if relative in {"", "index.html"}:
                    run_id = (query.get("run") or [""])[0]
                    try:
                        html = render_dashboard_html(session, run_id)
                    except (OSError, RuntimeError, ValueError) as exc:
                        message = session.scrub(str(exc)) or "Results are unavailable."
                        self._send(400, message.encode(), "text/plain; charset=utf-8")
                        return
                    self._send(200, html.encode(), "text/html; charset=utf-8")
                    return
                if relative in PREVIEW_FILES:
                    file = session.preview_dir / relative
                    self._send(200, file.read_bytes(), PREVIEW_FILES[relative])
                    return
                brand_root = (session.preview_dir / "brand").resolve()
                target = _safe_join(session.preview_dir, relative)
                if target is None:
                    self._send(404, b"Not found", "text/plain; charset=utf-8")
                    return
                try:
                    target.resolve().relative_to(brand_root)
                except ValueError:
                    self._send(404, b"Not found", "text/plain; charset=utf-8")
                    return
                self._send(200, target.read_bytes(), _mime(target.suffix))
                return
            self._send(404, b"Not found", "text/plain; charset=utf-8")

        def do_POST(self) -> None:  # noqa: N802
            if self.path != "/rpc":
                self._send(404, b"Not found", "text/plain; charset=utf-8")
                return
            length = int(self.headers.get("Content-Length", "0") or 0)
            if length > 262_144:
                self._send(413, b"Message is too large.", "text/plain; charset=utf-8")
                return
            try:
                request = json.loads(self.rfile.read(length) or b"{}")
            except json.JSONDecodeError:
                self._send(400, b"The message could not be read.", "text/plain; charset=utf-8")
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()

            def emit(message: dict[str, Any]) -> None:
                self.wfile.write((json.dumps(message, ensure_ascii=False) + "\n").encode())
                self.wfile.flush()

            dispatch(session, request, emit)

    return Handler


def _mime(suffix: str) -> str:
    return {
        ".svg": "image/svg+xml",
        ".png": "image/png",
        ".ico": "image/x-icon",
        ".woff2": "font/woff2",
        ".txt": "text/plain; charset=utf-8",
        ".css": "text/css; charset=utf-8",
        ".js": "text/javascript; charset=utf-8",
        ".html": "text/html; charset=utf-8",
    }.get(suffix, "application/octet-stream")


def dispatch(
    session: DesktopSession, request: dict[str, Any], emit: Callable[[dict], None]
) -> None:
    ident = request.get("id")
    method = request.get("method")
    params = request.get("params") or {}
    if not isinstance(params, dict):
        emit({"id": ident, "ok": False, "error": "The message could not be read."})
        return
    try:
        if method == "ping":
            result = session.ping()
        elif method == "catalog":
            result = session.catalog()
        elif method == "key_status":
            result = session.key_status()
        elif method == "set_keys":
            result = session.set_keys(params.get("keys") or {})
        elif method == "set_urls":
            result = session.set_urls(params.get("urls") or {})
        elif method == "estimate":
            result = session.estimate(
                suite=params.get("suite", ""),
                runners=params.get("runners") or [],
                repeats=int(params.get("repeats") or 1),
                max_usd=float(params.get("max_usd") if params.get("max_usd") is not None else 1),
                usd_to_inr=params.get("usd_to_inr"),
            )
        elif method == "run":

            def on_event(event: dict[str, Any]) -> None:
                emit({"id": ident, "event": "progress", "result": event})

            result = session.run(
                suite=params.get("suite", ""),
                runners=params.get("runners") or [],
                repeats=int(params.get("repeats") or 1),
                max_usd=float(params.get("max_usd") if params.get("max_usd") is not None else 1),
                confirmed=params.get("confirmed") is True,
                seed=int(params.get("seed") or 0),
                usd_to_inr=params.get("usd_to_inr"),
                on_event=on_event,
            )
        elif method == "history_preview":
            result = session.history_preview(params.get("paths"))
        elif method == "micro_eval_estimate":
            result = session.micro_eval_estimate(
                paths=params.get("paths"),
                judge=str(params.get("judge") or ""),
                limit=params.get("limit"),
            )
        elif method == "micro_eval":

            def on_mining(event: dict[str, Any]) -> None:
                emit({"id": ident, "event": "progress", "result": event})

            result = session.micro_eval(
                paths=params.get("paths"),
                judge=str(params.get("judge") or ""),
                name=str(params.get("name") or ""),
                max_usd=float(params.get("max_usd") if params.get("max_usd") is not None else 1),
                confirmed=params.get("confirmed") is True,
                limit=params.get("limit"),
                min_confidence=float(params.get("min_confidence") or 0.7),
                on_event=on_mining,
            )
        elif method == "cancel":
            result = session.cancel()
        elif method == "report":
            result = session.report(str(params.get("run_id") or ""))
        elif method == "list_runs":
            result = session.list_runs()
        elif method == "share_card":
            result = session.share_card(str(params.get("run_id") or ""))
        elif method == "dashboard_url":
            result = {
                "url": session.dashboard_url(
                    str(params.get("run_id") or ""), str(params.get("theme") or "system")
                )
            }
        else:
            raise ValueError("Unknown action.")
        emit({"id": ident, "ok": True, "result": result})
    except (OSError, RuntimeError, TypeError, ValueError) as exc:
        emit(
            {
                "id": ident,
                "ok": False,
                "error": session.scrub(str(exc)) or "Something went wrong.",
            }
        )


def serve_stdio(session: DesktopSession | None = None) -> None:
    session = session or DesktopSession()
    write_lock = threading.Lock()

    def emit(message: dict[str, Any]) -> None:
        line = json.dumps(message, ensure_ascii=False)
        with write_lock:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    for raw in sys.stdin:
        if not raw.strip():
            continue
        try:
            request = json.loads(raw)
        except json.JSONDecodeError:
            emit({"id": None, "ok": False, "error": "The message could not be read."})
            continue
        if not isinstance(request, dict):
            emit({"id": None, "ok": False, "error": "The message could not be read."})
            continue
        if request.get("method") in {"run", "micro_eval", "micro_eval_estimate", "history_preview"}:
            threading.Thread(
                target=dispatch, args=(session, request, emit), name="km-run", daemon=True
            ).start()
        else:
            dispatch(session, request, emit)


def serve_http(port: int = 0) -> None:
    """Loopback-only shell used for local preview and screenshots. Not for a public network."""
    session = DesktopSession()
    handler = make_handler(session, shell=True)
    server = ThreadingHTTPServer(("127.0.0.1", port), handler)
    session.attach_http(f"http://127.0.0.1:{server.server_port}")
    sys.stderr.write(f"Krutrim Mulyankan desktop http://127.0.0.1:{server.server_port}/\n")
    sys.stderr.flush()
    server.serve_forever()


def main(argv: list[str] | None = None) -> None:
    args = list(sys.argv[1:] if argv is None else argv)
    if args and args[0] == "--http":
        port = int(args[1]) if len(args) > 1 else 0
        serve_http(port)
        return
    serve_stdio()


if __name__ == "__main__":
    main()
