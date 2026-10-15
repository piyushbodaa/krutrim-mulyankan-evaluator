"""Release repackaging must use the exact public, checksum-verified payload."""

import hashlib
import io
import json
import runpy
from pathlib import Path

import pytest

MODULE = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/fetch_linux_release.py"))
REPO = MODULE["REPOSITORY"]
PAYLOADS = {name: (name + " payload").encode() for name in MODULE["INPUTS"]}


def release():
    return {
        "tag_name": "v0.3.0",
        "draft": False,
        "prerelease": False,
        "assets": [
            {
                "name": name,
                "browser_download_url": f"https://github.com/{REPO}/releases/download/v0.3.0/{name}",
                "state": "uploaded",
                "size": len(data),
                "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
            }
            for name, data in PAYLOADS.items()
        ],
    }


def mock_downloads(monkeypatch, metadata, *, corrupt=False):
    requests = []

    def download(request, **kwargs):
        url = request.full_url if hasattr(request, "full_url") else request
        requests.append(url)
        if url.startswith("https://api.github.com/"):
            return io.BytesIO(json.dumps(metadata).encode())
        name = url.rsplit("/", 1)[1]
        return io.BytesIO(b"damaged" if corrupt else PAYLOADS[name])

    monkeypatch.setattr(MODULE["urllib"].request, "urlopen", download)
    return requests


def test_fetches_exact_release_and_records_verified_provenance(tmp_path, monkeypatch):
    requests = mock_downloads(monkeypatch, release())
    result = MODULE["fetch_release"](tmp_path, "v0.3.0")
    assert len(requests) == 3
    for name, data in PAYLOADS.items():
        assert (tmp_path / name).read_bytes() == data
        assert result["inputs"][name]["sha256"] == hashlib.sha256(data).hexdigest()
    assert json.loads((tmp_path / "release-inputs.json").read_text()) == result


@pytest.mark.parametrize("field", ["draft", "prerelease"])
def test_refuses_unpublished_or_preview_release(tmp_path, monkeypatch, field):
    metadata = release()
    metadata[field] = True
    requests = mock_downloads(monkeypatch, metadata)
    with pytest.raises(ValueError, match="public stable"):
        MODULE["fetch_release"](tmp_path, "v0.3.0")
    assert len(requests) == 1
    assert not list(tmp_path.iterdir())


def test_rejects_foreign_asset_url_before_download(tmp_path, monkeypatch):
    metadata = release()
    metadata["assets"][0]["browser_download_url"] = "https://example.com/other.AppImage"
    requests = mock_downloads(monkeypatch, metadata)
    with pytest.raises(ValueError, match="Invalid release input"):
        MODULE["fetch_release"](tmp_path, "v0.3.0")
    assert len(requests) == 1


def test_checksum_failure_preserves_previous_payload(tmp_path, monkeypatch):
    name = MODULE["INPUTS"][0]
    (tmp_path / name).write_bytes(b"previous verified download")
    mock_downloads(monkeypatch, release(), corrupt=True)
    with pytest.raises(ValueError, match="checksum"):
        MODULE["fetch_release"](tmp_path, "v0.3.0")
    assert (tmp_path / name).read_bytes() == b"previous verified download"
    assert not list(tmp_path.glob("*.part"))
    assert not (tmp_path / "release-inputs.json").exists()
