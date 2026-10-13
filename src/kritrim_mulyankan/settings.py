"""Runtime settings, read from environment variables (a .env file is loaded by the CLI)."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field


class Settings(BaseModel):
    model_config = ConfigDict(allow_inf_nan=False)
    runs_dir: Path = Path("runs")
    db_path: Path = Path("runs/results.db")
    suites_dir: Path = Path("suites")
    prices_path: Path = Field(Path("prices.yaml"), description="Optional price overrides.")
    max_usd: float = Field(1.0, ge=0, description="Hard budget cap per run, in USD.")
    usd_to_inr: float = Field(88.0, gt=0, description="FX rate for reporting costs in INR.")

    @classmethod
    def from_env(cls, env: Mapping[str, str] | None = None) -> Settings:
        env = os.environ if env is None else env
        runs_dir = Path(env.get("MJ_RUNS_DIR", "runs"))
        return cls(
            runs_dir=runs_dir,
            db_path=Path(env.get("MJ_DB", str(runs_dir / "results.db"))),
            suites_dir=Path(env.get("MJ_SUITES_DIR", "suites")),
            prices_path=Path(env.get("MJ_PRICES", "prices.yaml")),
            max_usd=float(env.get("MJ_MAX_USD", "1.0")),
            usd_to_inr=float(env.get("MJ_USD_INR", "88.0")),
        )
