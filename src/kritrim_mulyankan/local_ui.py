"""Loopback pilot workbench: manual app observations and fixed free mock controls."""

from __future__ import annotations

import hashlib
import json
import re
import secrets
import threading
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Literal
from urllib.parse import urlsplit

from pydantic import BaseModel, ConfigDict, ValidationError

from kritrim_mulyankan.app_observations import AppObservation, AppObservationRunner
from kritrim_mulyankan.comparisons import ComparisonBundle, ConfigurationRegistry, NewConfiguration
from kritrim_mulyankan.engine import run_suite
from kritrim_mulyankan.evidence_archive import evidence_detail, list_evidence
from kritrim_mulyankan.export import explanation_key, result_export, write_export
from kritrim_mulyankan.graders.deterministic import strict_json
from kritrim_mulyankan.job_routes import get_route, post_route
from kritrim_mulyankan.jobs import JobManager
from kritrim_mulyankan.runners.fake import EchoRunner, OracleRunner
from kritrim_mulyankan.settings import Settings
from kritrim_mulyankan.store import Store
from kritrim_mulyankan.suite import load_suite

CATEGORIES = {
    "rupee_numeracy": "Money & numbers",
    "india_scheduling": "Dates & schedules",
    "record_reconciliation": "Office records",
    "hindi_hinglish_instructions": "Hindi & Hinglish",
    "evidence_and_freshness": "Checking evidence",
    "robustness_and_abstention": "Avoiding mistakes",
    "tool-use": "Travel planning",
}
TITLES = {
    "NUM01": "Add up an office budget",
    "NUM02": "Check an invoice total",
    "NUM03": "Calculate a balance in Hindi",
    "NUM04": "Choose a supplier within budget",
    "DATE01": "Convert an Indian meeting time to UTC",
    "DATE02": "Schedule a meeting in India",
    "DATE03": "Find a payment due date",
    "DATE04": "Spot an unclear date",
    "DOC01": "Remove duplicate invoices",
    "DOC02": "Reconcile an office delivery",
    "DOC03": "Read a Hindi meeting note",
    "DOC04": "Compare two supplier quotes",
    "LANG01": "Prioritise open tickets in Hindi",
    "LANG02": "Read a Hinglish deadline",
    "LANG03": "Check which files can be sent",
    "LANG04": "Follow the latest dispatch update",
    "EVID01": "Find the approved budget",
    "EVID02": "Check a time-zone reference",
    "EVID03": "Know when the evidence is missing",
    "EVID04": "Compare forecast and actual spend",
    "ROB01": "Ignore instructions hidden in an invoice",
    "ROB02": "Summarise without personal details",
    "ROB03": "Ask for missing information",
    "ROB04": "Spot an unreadable amount",
    "RAIL01": "Save a railway itinerary draft",
}
# Fixed allow-list of files under ui_assets/; the workbench never maps a URL onto an arbitrary path.
# brand/ holds the approved Chakra Engine kit files, unmodified, plus its fonts converted to WOFF2.
STATIC_ASSETS: dict[str, tuple[str, str]] = {
    "/": ("index.html", "text/html; charset=utf-8"),
    "/app.js": ("app.js", "text/javascript; charset=utf-8"),
    "/showcase.js": ("showcase.js", "text/javascript; charset=utf-8"),
    "/compare.js": ("compare.js", "text/javascript; charset=utf-8"),
    "/jobs.js": ("jobs.js", "text/javascript; charset=utf-8"),
    "/theme.js": ("theme.js", "text/javascript; charset=utf-8"),
    "/style.css": ("style.css", "text/css; charset=utf-8"),
    "/favicon.ico": ("brand/favicon.ico", "image/x-icon"),
    "/brand/favicon.ico": ("brand/favicon.ico", "image/x-icon"),
    "/brand/favicon-16.png": ("brand/favicon-16.png", "image/png"),
    "/brand/favicon-32.png": ("brand/favicon-32.png", "image/png"),
    "/brand/favicon-180.png": ("brand/favicon-180.png", "image/png"),
    "/brand/km-chakra-wordmark.svg": ("brand/km-chakra-wordmark.svg", "image/svg+xml"),
    "/brand/km-chakra-wordmark-white.svg": ("brand/km-chakra-wordmark-white.svg", "image/svg+xml"),
    "/brand/fonts/Sora.woff2": ("brand/fonts/Sora.woff2", "font/woff2"),
    "/brand/fonts/NotoSansDevanagari-Regular.woff2": (
        "brand/fonts/NotoSansDevanagari-Regular.woff2",
        "font/woff2",
    ),
}


class ValidationRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    suite: Literal["india-pilot", "railway-pilot"]
    task_id: str
    control: Literal["both", "oracle", "echo"] = "both"


class PilotService:
    def __init__(self, project: Path, settings: Settings | None = None) -> None:
        self.project = project.resolve()
        self.settings = settings or Settings(
            suites_dir=self.project / "suites",
            runs_dir=self.project / "runs",
            db_path=self.project / "runs/results.db",
            max_usd=0,
        )
        self.suites = {
            name: load_suite(self.settings.suites_dir / f"{name}.yaml")
            for name in ("india-pilot", "railway-pilot")
        }
        self.lock = threading.Lock()
        self.registry = ConfigurationRegistry(self.settings.runs_dir / "configurations.json")
        self.jobs = JobManager(self.suites, self.settings)

    def tasks(self) -> list[dict]:
        result = []
        for name, suite in self.suites.items():
            for item in suite.tasks:
                spec = item.grader
                expected = spec.expected if spec.type in {"json", "workflow"} else None
                instruction = item.prompt.split(
                    "Use only the supplied context; do not browse or take external actions."
                )[-1].strip()
                if item.id == "NUM01":
                    instruction = (
                        "Work out the office budget after the one-time discount. "
                        "Give the total amount and write it using Indian comma grouping."
                    )
                result.append(
                    {
                        "id": item.id,
                        "suite": name,
                        "title": TITLES.get(item.id, item.id),
                        "category": CATEGORIES.get(item.category, item.category),
                        "language": {"en-IN": "English", "hi": "Hindi", "hi-Latn": "Hinglish"}.get(
                            item.language, item.language
                        ),
                        "track": item.capability_track,
                        "instruction": instruction,
                        "context": item.context
                        or "The fictional train inventory is in the exact task prompt below.",
                        "full_prompt": item.render_prompt(),
                        "prompt_sha256": hashlib.sha256(item.render_prompt().encode()).hexdigest(),
                        "task_sha256": item.content_hash(),
                        "task_version": item.version,
                        "expected": expected,
                        "grading": "Every answer field must match; no missing or extra fields."
                        if spec.type == "json"
                        else "Save the cheapest eligible draft. No booking, payment or login.",
                    }
                )
        return result

    def proposals(self) -> dict:
        path = (
            self.project
            / "research/consumer100-proposals/krutrim_mulyankan_consumer100/consumer_100.json"
        )
        if not path.is_file():
            return {"status": "unavailable", "items": []}
        data = json.loads(path.read_text())
        # Browse design seeds only. Gold/adversarial fixtures are not candidate inputs.
        fields = {
            "id",
            "title",
            "category",
            "service_provider",
            "user_prompt",
            "safe_execution_boundary",
        }
        return {
            "status": "proposed_not_executable",
            "items": [
                {key: value for key, value in item.items() if key in fields}
                for item in data["items"]
            ],
        }

    def runs(self) -> list[dict]:
        if not self.settings.db_path.exists():
            return []
        with Store(self.settings.db_path) as store:
            runs = [
                r
                for r in store.list_runs(limit=50)
                if r.suite_name in self.suites
                and (
                    set(r.runners) <= {"echo", "oracle"}
                    or all(name.startswith("app-observation:") for name in r.runners)
                )
            ]
            records = [
                {
                    "id": r.id,
                    "suite": r.suite_name,
                    "started_at": r.started_at,
                    "status": r.status,
                    "cost_usd": None if r.uncertain_cost else r.spent_usd,
                    "task_count": len({x.task_id for x in store.results_for_run(r.id)}),
                    "controls": r.runners,
                    "mode": "manual_app"
                    if all(n.startswith("app-observation:") for n in r.runners)
                    else "mock",
                }
                for r in runs
            ]
            for record, run in zip(records, runs, strict=True):
                if record["mode"] == "manual_app":
                    manifest_path = Path(run.artifacts_dir) / "manifest.json"
                    manifest = (
                        json.loads(manifest_path.read_text()) if manifest_path.is_file() else {}
                    )
                    configuration = manifest.get("runners", [{}])[0]
                    record["verification_status"] = configuration.get(
                        "verification_status", "unverified"
                    )
                    record["app_context"] = configuration.get("app_context")
            return records

    def run_detail(self, run_id: str, *, outputs: bool = True) -> dict:
        if not re.fullmatch(r"[0-9]{8}-[0-9]{6}(?:-[0-9]+)?", run_id):
            raise ValueError("unknown run")
        with Store(self.settings.db_path) as store:
            run = store.get_run(run_id)
            if (
                not run
                or run.suite_name not in self.suites
                or not (
                    set(run.runners) <= {"echo", "oracle"}
                    or all(name.startswith("app-observation:") for name in run.runners)
                )
            ):
                raise ValueError("only local mock/app-observation pilot results are available here")
            payload = result_export(store, run_id, include_outputs=outputs).model_dump(mode="json")
            details = {
                explanation_key(record): record.grade_details
                for record in store.results_for_run(run_id)
            }
        if outputs:
            payload["explanations"] = details
            comparison_path = Path(run.artifacts_dir) / "comparison.json"
            if comparison_path.is_file():
                payload["comparison_bundle"] = json.loads(comparison_path.read_text())
        return payload

    def validate(self, request: ValidationRequest) -> dict:
        suite = self.suites[request.suite]
        suite.select([request.task_id])  # Reject unknown tasks before acquiring the run lock.
        if not self.lock.acquire(blocking=False):
            raise RuntimeError("A check is already running. Please try again shortly.")
        try:
            runners = {
                "oracle": [OracleRunner()],
                "echo": [EchoRunner()],
                "both": [OracleRunner(), EchoRunner()],
            }[request.control]
            with Store(self.settings.db_path) as store:
                summary = run_suite(
                    suite,
                    runners,
                    store=store,
                    settings=self.settings,
                    task_ids=[request.task_id],
                    max_usd=0,
                    seed=12,
                )
                write_export(store, summary.run_id, summary.run_dir / "results.json")
            return self.run_detail(summary.run_id)
        finally:
            self.lock.release()

    def record_observation(self, request: AppObservation) -> dict:
        suite = self.suites[request.suite]
        item = suite.select([request.task_id])[0]
        if item.capability_track != "text":
            raise ValueError("manual text responses cannot establish browser-workflow success")
        expected_hash = hashlib.sha256(item.render_prompt().encode()).hexdigest()
        if request.prompt_sha256 != expected_hash:
            raise ValueError("the submitted prompt does not match this frozen task")
        if not self.lock.acquire(blocking=False):
            raise RuntimeError("A check is already running. Please try again shortly.")
        try:
            with Store(self.settings.db_path) as store:
                summary = run_suite(
                    suite,
                    [AppObservationRunner(request)],
                    store=store,
                    settings=self.settings,
                    task_ids=[request.task_id],
                    max_usd=0,
                    seed=12,
                )
                write_export(store, summary.run_id, summary.run_dir / "results.json")
            return self.run_detail(summary.run_id)
        finally:
            self.lock.release()

    def configurations(self) -> list[dict]:
        status = {}
        if self.settings.db_path.is_file():
            with Store(self.settings.db_path) as store:
                for run in store.list_runs(limit=200):
                    manifest_path = Path(run.artifacts_dir) / "manifest.json"
                    if not manifest_path.is_file():
                        continue
                    for config in json.loads(manifest_path.read_text()).get("runners", []):
                        registry_id = config.get("configuration_registry_id")
                        if registry_id:
                            current = (
                                "sample_only"
                                if config.get("verification_status") == "test_fixture"
                                else "recorded_unverified"
                            )
                            if status.get(registry_id) != "recorded_unverified":
                                status[registry_id] = current
        return [
            {
                "configuration": config.model_dump(mode="json"),
                "status": status.get(config.id, "unrun"),
            }
            for config in self.registry.list()
        ]

    def compare(self, bundle: ComparisonBundle, *, imported: bool = False) -> dict:
        suite = self.suites[bundle.suite]
        item = suite.select([bundle.task_id])[0]
        if item.content_hash() != bundle.task_sha256 or item.version != bundle.task_version:
            raise ValueError("task definition/version changed; use the same frozen task")
        if hashlib.sha256(item.render_prompt().encode()).hexdigest() != bundle.prompt_sha256:
            raise ValueError("prompt hash differs from the selected task")
        observations = [entry.observation(bundle) for entry in bundle.entries]
        if not self.lock.acquire(blocking=False):
            raise RuntimeError("A check is already running. Please try again shortly.")
        try:
            self.registry.ensure(
                [entry.configuration for entry in bundle.entries], allow_new=imported
            )
            with Store(self.settings.db_path) as store:
                summary = run_suite(
                    suite,
                    [AppObservationRunner(obs) for obs in observations],
                    store=store,
                    settings=self.settings,
                    task_ids=[item.id],
                    seed=12,
                    max_usd=0,
                )
                captured = bundle.model_copy(update={"recorded_run_id": summary.run_id})
                (summary.run_dir / "comparison.json").write_text(
                    captured.model_dump_json(indent=2) + "\n"
                )
                write_export(store, summary.run_id, summary.run_dir / "results.json")
            payload = self.run_detail(summary.run_id)
            payload["comparison_bundle"] = captured.model_dump(mode="json")
            return payload
        finally:
            self.lock.release()

    def import_comparison(self, document: str) -> dict:
        data = strict_json(document, decimal_numbers=False)
        bundle = ComparisonBundle.model_validate(data)
        return self.compare(bundle, imported=True)


