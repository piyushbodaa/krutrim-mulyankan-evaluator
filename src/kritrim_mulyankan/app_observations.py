"""Record a response observed in a consumer app. Never drive accounts or call a provider."""

from __future__ import annotations

import hashlib
import json
from datetime import datetime
from decimal import Decimal
from typing import Literal
from urllib.parse import urlsplit

from pydantic import BaseModel, ConfigDict, Field, model_validator

from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage
from kritrim_mulyankan.suite import Task


class AppContext(BaseModel):
    model_config = ConfigDict(extra="forbid", allow_inf_nan=False)
    app: str = Field(min_length=1, max_length=100)
    app_url: str | None = Field(None, max_length=500)
    client: str = Field(min_length=1, max_length=100)
    plan: str = Field(min_length=1, max_length=100)
    client_version: str | None = Field(None, max_length=100)
    displayed_model: str | None = Field(None, max_length=100)
    session: Literal["fresh", "existing", "unknown"] = "unknown"
    context_notes: str = Field("", max_length=4000)
    observed_at: datetime | None = None
    observed_latency_s: float | None = Field(None, ge=0)
    subscription_amount: Decimal | None = Field(None, ge=0)
    subscription_currency: str | None = Field(None, pattern=r"^[A-Z]{3}$")
    subscription_period: Literal["month", "year", "other"] | None = None
    subscription_price_source: str | None = Field(None, max_length=500)
    observation_kind: Literal["contributor_reported", "test_fixture"] = "contributor_reported"

    @model_validator(mode="after")
    def _time(self) -> AppContext:
        if self.app_url:
            url = urlsplit(self.app_url)
            if (
                url.scheme not in {"https", "http"}
                or not url.hostname
                or url.username
                or url.password
                or url.query
                or url.fragment
            ):
                raise ValueError(
                    "app_url must be an HTTP(S) app URL without credentials/query tokens"
                )
        if self.observed_at and self.observed_at.utcoffset() is None:
            raise ValueError("observed_at must include a timezone")
        if self.subscription_amount is not None and not (
            self.subscription_currency
            and self.subscription_period
            and self.subscription_price_source
        ):
            raise ValueError("subscription amount needs currency, period and source")
        return self


class AppObservation(BaseModel):
    model_config = ConfigDict(extra="forbid")
    schema_version: Literal["1.0"] = "1.0"
    suite: Literal["india-pilot"] = "india-pilot"
    task_id: str
    prompt_sha256: str = Field(pattern=r"^[a-f0-9]{64}$")
    app_context: AppContext
    response: str = Field(max_length=30000)
    evidence_note: str = Field(min_length=8, max_length=4000)
    evidence_references: list[str] = Field(default_factory=list, max_length=10)
    configuration_name: str | None = Field(None, max_length=120)
    configuration_registry_id: str | None = Field(None, pattern=r"^[A-Za-z0-9._-]{1,80}$")
    completion_status: Literal["completed", "partial", "not_completed", "unknown"] = "unknown"
    observed_cost_amount: Decimal | None = Field(None, ge=0)
    observed_cost_currency: str | None = Field(None, pattern=r"^[A-Z]{3}$")
    observed_cost_source: str | None = Field(None, max_length=500)
    exact_prompt_used: bool
    attested: bool = False

    @model_validator(mode="after")
    def _attestation(self) -> AppObservation:
        if not self.response.strip() and self.completion_status != "not_completed":
            raise ValueError("provide a response or explicitly record not_completed")
        if self.observed_cost_amount is not None and not (
            self.observed_cost_currency and self.observed_cost_source
        ):
            raise ValueError("reported per-task cost needs currency and source")
        if self.app_context.observation_kind == "contributor_reported" and not self.attested:
            raise ValueError("contributor must attest that the response was observed in the app")
        return self


class AppObservationRunner(Runner):
    """A manual evidence import, not a live agent or API evaluation."""

    kind = "app"

    def __init__(self, observation: AppObservation) -> None:
        self.observation = observation
        context = json.dumps(
            {
                "context": observation.app_context.model_dump(mode="json"),
                "registry_id": observation.configuration_registry_id,
                "name": observation.configuration_name,
            },
            sort_keys=True,
        )
        identifier = hashlib.sha256(context.encode()).hexdigest()[:16]
        super().__init__(f"app-observation:{identifier}")

    def configuration(self) -> dict:
        return {
            **super().configuration(),
            "evaluation_mode": "manual_app",
            "evaluation_track": "consumer_app",
            "app_context": self.observation.app_context.model_dump(mode="json"),
            "verification_status": self.observation.app_context.observation_kind,
            "configuration_name": self.observation.configuration_name,
            "configuration_registry_id": self.observation.configuration_registry_id,
            "exact_prompt_used": self.observation.exact_prompt_used,
            "cost_scope": "Recorded app usage; per-task charge unknown. "
            "Subscription recorded separately.",
        }

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        return 0  # Importing an already observed response makes no inference call.

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        if (
            task.id != self.observation.task_id
            or hashlib.sha256(prompt.encode()).hexdigest() != self.observation.prompt_sha256
        ):
            raise ValueError("recorded task/prompt does not match the selected frozen task")
        return RunOutput(
            output=self.observation.response,
            usage=Usage(cost_usd=None),
            raw={
                "observation": self.observation.model_dump(mode="json"),
                "import_api_charge_usd": 0,
                "response_sha256": hashlib.sha256(self.observation.response.encode()).hexdigest(),
            },
        )
