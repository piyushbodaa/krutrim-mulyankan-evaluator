"""Paired verdicts: is a gap between two systems real, or too close to call?

Every system in a run answers the same tasks, so each task is a matched pair. Accuracy compares
the share of tries each system got right on a task; speed compares its typical (median) wait.
The verdict comes from an exact two-sided sign test over the tasks where the pair differ; with
one try per task that is McNemar's exact test. A paired bootstrap over tasks gives a 95% range
for the size of the gap, with a fixed seed so every export is reproducible. With more than two
systems, p-values are Holm-adjusted within each metric so that more pairs do not mean more false
alarms. Systems on different evaluation tracks (mock controls, CLI harnesses, model APIs,
recorded apps) are never compared.
"""

from __future__ import annotations

import math
import random
from collections import defaultdict
from collections.abc import Callable, Mapping, Sequence
from itertools import combinations
from statistics import mean, median
from typing import Any, Literal

from pydantic import BaseModel

ALPHA = 0.05
RESAMPLES = 4000
SEED = 20261014
EPSILON = 1e-12
METHOD = "Exact two-sided sign test over paired tasks; 95% paired bootstrap range (4000 resamples)."


class Verdict(BaseModel):
    metric: Literal["accuracy", "speed"]
    capability_track: str
    runner_a: str
    runner_b: str
    verdict: Literal["clearly_different", "too_close_to_call"]
    favours: str | None
    estimate: float
    estimate_unit: Literal["percentage_points", "times_as_long"]
    ci_95: tuple[float, float] | None
    p_value: float
    p_value_adjusted: float
    tasks: int
    a_better: int
    b_better: int
    tied: int
    tries_per_task: int
    alpha: float = ALPHA
    method: str = METHOD
    explanation: str


def sign_test(wins: int, losses: int) -> float:
    """Exact two-sided p-value for a wins/losses split if neither side is really better."""
    n = wins + losses
    if n == 0:
        return 1.0
    tail = sum(math.comb(n, k) for k in range(min(wins, losses) + 1))
    return min(1.0, 2 * tail / 2**n)


def holm(p_values: Sequence[float]) -> list[float]:
    """Holm step-down adjustment; keeps the chance of any false alarm at or below alpha."""
    order = sorted(range(len(p_values)), key=lambda index: p_values[index])
    adjusted = [1.0] * len(p_values)
    running = 0.0
    for rank, index in enumerate(order):
        running = max(running, min(1.0, (len(p_values) - rank) * p_values[index]))
        adjusted[index] = running
    return adjusted


def bootstrap_ci(
    values: Sequence[float], statistic: Callable[[list[float]], float] = mean
) -> tuple[float, float] | None:
    """95% percentile interval from resampling whole tasks; None with fewer than two tasks."""
    if len(values) < 2:
        return None
    rng = random.Random(SEED)
    samples = sorted(statistic(rng.choices(values, k=len(values))) for _ in range(RESAMPLES))
    return _quantile(samples, 0.025), _quantile(samples, 0.975)


def _quantile(ordered: list[float], q: float) -> float:
    position = q * (len(ordered) - 1)
    low, high = math.floor(position), math.ceil(position)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def task_tries(rows: Sequence[Mapping[str, Any]]) -> dict[str, tuple[int, int]]:
    """Per task: (tries answered correctly, tries counted). Execution errors count as wrong,
    as in the success rate; skipped tries and tries still awaiting a grade are left out."""
    counts: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    for row in rows:
        if row["status"] == "error":
            counts[row["task_id"]][1] += 1
        elif row["status"] == "ok" and row.get("grade_status") == "graded":
            counts[row["task_id"]][0] += row.get("passed") is True
            counts[row["task_id"]][1] += 1
    return {task: (right, tries) for task, (right, tries) in counts.items() if tries}


def task_waits(rows: Sequence[Mapping[str, Any]]) -> dict[str, float]:
    """Per task: the median wait of the tries that returned an answer."""
    waits: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        latency = row.get("latency_s")
        if row["status"] == "ok" and isinstance(latency, int | float) and latency > 0:
            waits[row["task_id"]].append(float(latency))
    return {task: median(values) for task, values in waits.items()}


def _accuracy(capability: str, a: str, b: str, rows_a, rows_b) -> dict[str, Any] | None:
    tries_a, tries_b = task_tries(rows_a), task_tries(rows_b)
    shared = sorted(set(tries_a) & set(tries_b))
    if not shared:
        return None
    gaps = [tries_a[t][0] / tries_a[t][1] - tries_b[t][0] / tries_b[t][1] for t in shared]
    wins, losses = sum(g > EPSILON for g in gaps), sum(g < -EPSILON for g in gaps)
    centre, interval = mean(gaps), bootstrap_ci(gaps)
    return {
        "metric": "accuracy",
        "capability_track": capability,
        "runner_a": a,
        "runner_b": b,
        "favours": a if centre > EPSILON else b if centre < -EPSILON else None,
        "estimate": centre * 100,
        "estimate_unit": "percentage_points",
        "ci_95": (interval[0] * 100, interval[1] * 100) if interval else None,
        "p_value": sign_test(wins, losses),
        "tasks": len(shared),
        "a_better": wins,
        "b_better": losses,
        "tied": len(shared) - wins - losses,
        "tries_per_task": max(max(tries_a[t][1], tries_b[t][1]) for t in shared),
    }


