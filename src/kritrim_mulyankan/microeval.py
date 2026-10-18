"""Turn mined failures into a frozen suite plus an evidence sidecar.

The suite is the test: the user's original prompt verbatim, graded by an
LLM judge against the mined expected answer and rubric. The sidecar
(`<suite>.evidence.json`, written next to the YAML) holds the private
evidence — original answers and full verdicts — and stays out of the suite
file, so the suite can be shared without sharing the history it came from.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

import yaml

from kritrim_mulyankan.miner import MinedFailure, judge_prompt_hash
from kritrim_mulyankan.suite import Suite, Task

_PLACEHOLDER = re.compile(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}")


def _literal_inputs(prompt: str) -> dict[str, str]:
    """Keep user prompts verbatim even when they contain {{ template }} text.

    Suite prompts treat {{ name }} as an input placeholder. Registering each
    found name with its own literal text makes rendering reproduce the
    original prompt instead of failing validation or rewriting the prompt.
    """
    return {name: "{{" + name + "}}" for name in sorted(set(_PLACEHOLDER.findall(prompt)))}


def build_suite(
    failures: list[MinedFailure],
    *,
    name: str,
    version: str,
    judge_spec: str,
) -> Suite:
    if not failures:
        raise ValueError("no mined failures to build a micro-eval from")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name):
        raise ValueError(f"suite name {name!r} must use letters, digits, '.', '-' or '_'")
    tasks: list[Task] = []
    for index, failure in enumerate(failures, start=1):
        pair = failure.pair
        short = hashlib.sha256(f"{pair.prompt}\x00{pair.answer}".encode()).hexdigest()[:8]
        rubric = failure.verdict.rubric.strip() or (
            "The answer is correct only if it matches the reference answer's facts "
            "and follows every constraint in the prompt."
        )
        tasks.append(
            Task.model_validate(
                {
                    "id": f"mine.{pair.source}.{index:04d}-{short}",
                    "version": 1,
                    "category": "personal",
                    "domain": pair.source,
                    "prompt": pair.prompt,
                    "inputs": _literal_inputs(pair.prompt),
                    "grader": {
                        "type": "llm_judge",
                        "mode": "absolute",
                        "judge_model": judge_spec,
                        "pass_threshold": 7,
                    },
                    "rubric": rubric,
                    "reference": failure.verdict.expected,
                    "provenance": [
                        {
                            "source": pair.source,
                            "conversation_id": pair.conversation_id,
                            "conversation_title": pair.conversation_title,
                            "turn_index": pair.turn_index,
                            "original_model": pair.model,
                            "judge": failure.judge_spec,
                            "judge_prompt_hash": judge_prompt_hash(),
                            "judge_confidence": failure.verdict.confidence,
                            "judge_reason": failure.verdict.reason,
                            "original_answer_sha256": failure.answer_sha256,
                        }
                    ],
                }
            )
        )
    return Suite.model_validate(
        {
            "name": name,
            "version": version,
            "description": (
                "Personal micro-eval mined automatically from the owner's own chat "
                "history: prompts their previous model answered wrongly, graded by "
                "an LLM judge against mined expected answers. Judge-graded, not "
                "ground truth."
            ),
            "validation_status": "unvalidated_pilot",
            "tasks": [task.model_dump(mode="json") for task in tasks],
        }
    )


def evidence_payload(suite: Suite, failures: list[MinedFailure], judge_spec: str) -> dict:
    return {
        "schema_version": "1.0",
        "suite": suite.name,
        "version": suite.version,
        "judge": judge_spec,
        "judge_prompt_hash": judge_prompt_hash(),
        "private": (
            "Contains the owner's original prompts and answers. "
            "Keep private; never commit or publish."
        ),
        "items": [
            {
                "task_id": task.id,
                "source": failure.pair.source,
                "conversation_id": failure.pair.conversation_id,
                "conversation_title": failure.pair.conversation_title,
                "turn_index": failure.pair.turn_index,
                "created_at": failure.pair.created_at,
                "original_model": failure.pair.model,
                "prompt": failure.pair.prompt,
                "original_answer": failure.pair.answer,
                "verdict": {
                    "verdict": failure.verdict.verdict,
                    "confidence": failure.verdict.confidence,
                    "reason": failure.verdict.reason,
                    "expected": failure.verdict.expected,
                    "rubric": failure.verdict.rubric,
                },
            }
            for task, failure in zip(suite.tasks, failures, strict=True)
        ],
    }


def write_micro_eval(
    failures: list[MinedFailure],
    destination: Path,
    *,
    name: str,
    version: str,
    judge_spec: str,
) -> tuple[Suite, Path]:
    """Write the suite YAML and its evidence sidecar; never overwrites either."""
    suite = build_suite(failures, name=name, version=version, judge_spec=judge_spec)
    evidence_path = destination.parent / (destination.stem + ".evidence.json")
    if evidence_path.exists():
        raise FileExistsError(f"refusing to overwrite {evidence_path}")
    # Exclusive creation avoids overwriting user-authored suites (as in importer).
    with destination.open("x", encoding="utf-8") as handle:
        yaml.safe_dump(suite.model_dump(mode="json"), handle, allow_unicode=True, sort_keys=False)
    try:
        with evidence_path.open("x", encoding="utf-8") as handle:
            json.dump(
                evidence_payload(suite, failures, judge_spec),
                handle,
                ensure_ascii=False,
                indent=1,
            )
    except BaseException:
        destination.unlink(missing_ok=True)  # never leave a suite without its evidence
        raise
    return suite, evidence_path
