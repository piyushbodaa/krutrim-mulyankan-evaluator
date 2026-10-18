"""Mine failures from prompt/answer pairs with an automatic LLM judge.

The owner's chosen flow is fully automatic: no human review step. The judge
can be wrong, so this miner is conservative — only `wrong` verdicts at or
above a confidence threshold become eval items, `unclear` is always dropped,
and every kept item records the judge spec, the judge-prompt hash, the reason
and the original answer's hash, so a score can always be traced back to the
judgement that created its test.
"""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Literal

from kritrim_mulyankan.history import PromptPair
from kritrim_mulyankan.judge import JudgeFn
from kritrim_mulyankan.provenance import redact

Verdict = Literal["wrong", "correct", "unclear"]

_JSON_OBJECT = re.compile(r"\{.*\}", re.DOTALL)

MINE_TEMPLATE = """\
You are checking whether an AI assistant's ANSWER to a user's PROMPT was wrong.
Judge factual correctness, arithmetic, logic, code correctness and whether the
prompt's explicit constraints were followed. Do not mark an answer wrong for
style, length or tone. If the prompt has no checkable right answer (chitchat,
creative writing with no constraints, missing context), say "unclear".

<prompt>
{prompt}
</prompt>

<answer>
{answer}
</answer>

Reply with JSON only:
{{"verdict": "wrong" | "correct" | "unclear",
  "confidence": <number 0 to 1>,
  "reason": "<one sentence>",
  "expected": "<the correct answer, or what a correct answer must contain; empty unless wrong>",
  "rubric": "<how to grade a new answer, one or two sentences; empty unless wrong>"}}"""


# Secret shapes pasted into real chat history (plus exact env secrets via
# provenance.redact). The mining judge may be a *different* provider than the
# one the user originally pasted the secret to, so never forward them raw.
_SECRET_PATTERNS = [
    re.compile(r"sk-ant-[A-Za-z0-9_\-]{8,}"),
    re.compile(r"sk-[A-Za-z0-9_\-]{16,}"),
    re.compile(r"gh[pousr]_[A-Za-z0-9]{20,}"),
    re.compile(r"AKIA[0-9A-Z]{16}"),
    re.compile(r"xox[baprs]-[A-Za-z0-9\-]{10,}"),
    re.compile(r"(?i)(bearer\s+)[A-Za-z0-9._\-]{16,}"),
    re.compile(r"(?i)((?:api[_-]?key|auth[_-]?token|secret|password)\s*[:=]\s*)[^\s\"']{8,}"),
]

#: Pairs longer than this (chars, prompt + answer) are skipped, never judged:
#: one giant paste must not dominate the judging budget.
MAX_PAIR_CHARS = 32_000


def redact_secrets(text: str) -> str:
    out = redact(text)
    for pattern in _SECRET_PATTERNS:
        out = pattern.sub(
            lambda m: (m.group(1) + "[REDACTED]") if m.groups() else "[REDACTED]", out
        )
    return out


def build_mine_prompt(pair: PromptPair) -> str:
    return MINE_TEMPLATE.format(
        prompt=redact_secrets(pair.prompt), answer=redact_secrets(pair.answer)
    )


def _canonical_template() -> str:
    # Hash the rendered schema (single braces) so the Python and Rust miners,
    # whose template literals escape braces differently, record the same hash.
    return MINE_TEMPLATE.replace("{{", "{").replace("}}", "}")


def judge_prompt_hash() -> str:
    return hashlib.sha256(_canonical_template().encode()).hexdigest()[:16]


@dataclass
class Selection:
    pairs: list[PromptPair] = field(default_factory=list)
    duplicates: int = 0
    oversized: int = 0


def select_pairs(pairs: list[PromptPair], limit: int | None = None) -> Selection:
    """Dedupe, drop oversized, then sample evenly (never just the oldest)."""
    seen: set[str] = set()
    picked: list[PromptPair] = []
    duplicates = oversized = 0
    for pair in pairs:
        key = hashlib.sha256(f"{pair.prompt}\x00{pair.answer}".encode()).hexdigest()
        if key in seen:
            duplicates += 1
            continue
        seen.add(key)
        if len(pair.prompt) + len(pair.answer) > MAX_PAIR_CHARS:
            oversized += 1
            continue
        picked.append(pair)
    if limit is not None and 0 <= limit < len(picked):
        # range(0) yields nothing, so limit == 0 needs no special case.
        picked = [picked[int(i * len(picked) / limit)] for i in range(limit)]
    return Selection(pairs=picked, duplicates=duplicates, oversized=oversized)


@dataclass(frozen=True)
class MineVerdict:
    verdict: Verdict
    confidence: float
    reason: str
    expected: str
    rubric: str


def parse_verdict(reply: str) -> MineVerdict:
    match = _JSON_OBJECT.search(reply)
    if not match:
        raise ValueError("judge reply contains no JSON object")
    data = json.loads(match.group(0))
    if not isinstance(data, dict):
        raise ValueError("judge reply JSON is not an object")
    verdict = str(data.get("verdict", "")).strip().lower()
    if verdict not in ("wrong", "correct", "unclear"):
        raise ValueError(f"verdict must be wrong/correct/unclear, got {verdict!r}")
    confidence = data.get("confidence")
    if isinstance(confidence, bool) or not isinstance(confidence, int | float):
        raise ValueError(f"confidence must be a number, got {confidence!r}")
    if not 0 <= confidence <= 1:
        raise ValueError(f"confidence must be between 0 and 1, got {confidence!r}")
    return MineVerdict(
        verdict=verdict,  # type: ignore[arg-type]
        confidence=float(confidence),
        reason=str(data.get("reason", "")),
        expected=str(data.get("expected", "")),
        rubric=str(data.get("rubric", "")),
    )


