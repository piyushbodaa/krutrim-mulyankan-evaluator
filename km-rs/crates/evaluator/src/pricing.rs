//! Model price table, mirroring pricing.py (USD per million tokens).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub const MESSAGE_OVERHEAD_TOKENS: i64 = 64;
const BUNDLED: &str = include_str!("../../../../src/kritrim_mulyankan/prices.yaml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub long_context_threshold: Option<i64>,
    #[serde(default)]
    pub long_input: Option<f64>,
    #[serde(default)]
    pub long_output: Option<f64>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub as_of: Option<String>,
}

impl Price {
    pub fn cost(&self, input_tokens: i64, output_tokens: i64) -> f64 {
        self.cost_full(input_tokens, output_tokens, 0, 0)
    }

    pub fn cost_full(
        &self,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
    ) -> f64 {
        let prompt_tokens = input_tokens + cache_read_tokens + cache_write_tokens;
        let long = self
            .long_context_threshold
            .map(|t| prompt_tokens > t)
            .unwrap_or(false);
        let inp = if long {
            self.long_input.unwrap_or(self.input)
        } else {
            self.input
        };
        let out = if long {
            self.long_output.unwrap_or(self.output)
        } else {
            self.output
        };
        let read = self.cache_read.unwrap_or(inp);
        let write = self.cache_write.unwrap_or(inp * 1.25);
        (input_tokens as f64 * inp
            + output_tokens as f64 * out
            + cache_read_tokens as f64 * read
            + cache_write_tokens as f64 * write)
            / 1_000_000.0
    }
}

#[derive(Debug, Default, Clone)]
pub struct PriceTable {
    pub prices: BTreeMap<String, Price>,
}

impl PriceTable {
    pub fn lookup(&self, provider: &str, model: &str) -> Option<&Price> {
        self.prices
            .get(&format!("{provider}/{model}"))
            .or_else(|| self.prices.get(&format!("{provider}/*")))
    }

    pub fn parse(text: &str, origin: &str) -> Result<PriceTable, String> {
        let doc: serde_yaml::Value =
            serde_yaml::from_str(text).map_err(|e| format!("{origin}: {e}"))?;
        let mut prices = BTreeMap::new();
        let Some(root) = doc.as_mapping() else {
            if doc.is_null() {
                return Ok(PriceTable::default());
            }
            return Err(format!("{origin}: expected a mapping of providers"));
        };
        for (provider_v, block_v) in root {
            let provider = provider_v.as_str().unwrap_or_default().to_string();
            let block = block_v.as_mapping().ok_or_else(|| {
                format!("{origin}: provider '{provider}' needs a `models` mapping")
            })?;
            let source = block
                .get(serde_yaml::Value::String("source".into()))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let as_of = block
                .get(serde_yaml::Value::String("as_of".into()))
                .and_then(|v| match v {
                    serde_yaml::Value::String(s) => Some(s.clone()),
                    serde_yaml::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                });
            let models = block
                .get(serde_yaml::Value::String("models".into()))
                .and_then(|v| v.as_mapping())
                .ok_or_else(|| {
                    format!("{origin}: provider '{provider}' needs a `models` mapping")
                })?;
            for (model_v, entry_v) in models {
                let model = model_v.as_str().unwrap_or_default().to_string();
                let mut price: Price = serde_yaml::from_value(entry_v.clone())
                    .map_err(|e| format!("{origin}: {provider}/{model}: {e}"))?;
                if price.source.is_none() {
                    price.source = source.clone();
                }
                if price.as_of.is_none() {
                    price.as_of = as_of.clone();
                }
                prices.insert(format!("{provider}/{model}"), price);
            }
        }
        Ok(PriceTable { prices })
    }
}

pub fn bundled_prices() -> PriceTable {
    PriceTable::parse(BUNDLED, "bundled prices.yaml").unwrap_or_default()
}

pub fn load_prices(override_path: Option<&Path>) -> Result<PriceTable, String> {
    let mut table = bundled_prices();
    if let Some(p) = override_path {
        if p.is_file() {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            let over = PriceTable::parse(&text, &p.display().to_string())?;
            table.prices.extend(over.prices);
        }
    }
    Ok(table)
}

pub fn max_input_tokens(prompt: &str) -> i64 {
    prompt.len() as i64 + MESSAGE_OVERHEAD_TOKENS
}
