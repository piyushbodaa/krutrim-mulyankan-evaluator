from pathlib import Path

import pytest
import yaml
from pydantic import ValidationError

from kritrim_mulyankan.suite import Task, load_suite, resolve_suite


def _task(**overrides) -> dict:
    base = {
        "id": "t.one",
        "category": "reasoning",
        "prompt": "What is 2+2?",
        "grader": {"type": "exact", "expected": "4"},
    }
    return {**base, **overrides}


def _write_suite(tmp_path: Path, tasks: list[dict], **extra) -> Path:
    path = tmp_path / "s.yaml"
    path.write_text(yaml.safe_dump({"name": "s", "version": 1, "tasks": tasks, **extra}))
    return path


def test_example_suite_has_five_tasks_one_per_category(example_suite):
    assert len(example_suite.tasks) == 5
    assert {t.category for t in example_suite.tasks} == {
        "coding",
        "reasoning",
        "writing",
        "tool-use",
        "life-admin",
    }
    assert example_suite.version == "1"


def test_prompt_inputs_are_rendered(example_suite):
    task = next(t for t in example_suite.tasks if t.id == "tool-use.pick-tool")
    prompt = task.render_prompt()
    assert "{{" not in prompt
    assert "create_event(title, start, end)" in prompt
    assert "dentist appointment" in prompt


def test_single_expected_string_becomes_list():
    task = Task.model_validate(_task())
    assert task.grader.expected == ["4"]


def test_unknown_placeholder_is_rejected():
    with pytest.raises(ValidationError, match="unknown inputs"):
        Task.model_validate(_task(prompt="Hi {{ name }}"))


def test_llm_judge_requires_rubric():
    with pytest.raises(ValidationError, match="rubric"):
        Task.model_validate(_task(grader={"type": "llm_judge"}))


def test_unknown_fields_are_rejected():
    with pytest.raises(ValidationError):
        Task.model_validate(_task(promt="typo"))


def test_duplicate_task_ids_are_rejected(tmp_path):
    path = _write_suite(tmp_path, [_task(), _task()])
    with pytest.raises(ValidationError, match="duplicate"):
        load_suite(path)


def test_missing_fixture_is_rejected(tmp_path):
    path = _write_suite(tmp_path, [_task(fixtures=["nope.txt"])])
    with pytest.raises(ValueError, match="fixture not found"):
        load_suite(path)


def test_content_hash_is_stable_and_tracks_edits():
    a = Task.model_validate(_task())
    assert a.content_hash() == Task.model_validate(_task()).content_hash()
    assert a.content_hash() != Task.model_validate(_task(prompt="What is 3+3?")).content_hash()


def test_select_tasks(example_suite):
    picked = example_suite.select(["coding.sum-of-squares"])
    assert [t.id for t in picked] == ["coding.sum-of-squares"]
    assert len(example_suite.select(None)) == 5
    with pytest.raises(ValueError, match="unknown task ids"):
        example_suite.select(["nope"])


def test_resolve_suite_by_name_or_path(suites_dir, tmp_path):
    assert resolve_suite("example", suites_dir) == suites_dir / "example.yaml"
    path = _write_suite(tmp_path, [_task()])
    assert resolve_suite(str(path), suites_dir) == path
    with pytest.raises(FileNotFoundError):
        resolve_suite("missing", suites_dir)
