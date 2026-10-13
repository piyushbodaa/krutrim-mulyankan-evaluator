"""The evaluator uses the owner-approved Chakra Engine identity, unmodified, in both of its UIs.

The hosted preview (preview/) and the local workbench (src/kritrim_mulyankan/ui_assets/) carry the
same brand files. Hashes are those of the approved kit's exports, so any redrawn, recoloured or
simplified substitute fails here.
"""

import hashlib
import re
import struct
import threading
import urllib.error
import urllib.request
from pathlib import Path

import pytest

from kritrim_mulyankan.local_ui import STATIC_ASSETS, PilotService, create_server

REPO = Path(__file__).resolve().parent.parent
PREVIEW = REPO / "preview"
LOCAL = REPO / "src" / "kritrim_mulyankan" / "ui_assets"
ROOTS = [PREVIEW, LOCAL]

# SHA-256 of files in Krutrim-Mulyankan-Chakra-Engine-Production-Kit, assets/ and assets/fonts/.
KIT_FILES = {
    "km-chakra-wordmark.svg": "799e73c5c2a00d42df85e2e0b8edab4bb1a3974cf377c78233823a685040158d",
    "km-chakra-wordmark-white.svg": (
        "8d85738a8b446b1abeb89b41aff858fb4938d151a62ea315012a671612465c29"
    ),
    "favicon.ico": "61ef00fc2f8a0bc498f7bfe52451a7c29ad441d0dbaeb28a7ac3989f39414916",
    "favicon-16.png": "b2b067da36755a9e0085c9301cd2e29d10720f966cfb194af657bd8140afda57",
    "favicon-32.png": "082ac24c54cfbb1b2f90cde5e22db03076ec0151cfe548c30bdbad1d85cd181d",
    "favicon-180.png": "79821bab41953a6432cc2757623254099cf7d870ef55632057dce66cb45d9cf9",
    "fonts/Sora-OFL.txt": "ba0b9729c9428ba79a0459ab8ec575791b51509dbec213e383d0316d37fec299",
    "fonts/Noto-LICENSE.txt": "075f91382875da1c4b491f1facf2586f55448705133264d98ce15cab6211b125",
}
# The kit's Sora.ttf and NotoSansDevanagari-Regular.ttf, losslessly converted with woff2_compress.
WEB_FONTS = ["fonts/Sora.woff2", "fonts/NotoSansDevanagari-Regular.woff2"]
SHARED = ["style.css", "theme.js", "compare.js"]
# Carbon, white, neutral, secondary text, borders; then the main site's dark palette and hover.
PALETTE = {"#171717", "#FFFFFF", "#F3F3F1", "#5F6065", "#D8D8D5"}
MAIN_SITE_EXTRAS = {"#242422", "#41413E", "#BEBEBA", "#444444"}
OLD_WHEEL = "M18 3v11m0 8v11"  # The unapproved eight-spoke wheel that the kit lockup replaces.


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@pytest.mark.parametrize("root", ROOTS, ids=["preview", "local"])
@pytest.mark.parametrize("name", sorted(KIT_FILES))
def test_brand_files_are_the_unmodified_kit_exports(root, name):
    assert sha256(root / "brand" / name) == KIT_FILES[name]


@pytest.mark.parametrize("name", SHARED + [f"brand/{n}" for n in [*KIT_FILES, *WEB_FONTS]])
def test_preview_and_local_workbench_stay_identical(name):
    assert (PREVIEW / name).read_bytes() == (LOCAL / name).read_bytes()


@pytest.mark.parametrize("root", ROOTS, ids=["preview", "local"])
def test_pages_show_the_black_logo_on_light_and_white_logo_on_dark(root):
    html = (root / "index.html").read_text()
    assert re.search(r'<img class="logo-on-light" src="/?brand/km-chakra-wordmark\.svg"', html)
    assert re.search(r'<img class="logo-on-dark" src="/?brand/km-chakra-wordmark-white\.svg"', html)
    for icon in ["favicon.ico", "favicon-16.png", "favicon-32.png", "favicon-180.png"]:
        assert f'brand/{icon}"' in html
    assert re.search(r'<script src="/?theme\.js"></script>', html), "theme must apply before paint"
    css = (root / "style.css").read_text()
    assert ":root[data-theme=dark] .logo-on-dark{display:block!important}" in css
    for text in [html, css, (root / "app.js").read_text()]:
        assert OLD_WHEEL not in text and "brand-mark" not in text


@pytest.mark.parametrize("root", ROOTS, ids=["preview", "local"])
def test_styles_use_the_monochrome_palette_sora_and_noto(root):
    css = (root / "style.css").read_text()
    colours = {c.upper() for c in re.findall(r"#[0-9a-fA-F]{6}\b", css)}
    assert colours >= PALETTE
    assert colours <= PALETTE | MAIN_SITE_EXTRAS, colours - PALETTE - MAIN_SITE_EXTRAS
    assert not re.search(r"\b(rgba?|hsla?)\(", css)
    assert "#F04B32" not in css.upper()  # Historical signal orange is not used.
    assert 'url("brand/fonts/Sora.woff2")' in css
    assert 'url("brand/fonts/NotoSansDevanagari-Regular.woff2")' in css
    assert '--font:"Sora","Noto Sans Devanagari"' in css
    assert ":lang(hi){font-family:var(--hindi)}" in css
    assert "h1,h2,h3{font-weight:600}" in css


def test_social_card_is_declared_and_is_1200_by_630():
    html = (PREVIEW / "index.html").read_text()
    card = "https://krutrim-mulyankan-evaluator.vercel.app/brand/social-card.png"
    assert f'<meta property="og:image" content="{card}">' in html
    assert f'<meta name="twitter:image" content="{card}">' in html
    assert '<meta name="twitter:card" content="summary_large_image">' in html
    png = (PREVIEW / "brand" / "social-card.png").read_bytes()
    assert png[:8] == b"\x89PNG\r\n\x1a\n" and png[12:16] == b"IHDR"
    assert struct.unpack(">II", png[16:24]) == (1200, 630)


@pytest.fixture
def endpoint(settings):
    server = create_server(REPO, 0, service=PilotService(REPO, settings))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def test_local_workbench_serves_brand_files_from_a_fixed_allow_list(endpoint):
    for path, (name, content_type) in STATIC_ASSETS.items():
        with urllib.request.urlopen(endpoint + path) as response:
            assert response.headers["Content-Type"] == content_type, path
            body = response.read()
        if name != "index.html":
            assert body == (LOCAL / name).read_bytes(), path
    for path in ["/brand/../local_ui.py", "/brand/social-card.png", "/brand/fonts/Sora.ttf"]:
        with pytest.raises(urllib.error.HTTPError) as refused:
            urllib.request.urlopen(endpoint + path)
        assert refused.value.code == 404
