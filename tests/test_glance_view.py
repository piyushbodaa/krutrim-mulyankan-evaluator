"""The "At a glance" view: the plain-language dashboard both UIs open on.

showcase.js turns the results payload into everyday instruments (report card, speedometer, chat
replay, phone-style comparison). It may read outcomes, wait times, subjects and languages only;
prompts, context, expected answers and model outputs never reach it.
"""

import json
import re
from pathlib import Path

import pytest

from kritrim_mulyankan.local_ui import TITLES

REPO = Path(__file__).resolve().parent.parent
PREVIEW = REPO / "preview"
LOCAL = REPO / "src" / "kritrim_mulyankan" / "ui_assets"
ROOTS = [PREVIEW, LOCAL]


@pytest.mark.parametrize("root", ROOTS, ids=["preview", "local"])
def test_pages_open_on_the_glance_view(root):
    html = (root / "index.html").read_text()
    nav = re.search(r'<nav class="nav"[^>]*>(.*?)</nav>', html).group(1)
    first_tab = '<button type="button" data-view="glance" aria-current="page">At a glance'
    assert nav.startswith(first_tab)
    assert '<section id="glance-view" aria-labelledby="glance-title">' in html
    assert '<section id="board-view" aria-labelledby="board-title" hidden>' in html
    scripts = re.findall(r'<script src="/?([\w-]+)\.js"', html)
    assert scripts.index("app") < scripts.index("showcase") < scripts.index("jobs")


@pytest.mark.parametrize("root", ROOTS, ids=["preview", "local"])
def test_app_routes_to_glance_and_feeds_it_the_board_payload(root):
    app = (root / "app.js").read_text()
    assert "const viewSlugs={glance:''," in app
    assert "find(key=>viewSlugs[key]===slug)||'glance';}" in app
    assert "if(typeof renderGlance==='function')renderGlance(payload,job,adapters);" in app
    assert "if(typeof refreshGlance==='function')refreshGlance();" in app


def test_glance_never_reads_prompts_or_answers():
    source = (PREVIEW / "showcase.js").read_text()
    for field in ["output", "full_prompt", "instruction", "context", "expected", "prompt"]:
        assert not re.search(rf"\.{field}\b", source), field


def test_preview_task_titles_match_the_local_workbench():
    fixtures = (PREVIEW / "preview-fixtures.js").read_text()
    data, _ = json.JSONDecoder().raw_decode(fixtures[fixtures.index("{") :])
    pilot = [task for task in data["tasks"] if not task["id"].startswith("DEMO")]
    assert len(pilot) == 24
    for task in pilot:
        assert task["title"] == TITLES[task["id"]]