def _speed(capability: str, a: str, b: str, rows_a, rows_b) -> dict[str, Any] | None:
    waits_a, waits_b = task_waits(rows_a), task_waits(rows_b)
    shared = sorted(set(waits_a) & set(waits_b))
    if not shared:
        return None
    # Log of how many times longer b took than a; positive means a was faster on that task.
    logs = [math.log(waits_b[t] / waits_a[t]) for t in shared]
    wins, losses = sum(v > EPSILON for v in logs), sum(v < -EPSILON for v in logs)
    centre, interval = mean(logs), bootstrap_ci(logs)
    if centre >= 0:
        favours, estimate = (a if centre > EPSILON else None), math.exp(centre)
        ci = (math.exp(interval[0]), math.exp(interval[1])) if interval else None
    else:
        favours, estimate = b, math.exp(-centre)
        ci = (math.exp(-interval[1]), math.exp(-interval[0])) if interval else None
    tries = defaultdict(int)
    for row in [*rows_a, *rows_b]:
        tries[(row["runner_id"], row["task_id"])] += 1
    return {
        "metric": "speed",
        "capability_track": capability,
        "runner_a": a,
        "runner_b": b,
        "favours": favours,
        "estimate": estimate,
        "estimate_unit": "times_as_long",
        "ci_95": ci,
        "p_value": sign_test(wins, losses),
        "tasks": len(shared),
        "a_better": wins,
        "b_better": losses,
        "tied": len(shared) - wins - losses,
        "tries_per_task": max(tries.values()),
    }


def _plural(count: int, word: str) -> str:
    return f"{count} {word}{'' if count == 1 else 's'}"


def _p_text(p: float) -> str:
    return "p < 0.001" if p < 0.001 else f"p = {p:.3f}"


def _split(draft: dict[str, Any]) -> str:
    a, b, wins, losses = draft["runner_a"], draft["runner_b"], draft["a_better"], draft["b_better"]
    better = "did better" if draft["metric"] == "accuracy" else "was faster"
    tied = f"; {draft['tied']} tied." if draft["tied"] else "."
    if wins and losses:
        return f"{a} {better} on {_plural(wins, 'task')} and {b} on {losses}{tied}"
    leader, count = (a, wins) if wins else (b, losses)
    if not draft["tied"]:
        return f"{leader} {better} on all {_plural(count, 'task')}."
    return f"{leader} {better} on {_plural(count, 'task')}{tied}"


def _explain(draft: dict[str, Any], verdict: str, p_adjusted: float) -> str:
    differing = draft["a_better"] + draft["b_better"]
    if differing == 0:
        tasks = _plural(draft["tasks"], "task")
        return f"{draft['runner_a']} and {draft['runner_b']} matched on all {tasks}."
    split, p_text = _split(draft), _p_text(p_adjusted)
    if verdict == "clearly_different":
        return f"{split} A split this one-sided is unlikely if they were equally good ({p_text})."
    if 2 * 0.5**differing >= ALPHA:
        return f"{split} Only {_plural(differing, 'task')} differed, too few to rule out chance."
    return f"{split} A split like this is common between equally good systems ({p_text})."


def paired_verdicts(
    results: Sequence[Mapping[str, Any]], models: Sequence[Mapping[str, Any]]
) -> list[Verdict]:
    """Accuracy and speed verdicts for every pair of systems on the same track and tasks."""
    order = [model["runner_id"] for model in models]
    track = {model["runner_id"]: model.get("evaluation_track", "unverified") for model in models}
    rows: dict[tuple[str, str], list[Mapping[str, Any]]] = defaultdict(list)
    for row in results:
        rows[(row.get("capability_track") or "text", row["runner_id"])].append(row)
    verdicts = []
    for capability in sorted({key[0] for key in rows}):
        present = [runner for runner in order if (capability, runner) in rows]
        pairs = [(a, b) for a, b in combinations(present, 2) if track.get(a) == track.get(b)]
        for build in (_accuracy, _speed):
            drafts = [
                draft
                for a, b in pairs
                if (draft := build(capability, a, b, rows[(capability, a)], rows[(capability, b)]))
            ]
            for draft, adjusted in zip(drafts, holm([d["p_value"] for d in drafts]), strict=True):
                outcome = "clearly_different" if adjusted < ALPHA else "too_close_to_call"
                verdicts.append(
                    Verdict(
                        **draft,
                        verdict=outcome,
                        p_value_adjusted=adjusted,
                        explanation=_explain(draft, outcome, adjusted),
                    )
                )
    return verdicts
