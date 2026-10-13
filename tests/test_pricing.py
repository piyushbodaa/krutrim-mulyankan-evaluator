import pytest

from kritrim_mulyankan.pricing import (
    MESSAGE_OVERHEAD_TOKENS,
    Price,
    PriceTable,
    bundled_prices,
    load_prices,
    max_input_tokens,
)


def test_bundled_prices_cover_current_claude_models_and_local_ollama():
    table = bundled_prices()
    opus = table.lookup("anthropic", "claude-opus-5-5")
    assert (opus.input, opus.output, opus.cache_read) == (4.0, 20.0, 0.2)
    assert opus.source.startswith("https://") and opus.as_of
    assert table.lookup("ollama", "llama3.2:3b").output == 0  # provider wildcard
    assert table.lookup("openai", "gpt-6") is None  # unverified prices are not bundled
    assert table.lookup("anthropic", "claude-unknown") is None


def test_cost_maths():
    price = Price(input=4, output=20)
    assert price.cost(1_000_000, 0) == pytest.approx(4.0)
    assert price.cost(100, 50) == pytest.approx((100 * 4 + 50 * 20) / 1e6)
    # Cache reads default to the input price, writes to 1.25x input.
    assert price.cost(0, 0, cache_read_tokens=1_000_000) == pytest.approx(4.0)
    assert price.cost(0, 0, cache_write_tokens=1_000_000) == pytest.approx(5.0)
    assert Price(input=4, output=20, cache_read=0.2).cost(
        0, 0, cache_read_tokens=1_000_000
    ) == pytest.approx(0.2)


def test_long_context_tier():
    haiku = bundled_prices().lookup("anthropic", "claude-haiku-5-5")
    assert haiku.cost(100_000, 1_000_000) == pytest.approx(0.01 + 0.5)
    assert haiku.cost(100_001, 1_000_000) == pytest.approx(100_001 * 0.5 / 1e6 + 2.5)


def test_override_file_wins_and_adds(tmp_path):
    path = tmp_path / "prices.yaml"
    path.write_text(
        "openai:\n  source: https://example.test/pricing\n  as_of: '2026-10-12'\n"
        "  models:\n    gpt-test: {input: 1.5, output: 6}\n"
        "anthropic:\n  models:\n    claude-opus-5-5: {input: 9, output: 9}\n"
    )
    table = load_prices(path)
    assert table.lookup("openai", "gpt-test").input == 1.5
    assert table.lookup("openai", "gpt-test").source == "https://example.test/pricing"
    assert table.lookup("anthropic", "claude-opus-5-5").input == 9
    assert table.lookup("anthropic", "claude-sonnet-5-5").input == 2  # untouched
    assert len(load_prices(tmp_path / "missing.yaml")) == len(bundled_prices())


@pytest.mark.parametrize(
    "bad",
    [
        "- just a list",
        "openai: {}",
        "openai:\n  models:\n    m: 3",
        "o:\n  models:\n    m: {input: 1}",
    ],
)
def test_malformed_price_files_are_rejected(tmp_path, bad):
    path = tmp_path / "prices.yaml"
    path.write_text(bad)
    with pytest.raises(ValueError):
        load_prices(path)


def test_max_input_tokens_is_bytes_plus_overhead():
    assert max_input_tokens("abcd") == 4 + MESSAGE_OVERHEAD_TOKENS
    assert max_input_tokens("₹") == 3 + MESSAGE_OVERHEAD_TOKENS  # 3 UTF-8 bytes


def test_empty_table():
    assert PriceTable().lookup("anthropic", "x") is None
