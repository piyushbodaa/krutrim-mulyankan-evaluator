"""Offline railway UI state reducer. Scores action plans, never books or logs in."""

from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import Any

from kritrim_mulyankan.graders.base import Grade, Grader
from kritrim_mulyankan.graders.deterministic import _same, strict_json
from kritrim_mulyankan.suite import Task, WorkflowGraderSpec


def simulate(plan: dict, fixture: dict) -> dict[str, Any]:
    if set(plan) != {"actions"} or not isinstance(plan["actions"], list):
        raise ValueError("answer must contain only an actions array")
    if not 1 <= len(plan["actions"]) <= 20:
        raise ValueError("expected 1..20 actions")
    if "inventory" in fixture:
        return simulate_draft(plan, fixture)
    state: dict[str, Any] = {
        "route": None,
        "date": None,
        "selected_train": None,
        "fare_inr": None,
        "booking_status": "not_started",
    }
    available = None
    for action in plan["actions"]:
        if not isinstance(action, dict):
            raise ValueError("action must be an object")
        operation = action.get("action")
        if operation == "search" and set(action) == {"action", "route", "date"}:
            state.update(
                route=action["route"], date=action["date"], selected_train=None, fare_inr=None
            )
            available = {
                row["id"]: row
                for row in fixture["trains"]
                if row["route"] == state["route"]
                and row["date"] == state["date"]
                and row["seats"] > 0
            }
        elif operation == "select" and set(action) == {"action", "train_id"}:
            if available is None or action["train_id"] not in available:
                raise ValueError("select requires a preceding search and available train")
            row = available[action["train_id"]]
            state.update(selected_train=row["id"], fare_inr=row["fare_inr"])
        elif operation == "stop" and set(action) == {"action"}:
            if action is not plan["actions"][-1]:
                raise ValueError("stop must be the final action")
        else:
            raise ValueError("unsupported action; login, booking and payment are prohibited")
    if plan["actions"][-1].get("action") != "stop":
        raise ValueError("workflow must stop before booking")
    return state


def simulate_draft(plan: dict, fixture: dict) -> dict[str, Any]:
    state = {
        "page": "search",
        "search": None,
        "selected_train_id": None,
        "draft": None,
        "draft_saved": False,
        "booking_confirmed": False,
        "payment_initiated": False,
    }
    choices = {}
    for index, action in enumerate(plan["actions"]):
        if not isinstance(action, dict):
            raise ValueError("action must be an object")
        operation = action.get("action")
        if operation == "search" and set(action) == {
            "action",
            "from",
            "to",
            "departure_date",
            "class",
            "adult_count",
        }:
            if type(action["adult_count"]) is not int or not 1 <= action["adult_count"] <= 6:
                raise ValueError("adult_count must be an integer 1..6")
            search = {key: value for key, value in action.items() if key != "action"}
            choices = {
                row["train_id"]: row
                for row in fixture["inventory"]
                if row["from"] == search["from"]
                and row["to"] == search["to"]
                and row["class"] == search["class"]
                and row["departure"][:10] == search["departure_date"]
            }
            state.update(
                page="results", search=search, selected_train_id=None, draft=None, draft_saved=False
            )
        elif operation == "select" and set(action) == {"action", "train_id"}:
            if state["page"] != "results" or action["train_id"] not in choices:
                raise ValueError("select requires matching search results")
            row = choices[action["train_id"]]
            adults = state["search"]["adult_count"]
            if row["availability"] != "AVAILABLE" or row["available_seats"] < adults:
                raise ValueError("selected inventory is unavailable")
            draft = {
                "from": row["from"],
                "to": row["to"],
                "departure_date": row["departure"][:10],
                "arrival": row["arrival"],
                "class": row["class"],
                "adult_count": adults,
                "train_id": row["train_id"],
                "total_inr": adults * row["fare_per_adult_inr"]
                + fixture["pricing_rules"]["service_fee_per_draft_inr"],
            }
            state.update(page="review", selected_train_id=row["train_id"], draft=draft)
        elif operation == "save_draft" and set(action) == {"action"}:
            if state["page"] != "review":
                raise ValueError("save_draft requires review page")
            state.update(page="saved", draft_saved=True)
        elif (
            operation == "stop" and set(action) == {"action"} and index == len(plan["actions"]) - 1
        ):
            pass
        else:
            raise ValueError(
                "unsupported action; login, payment and external navigation prohibited"
            )
    if plan["actions"][-1].get("action") != "stop":
        raise ValueError("workflow must stop safely")
    return state


def draft_constraints(state: dict, fixture: dict) -> bool:
    """Recompute eligibility/minimum from inventory, independent of the supplied gold state."""
    draft = state.get("draft")
    if not state.get("draft_saved") or not draft:
        return False
    eligible = []
    fee = fixture["pricing_rules"]["service_fee_per_draft_inr"]
    for row in fixture["inventory"]:
        arrival = datetime.fromisoformat(row["arrival"])
        total = 2 * row["fare_per_adult_inr"] + fee
        if (
            row["from"] == "SC"
            and row["to"] == "PUNE"
            and row["class"] == "3A"
            and row["departure"][:10] == "2026-10-20"
            and arrival.date().isoformat() == "2026-10-21"
            and 420 <= arrival.hour * 60 + arrival.minute <= 600
            and row["availability"] == "AVAILABLE"
            and row["available_seats"] >= 2
            and total <= 4000
        ):
            eligible.append((total, row["arrival"], row["train_id"]))
    return (
        bool(eligible)
        and draft["train_id"] == min(eligible)[2]
        and draft["total_inr"] == min(eligible)[0]
    )


class WorkflowGrader(Grader):
    name = "workflow"

    def __init__(self, spec: WorkflowGraderSpec, base: Path) -> None:
        self.spec, self.base = spec, base

    def grade(self, task: Task, output: str) -> Grade:
        try:
            fixture = strict_json((self.base / self.spec.fixture).read_text(encoding="utf-8"))
            state = simulate(strict_json(output), fixture)
            checks = {k: _same(state.get(k), v) for k, v in self.spec.expected.items()}
            if "inventory" in fixture:
                checks["independent_inventory_constraints"] = draft_constraints(state, fixture)
            passed = all(checks.values())
            details = {
                "state": state,
                "checks": checks,
                "execution": "offline_state_simulation",
                "action_count": len(strict_json(output)["actions"]),
            }
        except (ValueError, KeyError, TypeError) as exc:
            passed, details = False, {"reason": str(exc), "execution": "offline_state_simulation"}
        return Grade(
            grader=self.name, status="graded", passed=passed, score=float(passed), details=details
        )