@dataclass(frozen=True)
class MinedFailure:
    pair: PromptPair
    verdict: MineVerdict
    judge_spec: str

    @property
    def answer_sha256(self) -> str:
        return hashlib.sha256(self.pair.answer.encode()).hexdigest()


class VerdictCache:
    """JSONL cache of raw judge replies, keyed by judge + pair content.

    Re-running mining (new threshold, more history, a crash) replays cached
    verdicts instead of paying for them twice. Corrupt lines are ignored.
    """

    def __init__(self, path) -> None:  # noqa: ANN001 - Path, kept loose like prices
        from pathlib import Path

        self.path = Path(path)
        self.entries: dict[str, str] = {}
        if self.path.is_file():
            for line in self.path.read_text(encoding="utf-8").splitlines():
                try:
                    row = json.loads(line)
                    self.entries[str(row["key"])] = str(row["reply"])
                except (ValueError, KeyError, TypeError):
                    continue

    @staticmethod
    def key(pair: PromptPair, judge_spec: str) -> str:
        raw = f"{judge_spec}\x00{judge_prompt_hash()}\x00{pair.prompt}\x00{pair.answer}"
        return hashlib.sha256(raw.encode()).hexdigest()

    def get(self, pair: PromptPair, judge_spec: str) -> str | None:
        return self.entries.get(self.key(pair, judge_spec))

    def put(self, pair: PromptPair, judge_spec: str, reply: str) -> None:
        key = self.key(pair, judge_spec)
        if key in self.entries:
            return
        self.entries[key] = reply
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps({"key": key, "reply": reply}, ensure_ascii=False) + "\n")


@dataclass
class MiningResult:
    failures: list[MinedFailure] = field(default_factory=list)
    checked: int = 0
    correct: int = 0
    unclear: int = 0
    below_threshold: int = 0
    errors: int = 0
    duplicates: int = 0
    oversized: int = 0
    cache_hits: int = 0
    redacted_pairs: int = 0


ProgressFn = Callable[[int, int, PromptPair], None]


def mine_failures(
    pairs: list[PromptPair],
    judge: JudgeFn,
    *,
    judge_spec: str,
    min_confidence: float = 0.7,
    limit: int | None = None,
    on_progress: ProgressFn | None = None,
    cache: VerdictCache | None = None,
) -> MiningResult:
    """Judge every selected pair; keep confident `wrong` verdicts only.

    Selection dedupes repeats, drops oversized pairs and samples evenly
    under `limit`. A judge error on one pair never crashes the run: it is
    counted and the pair is skipped, so a flaky judge yields a smaller eval.
    """
    if not 0 <= min_confidence <= 1:
        raise ValueError("min_confidence must be between 0 and 1")
    selection = select_pairs(pairs, limit)
    selected = selection.pairs
    result = MiningResult(duplicates=selection.duplicates, oversized=selection.oversized)
    for index, pair in enumerate(selected):
        if on_progress is not None:
            on_progress(index + 1, len(selected), pair)
        result.checked += 1
        mine_prompt = build_mine_prompt(pair)
        if "[REDACTED]" in mine_prompt:
            result.redacted_pairs += 1
        try:
            reply = cache.get(pair, judge_spec) if cache is not None else None
            if reply is not None:
                result.cache_hits += 1
            else:
                reply = judge(mine_prompt)
                if cache is not None:
                    cache.put(pair, judge_spec, reply)
            verdict = parse_verdict(reply)
        except Exception:
            result.errors += 1
            continue
        if verdict.verdict == "correct":
            result.correct += 1
        elif verdict.verdict == "unclear":
            result.unclear += 1
        elif verdict.confidence < min_confidence:
            result.below_threshold += 1
        elif not verdict.expected.strip():
            result.unclear += 1  # 'wrong' with no expected answer cannot grade anything
        else:
            result.failures.append(MinedFailure(pair=pair, verdict=verdict, judge_spec=judge_spec))
    return result


def estimate_judge_cost_usd(  # noqa: ANN001
    pairs: list[PromptPair],
    judge_spec: str,
    prices,
    limit: int | None = None,
    cache: VerdictCache | None = None,
) -> float | None:
    """Worst-case mining cost: full pair text in, the judge token ceiling out.

    Returns None when the judge model has no price (callers must refuse paid
    work they cannot bound, matching the model-runner rule).
    """
    from kritrim_mulyankan.judge import DEFAULT_JUDGE_MAX_TOKENS, parse_judge_spec
    from kritrim_mulyankan.pricing import max_input_tokens

    provider, model = parse_judge_spec(judge_spec)
    price = prices.lookup(provider, model)
    if price is None:
        return None
    selected = select_pairs(pairs, limit).pairs
    return sum(
        price.cost(
            max_input_tokens(build_mine_prompt(pair)),
            DEFAULT_JUDGE_MAX_TOKENS,
        )
        for pair in selected
        if cache is None or cache.get(pair, judge_spec) is None
    )
