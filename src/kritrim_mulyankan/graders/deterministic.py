"""Strict reference graders. Never execute model output or use a paid judge."""

from __future__ import annotations

import json
import re
import unicodedata
from decimal import Decimal, InvalidOperation
from typing import Any

from kritrim_mulyankan.graders.base import Grade, Grader
from kritrim_mulyankan.suite import JSONGraderSpec, NumericGraderSpec, RegexGraderSpec, Task


def _reject_constant(value: str) -> None:
    raise ValueError(f"nonfinite JSON value: {value}")


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in pairs:
        key = unicodedata.normalize("NFC", key)
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def strict_json(text: str, *, decimal_numbers: bool = True) -> Any:
    return json.loads(
        text,
        parse_constant=_reject_constant,
        object_pairs_hook=_unique_object,
        parse_float=Decimal if decimal_numbers else float,
    )


def _same(a: Any, b: Any) -> bool:
    # Python treats True == 1; JSON grading must distinguish them.
    if isinstance(a, bool) or isinstance(b, bool):
        return type(a) is type(b) and a == b
    if isinstance(a, str) and isinstance(b, str):
        return unicodedata.normalize("NFC", a) == unicodedata.normalize("NFC", b)
    if isinstance(a, (int, float, Decimal)) and isinstance(b, (int, float, Decimal)):
        return Decimal(str(a)) == Decimal(str(b))
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_same(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_same(x, y) for x, y in zip(a, b, strict=True))
    return a == b


class JSONGrader(Grader):
    name = "json"

    def __init__(self, spec: JSONGraderSpec) -> None:
        self.spec = spec

    def grade(self, task: Task, output: str) -> Grade:
        try:
            parsed = strict_json(output)
            passed = _same(parsed, self.spec.expected)
            reason = "matched" if passed else "JSON differs from reference"
            details = {"parse_valid": True, "field_checks": {}, "extra_fields": []}
            if isinstance(parsed, dict) and isinstance(self.spec.expected, dict):
                details["field_checks"] = {
                    key: key in parsed and _same(parsed[key], value)
                    for key, value in self.spec.expected.items()
                }
                details["extra_fields"] = sorted(set(parsed) - set(self.spec.expected))
        except (ValueError, RecursionError):
            passed, reason = False, "invalid JSON (no prose, fences, duplicate keys or NaN)"
            details = {"parse_valid": False, "field_checks": {}, "extra_fields": []}
        return Grade(
            grader=self.name,
            status="graded",
            passed=passed,
            score=float(passed),
            details={"reason": reason, **details},
        )


def indic_number(text: str) -> Decimal:
    text = unicodedata.normalize("NFKC", text).strip().casefold()
    text = "".join(str(unicodedata.decimal(c)) if c.isdecimal() else c for c in text)
    match = re.fullmatch(
        r"(?:₹|rs\.?|inr)?\s*([+-]?(?:\d+(?:,\d+)*)(?:\.\d+)?)"
        r"\s*(lakh|lakhs|crore|crores|लाख|करोड़|करोड)?",
        text,
    )
    if not match:
        raise ValueError("expected a single number, optionally ₹/INR and lakh/crore")
    number, unit = match.groups()
    if "," in number and not re.fullmatch(
        r"[+-]?(?:\d{1,3}(?:,\d{3})+|\d{1,2}(?:,\d{2})*,\d{3})(?:\.\d+)?", number
    ):
        raise ValueError("invalid digit grouping")
    multiplier = 100_000 if unit in {"lakh", "lakhs", "लाख"} else 10_000_000 if unit else 1
    return Decimal(number.replace(",", "")) * multiplier


class NumericGrader(Grader):
    name = "numeric"

    def __init__(self, spec: NumericGraderSpec) -> None:
        self.spec = spec

    def grade(self, task: Task, output: str) -> Grade:
        try:
            value = indic_number(output)
            passed = abs(value - self.spec.expected) <= self.spec.tolerance
            details = {"parsed": str(value), "tolerance": str(self.spec.tolerance)}
        except (ValueError, InvalidOperation):
            passed, details = False, {"reason": "invalid numeric answer"}
        return Grade(
            grader=self.name, status="graded", passed=passed, score=float(passed), details=details
        )


class RegexGrader(Grader):
    name = "regex"

    def __init__(self, spec: RegexGraderSpec) -> None:
        self.spec = spec

    def grade(self, task: Task, output: str) -> Grade:
        flags = 0 if self.spec.case_sensitive else re.IGNORECASE
        passed = re.fullmatch(self.spec.pattern, output.strip(), flags) is not None
        return Grade(grader=self.name, status="graded", passed=passed, score=float(passed))