def create_server(
    project: Path, port: int = 8765, *, service: PilotService | None = None
) -> ThreadingHTTPServer:
    service = service or PilotService(project)
    token = secrets.token_urlsafe(
        32
    )  # Ephemeral same-origin request token, not provider credentials.
    assets = Path(__file__).parent / "ui_assets"

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            pass  # Do not log bodies or task content.

        def respond(self, payload: object, status: HTTPStatus = HTTPStatus.OK) -> None:
            body = json.dumps(payload, ensure_ascii=False).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def local_host(self) -> bool:
            return self.headers.get("Host") in {
                f"127.0.0.1:{self.server.server_port}",
                f"localhost:{self.server.server_port}",
            }

        def do_GET(self) -> None:
            if not self.local_host():
                self.respond({"error": "This workbench is local only."}, HTTPStatus.FORBIDDEN)
                return
            path = urlsplit(self.path).path
            if path == "/api/v1" or path.startswith("/api/v1/"):
                try:
                    self.respond(get_route(service, path))
                except ValueError as exc:
                    self.respond({"error": str(exc)[:220]}, HTTPStatus.NOT_FOUND)
                except Exception:
                    self.respond(
                        {"error": "Local data unavailable; retry shortly."},
                        HTTPStatus.SERVICE_UNAVAILABLE,
                    )
                return
            if path in {"/overview.html", "/index.html"}:
                self.send_response(HTTPStatus.FOUND)
                self.send_header("Location", "/")
                self.end_headers()
                return
            if path == "/api/tasks":
                self.respond({"tasks": service.tasks()})
                return
            if path == "/api/proposals":
                self.respond(service.proposals())
                return
            if path == "/api/configurations":
                self.respond({"configurations": service.configurations()})
                return
            if path == "/api/evidence":
                self.respond({"evidence": list_evidence(service.settings.runs_dir / "evidence")})
                return
            evidence_match = re.fullmatch(r"/api/evidence/([a-f0-9]{64})", path)
            if evidence_match:
                try:
                    self.respond(
                        evidence_detail(service.settings.runs_dir / "evidence", evidence_match[1])
                    )
                except (ValueError, OSError):
                    self.respond({"error": "evidence not found"}, HTTPStatus.NOT_FOUND)
                return
            if path == "/api/runs":
                self.respond({"runs": service.runs()})
                return
            match = re.fullmatch(r"/api/runs/([0-9-]+)(/export)?", path)
            if match:
                try:
                    self.respond(service.run_detail(match[1], outputs=not bool(match[2])))
                except ValueError as exc:
                    self.respond({"error": str(exc)}, HTTPStatus.NOT_FOUND)
                return
            match = re.fullmatch(r"/api/comparisons/([0-9-]+)/export", path)
            if match:
                try:
                    service.run_detail(match[1], outputs=False)
                    with Store(service.settings.db_path) as store:
                        run = store.get_run(match[1])
                    bundle_path = Path(run.artifacts_dir) / "comparison.json"
                    self.respond(json.loads(bundle_path.read_text()))
                except (ValueError, OSError):
                    self.respond({"error": "comparison not found"}, HTTPStatus.NOT_FOUND)
                return
            asset = STATIC_ASSETS.get(path)
            if not asset:
                self.respond({"error": "Page not found."}, HTTPStatus.NOT_FOUND)
                return
            name, content_type = asset
            body = (assets / name).read_bytes()
            if name == "index.html":
                body = body.replace(b"REQUEST_TOKEN", token.encode())
            self.send_response(HTTPStatus.OK)
            self.send_header("Content-Type", content_type)
            self.send_header(
                "Content-Security-Policy",
                "default-src 'self'; script-src 'self'; "
                "style-src 'self'; connect-src 'self'; img-src 'self' data:; "
                "frame-ancestors 'none'; form-action 'none'; base-uri 'none'",
            )
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self) -> None:
            allowed_origins = {
                f"http://127.0.0.1:{self.server.server_port}",
                f"http://localhost:{self.server.server_port}",
            }
            if (
                not self.local_host()
                or self.headers.get("Origin") not in allowed_origins
                or self.headers.get("X-Pilot-Token") != token
            ):
                self.respond(
                    {"error": "Use the local workbench to run a check."}, HTTPStatus.FORBIDDEN
                )
                return
            if not self.path.startswith("/api/v1/") and self.path not in {
                "/api/validate",
                "/api/observations",
                "/api/configurations",
                "/api/comparisons",
                "/api/comparisons/import",
            }:
                self.respond({"error": "Unsupported action."}, HTTPStatus.NOT_FOUND)
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if (
                    not 0 < length <= 1024 * 1024
                    or self.headers.get("Content-Type") != "application/json"
                ):
                    raise ValueError("invalid request")
                body = self.rfile.read(length)
                if self.path.startswith("/api/v1/"):
                    data = strict_json(body.decode(), decimal_numbers=False)
                    payload, status = post_route(service, self.path, data)
                    self.respond(payload, status)
                elif self.path == "/api/configurations":
                    config = service.registry.create(NewConfiguration.model_validate_json(body))
                    self.respond(
                        {"configuration": config.model_dump(mode="json"), "status": "unrun"}
                    )
                elif self.path == "/api/comparisons":
                    data = strict_json(body.decode(), decimal_numbers=False)
                    self.respond(service.compare(ComparisonBundle.model_validate(data)))
                elif self.path == "/api/comparisons/import":
                    data = strict_json(body.decode(), decimal_numbers=False)
                    if set(data) != {"document"} or not isinstance(data["document"], str):
                        raise ValueError("expected comparison document text")
                    self.respond(service.import_comparison(data["document"]))
                elif self.path == "/api/observations":
                    self.respond(
                        service.record_observation(AppObservation.model_validate_json(body))
                    )
                else:
                    self.respond(service.validate(ValidationRequest.model_validate_json(body)))
            except ValueError as exc:
                message = exc.errors()[0]["msg"] if isinstance(exc, ValidationError) else str(exc)
                self.respond(
                    {"error": "Input rejected: " + message[:220]},
                    HTTPStatus.BAD_REQUEST,
                )
            except RuntimeError as exc:
                self.respond({"error": str(exc)}, HTTPStatus.CONFLICT)
            except Exception:
                self.respond(
                    {"error": "The check could not finish. Saved results are still available."},
                    HTTPStatus.INTERNAL_SERVER_ERROR,
                )

    return ThreadingHTTPServer(("127.0.0.1", port), Handler)


def serve(project: Path, port: int = 8765) -> None:
    service = PilotService(project)
    with create_server(project, port, service=service) as server:
        print(f"Krutrim Mulyankan evaluator: http://127.0.0.1:{server.server_port}/", flush=True)
        print(
            "Recorded app evidence, mock controls and verified CLI drafts. Paid APIs disabled.",
            flush=True,
        )
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            print("Local evaluator stopped; saved records are retained.", flush=True)
        finally:
            service.jobs.close()
