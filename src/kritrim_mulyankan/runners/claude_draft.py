"""A bounded, tool-free installed CLI draft. Uses existing subscription auth only."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import signal
import subprocess
import time
from contextlib import suppress
from datetime import UTC, datetime
from functools import lru_cache
from pathlib import Path

from kritrim_mulyankan.provenance import redact
from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage
from kritrim_mulyankan.suite import Task


def cli_environment() -> dict[str, str]:
    # Do not pass API credentials, provider switches, paid-route configs or user prompt hooks.
    allowed = {
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
    }
    return {key: value for key, value in os.environ.items() if key in allowed}


@lru_cache(maxsize=8)
def cli_version(executable: str) -> str | None:
    try:
        output = subprocess.run(
            [executable, "--version"],
            env=cli_environment(),
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        )
        return output.stdout.strip()[:100] or None
    except (OSError, subprocess.SubprocessError):
        return None


class ClaudeDraftRunner(Runner):
    kind = "harness"

    def __init__(self, access_proof: Path, *, executable: str | None = None):
        super().__init__("harness:claude-draft")
        self.access_proof = access_proof
        self.executable = executable or shutil.which("claude")

    def problems(self) -> list[str]:
        if not self.executable:
            return ["Installed Claude CLI not found"]
        try:
            proof = json.loads(self.access_proof.read_text())
            valid = (
                isinstance(proof, dict)
                and proof.get("usage_credits_enabled") is False
                and proof.get("auth_method") == "claude.ai"
                and proof.get("operator_verified") is True
                and datetime.fromisoformat(proof["expires_at"]) > datetime.now(UTC)
            )
        except (OSError, ValueError, KeyError, TypeError):
            valid = False
        return [] if valid else ["Fresh included-plan/usage-credits-off verification is required"]

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        # No paid API calls. Subscription fees and actual marginal charge stay unknown.
        return 0

    def configuration(self) -> dict:
        return {
            **super().configuration(),
            "framework": "Claude Code CLI",
            "framework_version": cli_version(self.executable) if self.executable else None,
            "options": {
                "tools": [],
                "mcp": [],
                "safe_mode": True,
                "restricted": True,
                "session_persistence": False,
                "api_key_environment": False,
            },
            "endpoint": None,
            "seed_supported": False,
            "cost_scope": "Existing included subscription; actual charge unknown",
            "access_proof_sha256": hashlib.sha256(self.access_proof.read_bytes()).hexdigest()
            if self.access_proof.exists()
            else None,
        }

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        if issues := self.problems():
            raise RuntimeError("; ".join(issues))
        if task.capability_track != "text" or task.fixtures:
            raise ValueError("Claude draft supports frozen synthetic text tasks only")
        start = time.monotonic()
        auth = subprocess.run(
            [self.executable, "auth", "status", "--json"],
            env=cli_environment(),
            cwd=ctx.workdir,
            capture_output=True,
            text=True,
            timeout=10,
        )
        try:
            state = json.loads(auth.stdout)
        except ValueError:
            raise RuntimeError("CLI authentication status unavailable") from None
        if not (
            state.get("loggedIn")
            and state.get("authMethod") == "claude.ai"
            and state.get("apiProvider") == "firstParty"
        ):
            raise RuntimeError("CLI is not using verified subscription authentication")
        args = [
            self.executable,
            "--print",
            "--output-format",
            "json",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            '{"mcpServers":{}}',
            "--safe-mode",
            "--restricted",
            "--disable-slash-commands",
            "--no-chrome",
            "--no-session-persistence",
            "--setting-sources",
            "",
        ]
        process = subprocess.Popen(
            args,
            cwd=ctx.workdir,
            env=cli_environment(),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        reason = None
        try:
            first = True
            while True:
                if ctx.cancel_event and ctx.cancel_event.is_set():
                    reason = "Cancelled; CLI process stopped. Billing not instrumented."
                    break
                if time.monotonic() - start >= min(ctx.timeout_s, 90):
                    reason = "CLI timed out; process stopped. Billing not instrumented."
                    break
                try:
                    stdout, stderr = process.communicate(
                        input=prompt if first else None, timeout=0.2
                    )
                    break
                except subprocess.TimeoutExpired:
                    first = False
            if reason:
                with suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    stdout, stderr = process.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    stdout, stderr = process.communicate(timeout=3)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
        raw = redact(
            {
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": process.returncode,
                "billing": "included-plan check; token-price estimates are not actual charges",
            }
        )
        try:
            response = json.loads(stdout)
            raw["response"] = redact(response)
        except ValueError:
            return RunOutput(error=reason or "CLI returned no valid JSON envelope", raw=raw)
        if not isinstance(response, dict) or not isinstance(response.get("result", ""), str):
            return RunOutput(error="CLI envelope has unsupported result shape", raw=raw)
        usage = response.get("usage") or {}
        model_usage = response.get("modelUsage") or {}
        if not isinstance(usage, dict) or not isinstance(model_usage, dict):
            return RunOutput(error="CLI envelope has unsupported usage shape", raw=raw)
        models = list(model_usage.keys())
        raw["resolved_model"] = models[0] if len(models) == 1 else None
        raw["reported_token_price_usd"] = response.get("total_cost_usd")
        error = reason or (
            "CLI execution failed; inspect private envelope"
            if process.returncode or response.get("is_error")
            else None
        )
        return RunOutput(
            output=response.get("result") or "",
            error=error,
            usage=Usage(
                input_tokens=usage.get("input_tokens"),
                output_tokens=usage.get("output_tokens"),
                cost_usd=None,
            ),
            tool_calls=0,
            raw=raw,
        )
