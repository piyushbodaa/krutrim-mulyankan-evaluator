"""Recompute the static preview's verdicts with the evaluator's own statistics.

The hosted preview has no backend, so preview/preview-fixtures.js carries a results payload in
the export format. Run this after changing that payload or kritrim_mulyankan.verdicts:

    uv run python scripts/refresh_preview_verdicts.py
"""

import json
from pathlib import Path

from kritrim_mulyankan.verdicts import paired_verdicts

path = Path(__file__).resolve().parents[1] / "preview" / "preview-fixtures.js"
source = path.read_text(encoding="utf-8")
start = source.index("{")
fixtures, length = json.JSONDecoder().raw_decode(source[start:])
results = fixtures["results"]
results["schema_version"] = "1.1"
results["run"]["repeats"] = max(row.get("repeat_index", 0) for row in results["results"]) + 1
for row in results["results"]:
    row.setdefault("repeat_index", 0)
verdicts = paired_verdicts(results["results"], results["models"])
results["verdicts"] = [verdict.model_dump(mode="json") for verdict in verdicts]
rebuilt = source[:start] + json.dumps(fixtures, ensure_ascii=False) + source[start + length :]
path.write_text(rebuilt, encoding="utf-8")
print(f"{len(verdicts)} verdicts written to {path.name}")
