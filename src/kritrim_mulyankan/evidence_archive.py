"""Preserve external consumer-app captures without inventing a frozen-task score."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


def archive_evidence(document: str, destination: Path, *, source_name: str) -> dict:
    data = json.loads(document)
    attempts = data.get("attempts") if isinstance(data, dict) else None
    if not isinstance(attempts, list) or not attempts or len(attempts) > 1000:
        raise ValueError("expected an external evidence package with an attempts array")
    if any(
        not isinstance(item, dict)
        or not isinstance(item.get("app"), str)
        or not isinstance(item.get("task"), str)
        for item in attempts
    ):
        raise ValueError("each external attempt requires app and task labels")
    digest = hashlib.sha256(document.encode()).hexdigest()
    directory = destination / digest
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "source.json").write_text(document)
    summary = {
        "id": digest,
        "kind": "external_app_evidence",
        "source_name": source_name,
        "source_sha256": digest,
        "submission_count": len(attempts),
        "visible_answer_count": sum(
            len(item["response_variants"])
            if isinstance(item.get("response_variants"), list) and item["response_variants"]
            else 1
            for item in attempts
        ),
        "submission_count_unit": (
            "Operator-submitted UI/CLI prompts; backend requests uninstrumented"
        ),
        "clients": sorted({item["app"] for item in attempts}),
        "external_tasks": sorted({item["task"] for item in attempts}),
        "grading_status": "unscored_task_mapping_missing",
        "score": None,
        "exact_backend_model": None,
        "cost": None,
        "measured_elapsed_s": None,
        "limitation": data.get(
            "limit_notice", "External captures need task/protocol mapping and review."
        ),
    }
    (directory / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return summary


def list_evidence(destination: Path) -> list[dict]:
    if not destination.is_dir():
        return []
    return [json.loads(path.read_text()) for path in sorted(destination.glob("*/summary.json"))]


def evidence_detail(destination: Path, identifier: str) -> dict:
    if len(identifier) != 64 or any(c not in "0123456789abcdef" for c in identifier):
        raise ValueError("unknown evidence archive")
    directory = destination / identifier
    summary = json.loads((directory / "summary.json").read_text())
    data = json.loads((directory / "source.json").read_text())
    rows = []
    for index, attempt in enumerate(data["attempts"]):
        rows.append(
            {
                "index": index + 1,
                "app": attempt["app"],
                "task": attempt["task"],
                "response": attempt.get("response"),
                "response_variants": [
                    {"label": variant.get("label"), "response": variant.get("response")}
                    for variant in attempt.get("response_variants", [])
                    if isinstance(variant, dict)
                ],
                "submission_utc": attempt.get("submission_call_utc"),
                "capture_utc": attempt.get("evidence_capture_utc"),
                "visible_model_label": attempt.get("visible_model")
                or attempt.get("visible_model_mode")
                or attempt.get("visible_mode"),
                "scoring_status": "unscored_task_mapping_missing",
                "score": None,
                "cost": None,
                "measured_elapsed_s": None,
                "source_grade_claim": attempt.get("grade"),
            }
        )
    return {"summary": summary, "attempts": rows}
