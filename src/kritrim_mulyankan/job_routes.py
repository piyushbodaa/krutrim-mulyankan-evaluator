"""Versioned HTTP surface; all writes retain the loopback workbench's request guard."""

from __future__ import annotations

import re
from http import HTTPStatus
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field

from kritrim_mulyankan.jobs import JobRequest, JobState


class JobAction(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    api_version: Literal["1.0"] = "1.0"
    idempotency_key: str | None = Field(None, pattern=r"^[A-Za-z0-9_-]{8,100}$")


def get_route(service, path: str):
    manager = service.jobs
    if path == "/api/v1":
        return {
            "api_version": "1.0",
            "capabilities": "/api/v1/capabilities",
            "tasks": "/api/v1/tasks",
            "jobs": "/api/v1/jobs",
            "request_schema": "/api/v1/schemas/job-request",
            "state_schema": "/api/v1/schemas/job-state",
            "auth": "Loopback Host; same-origin writes; X-Pilot-Token from page meta pilot-token",
            "privacy": "Results endpoint includes private output; export omits raw output",
        }
    if path == "/api/v1/capabilities":
        return manager.capabilities()
    if path == "/api/v1/tasks":
        return {"api_version": "1.0", "tasks": service.tasks()}
    if path == "/api/v1/jobs":
        return {"api_version": "1.0", "jobs": manager.list()}
    if path == "/api/v1/schemas/job-request":
        return JobRequest.model_json_schema()
    if path == "/api/v1/schemas/job-state":
        return JobState.model_json_schema()
    match = re.fullmatch(r"/api/v1/jobs/([a-f0-9]{32})(/results|/export)?", path)
    if match:
        return (
            manager.result(match[1], outputs=match[2] == "/results")
            if match[2]
            else manager.get(match[1])
        )
    raise ValueError("unknown API route")


def post_route(service, path: str, data: dict):
    manager = service.jobs
    if path == "/api/v1/jobs/preview":
        preview, _ = manager.preview(JobRequest.model_validate(data))
        return preview, HTTPStatus.OK
    if path == "/api/v1/jobs":
        return manager.create(JobRequest.model_validate(data)), HTTPStatus.ACCEPTED
    match = re.fullmatch(r"/api/v1/jobs/([a-f0-9]{32})/(cancel|retry)", path)
    if match:
        action = JobAction.model_validate(data)
        if match[2] == "cancel":
            return manager.cancel(match[1]), HTTPStatus.OK
        if action.idempotency_key is None:
            raise ValueError("Retry requires a new idempotency key")
        return manager.retry(match[1], action.idempotency_key), HTTPStatus.ACCEPTED
    raise ValueError("unknown API route")
