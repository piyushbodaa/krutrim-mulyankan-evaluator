"""Refresh checked-in integration schemas from their authoritative typed models."""

import json
from pathlib import Path

from kritrim_mulyankan.app_observations import AppObservation
from kritrim_mulyankan.comparisons import ComparisonBundle
from kritrim_mulyankan.export import ResultExport
from kritrim_mulyankan.importer import ImportDataset
from kritrim_mulyankan.jobs import JobRequest, JobState

root = Path(__file__).resolve().parents[1]
for filename, model in [
    ("result-schema.json", ResultExport),
    ("dataset-schema.json", ImportDataset),
    ("app-observation-schema.json", AppObservation),
    ("comparison-schema.json", ComparisonBundle),
    ("job-request-schema.json", JobRequest),
    ("job-state-schema.json", JobState),
]:
    (root / "docs" / filename).write_text(json.dumps(model.model_json_schema(), indent=2) + "\n")
