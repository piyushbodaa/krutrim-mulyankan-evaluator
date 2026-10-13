"""Runner registry: turns a spec string like `echo` or `model:openai/gpt-6` into a Runner."""

from __future__ import annotations

from kritrim_mulyankan.pricing import PriceTable
from kritrim_mulyankan.runners.agent import AgentRunner
from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage
from kritrim_mulyankan.runners.fake import EchoRunner, OracleRunner
from kritrim_mulyankan.runners.harness import HarnessRunner
from kritrim_mulyankan.runners.model import ModelRunner

# (spec, kind, implemented, description), shown by `km list runners`.
RUNNER_CATALOG: list[tuple[str, str, bool, str]] = [
    ("echo", "fake", True, "Replies with the prompt. Free; a floor score."),
    ("oracle", "fake", True, "Replies with the expected/reference answer. Free; checks graders."),
    (
        "model:<provider>/<model>[,k=v]",
        "model",
        True,
        "Direct API call. Providers: anthropic, openai, xai, gemini, openrouter, ollama, "
        "vllm, compatible. Options: effort, max_tokens, temperature, retries.",
    ),
    ("harness:<cli>", "harness", False, "Headless coding/agent CLI in a temp workdir."),
    ("agent:<name>", "agent", False, "Personal agent via imported transcripts."),
]


def make_runner(spec: str, *, prices: PriceTable | None = None) -> Runner:
    kind, sep, arg = spec.partition(":")
    if not sep:
        fakes = {"echo": EchoRunner, "oracle": OracleRunner}
        if spec in fakes:
            return fakes[spec]()
    elif arg:
        if kind == "model":
            return ModelRunner.from_spec(arg, prices=prices)
        if kind == "harness":
            return HarnessRunner(arg)
        if kind == "agent":
            return AgentRunner(arg)
    raise ValueError(f"unknown runner {spec!r}; see `km list runners`")


__all__ = [
    "RUNNER_CATALOG",
    "AgentRunner",
    "EchoRunner",
    "HarnessRunner",
    "ModelRunner",
    "OracleRunner",
    "RunContext",
    "RunOutput",
    "Runner",
    "Usage",
    "make_runner",
]
