def test_list_suites_tasks_and_runners(km):
    result = km("list")
    assert result.exit_code == 0, result.output
    assert "example" in result.output and "5" in result.output

    result = km("list", "tasks")
    assert result.exit_code == 0, result.output
    assert "writing.late-order-apology" in result.output and "llm_judge" in result.output

    result = km("list", "runners")
    assert result.exit_code == 0, result.output
    assert "oracle" in result.output and "planned" in result.output


def test_list_runs_before_any_run(km):
    result = km("list", "runs")
    assert result.exit_code == 0 and "No runs yet" in result.output


def test_dry_run_executes_nothing(km):
    result = km("run", "example", "-r", "echo", "-r", "oracle", "--dry-run")
    assert result.exit_code == 0, result.output
    assert "Dry run" in result.output and "$0.0000" in result.output
    assert not (km.tmp_path / "runs").exists()


def test_run_then_report(km):
    result = km("run", "example", "-r", "echo", "-r", "oracle")
    assert result.exit_code == 0, result.output
    run_dirs = [p for p in (km.tmp_path / "runs").iterdir() if p.is_dir()]
    assert len(run_dirs) == 1
    run_dir = run_dirs[0]
    assert (run_dir / "report.md").is_file() and (run_dir / "report.html").is_file()
    assert "100% (4/4)" in result.output

    (run_dir / "report.md").unlink()
    result = km("report", "--format", "md")
    assert result.exit_code == 0, result.output
    assert (run_dir / "report.md").is_file() and "report.md" in result.output

    result = km("list", "runs")
    assert run_dir.name in result.output


def test_run_a_single_task_with_default_runner(km):
    result = km("run", "example", "-t", "coding.sum-of-squares")
    assert result.exit_code == 0, result.output
    assert "coding.sum-of-squares" in result.output and "echo" in result.output


def test_bad_inputs_exit_cleanly(km):
    assert km("run", "nope").exit_code == 1
    assert km("run", "example", "-r", "bogus").exit_code == 1
    assert km("run", "example", "-t", "missing.task").exit_code == 1
    assert km("run", "example", "-r", "echo", "-r", "echo").exit_code == 1
    assert km("report").exit_code == 1  # nothing recorded yet


def test_list_prices(km):
    result = km("list", "prices")
    assert result.exit_code == 0, result.output
    assert "anthropic/claude-opus-5-5" in result.output and "ollama/*" in result.output


def test_model_dry_run_reports_missing_price_and_key(km):
    result = km("run", "example", "-r", "model:openai/gpt-test", "--dry-run")
    assert result.exit_code == 1, result.output
    assert "no price for openai/gpt-test" in result.output
    assert "OPENAI_API_KEY is not set" in result.output
    assert "would be refused" in result.output
    assert not (km.tmp_path / "runs").exists()


def test_model_dry_run_with_price_override_and_key(km):
    (km.tmp_path / "prices.yaml").write_text(
        "openai:\n  models:\n    gpt-test: {input: 1.0, output: 4.0}\n"
    )
    result = km(
        "run", "example", "-r", "model:openai/gpt-test,max_tokens=1000", "--dry-run",
        env={"OPENAI_API_KEY": "sk-test"},
    )  # fmt: skip
    assert result.exit_code == 0, result.output
    assert "would start" in result.output
    # 5 attempts x 1000 output tokens x $4/MTok = $0.02 plus a little input.
    assert "$0.02" in result.output


def test_real_run_is_refused_when_not_ready(km):
    result = km("run", "example", "-r", "model:anthropic/claude-opus-5-5", "--max-usd", "0.01",
                env={"ANTHROPIC_API_KEY": "sk-test"})  # fmt: skip
    assert result.exit_code == 1
    assert "worst-case cost" in result.output and "refusing to run" in result.output
    assert not (km.tmp_path / "runs").exists()


def test_km_is_the_command_and_mj_remains_an_alias():
    from importlib.metadata import entry_points

    scripts = {ep.name: ep.value for ep in entry_points(group="console_scripts")}
    assert scripts["km"] == "kritrim_mulyankan.cli:app"
    assert scripts["mj"] == "kritrim_mulyankan.cli:app"


def test_help_uses_the_project_name(km):
    result = km("--help")
    assert result.exit_code == 0 and "Krutrim Mulyankan" in result.output


def test_paid_cli_requires_explicit_execution_permission(km):
    (km.tmp_path / "prices.yaml").write_text(
        "openai:\n  models:\n    gpt-test: {input: 1, output: 4}\n"
    )
    result = km(
        "run",
        "example",
        "-r",
        "model:openai/gpt-test",
        "--max-usd",
        "1",
        env={"OPENAI_API_KEY": "test-only-not-real"},
    )
    assert result.exit_code == 1 and "--allow-paid" in result.output
    assert not (km.tmp_path / "runs").exists()


def test_export_command_matches_versioned_contract(km):
    assert km("run", "example", "-r", "oracle", "--max-usd", "0").exit_code == 0
    output = km.tmp_path / "public.json"
    result = km("export", "--output", str(output))
    assert result.exit_code == 0, result.output
    import json

    payload = json.loads(output.read_text())
    assert payload["schema_version"] == "1.1"  # 1.1 adds repeat_index, repeats and verdicts.
    assert payload["run"]["evaluation_mode"] == "mock" and payload["run"]["repeats"] == 1
    assert {item["repeat_index"] for item in payload["results"]} == {0}
    assert payload["verdicts"] == []  # One runner: nothing to compare.
    assert all(item["output"] is None for item in payload["results"])
    assert all(not item["eligible_for_model_ranking"] for item in payload["aggregates"])
