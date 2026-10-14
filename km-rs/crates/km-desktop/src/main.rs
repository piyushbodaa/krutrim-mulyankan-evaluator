//! km-desktop: Rust desktop engine bridge, replacing the PyInstaller sidecar
//! that ran `kritrim_mulyankan.desktop_bridge`. Same JSON-lines stdio
//! protocol, same loopback dashboard, same --http preview shell, no secrets
//! written to disk, paid runs only after `confirmed: true` (and model
//! runners are refused until they are ported; nothing is called).

use krutrim_evaluator::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const HONESTY: &[(&str, &str)] = &[
    ("pilot", "Unvalidated pilot"),
    ("ranking", "Not a ranking"),
    ("close", "Too close to call"),
    (
        "close_means",
        "Too close to call means the gap could be chance, not that they are equal.",
    ),
];

const KEY_ENVS: &[(&str, &str)] = &[
    ("openai", "OPENAI_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("gemini", "GEMINI_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
    ("compatible", "COMPATIBLE_API_KEY"),
];

const URL_ENVS: &[&str] = &["OLLAMA_BASE_URL", "VLLM_BASE_URL", "COMPATIBLE_BASE_URL"];
const MAX_REPEATS: i64 = 5;
const MAX_RUNNERS: usize = 6;

fn honesty() -> Value {
    let mut m = Map::new();
    for (k, v) in HONESTY {
        m.insert(k.to_string(), json!(v));
    }
    Value::Object(m)
}

fn providers() -> Value {
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "openai",
            "OpenAI",
            "For GPT models. You are billed per token.",
            "GPT मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
            "https://platform.openai.com/api-keys",
        ),
        (
            "anthropic",
            "Anthropic",
            "For Claude models. You are billed per token.",
            "Claude मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
            "https://console.anthropic.com/settings/keys",
        ),
        (
            "gemini",
            "Google",
            "For Gemini models. You are billed per token.",
            "Gemini मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
            "https://aistudio.google.com/apikey",
        ),
        (
            "xai",
            "xAI",
            "For Grok models. You are billed per token.",
            "Grok मॉडल के लिए। शुल्क प्रति टोकन लगता है।",
            "https://console.x.ai/",
        ),
        (
            "openrouter",
            "OpenRouter",
            "One key that can call many models.",
            "एक कुंजी से कई मॉडल चल सकते हैं।",
            "https://openrouter.ai/keys",
        ),
        (
            "compatible",
            "Other compatible API",
            "A service that speaks the OpenAI API. You also set its address.",
            "कोई सेवा जो OpenAI एपीआई बोलती हो। उसका पता भी देना होगा।",
            "",
        ),
    ];
    Value::Array(
        rows.iter()
            .map(|(id, name, blurb, blurb_hi, signup)| {
                json!({"id": id, "name": name, "blurb": blurb, "blurb_hi": blurb_hi,
                       "signup": signup,
                       "env": key_env(id)})
            })
            .collect(),
    )
}

fn key_env(provider: &str) -> &'static str {
    KEY_ENVS
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, e)| *e)
        .unwrap_or("")
}

fn titles() -> BTreeMap<&'static str, &'static str> {
    const T: &[(&str, &str)] = &[
        ("NUM01", "Add up an office budget"),
        ("NUM02", "Check an invoice total"),
        ("NUM03", "Calculate a balance in Hindi"),
        ("NUM04", "Choose a supplier within budget"),
        ("DATE01", "Convert an Indian meeting time to UTC"),
        ("DATE02", "Schedule a meeting in India"),
        ("DATE03", "Find a payment due date"),
        ("DATE04", "Spot an unclear date"),
        ("DOC01", "Remove duplicate invoices"),
        ("DOC02", "Reconcile an office delivery"),
        ("DOC03", "Read a Hindi meeting note"),
        ("DOC04", "Compare two supplier quotes"),
        ("LANG01", "Prioritise open tickets in Hindi"),
        ("LANG02", "Read a Hinglish deadline"),
        ("LANG03", "Check which files can be sent"),
        ("LANG04", "Follow the latest dispatch update"),
        ("EVID01", "Find the approved budget"),
        ("EVID02", "Check a time-zone reference"),
        ("EVID03", "Know when the evidence is missing"),
        ("EVID04", "Compare forecast and actual spend"),
        ("ROB01", "Ignore instructions hidden in an invoice"),
        ("ROB02", "Summarise without personal details"),
        ("ROB03", "Ask for missing information"),
        ("ROB04", "Spot an unreadable amount"),
        ("RAIL01", "Save a railway itinerary draft"),
        ("DEMO01", "Add two rupee amounts"),
        ("DEMO02", "Drop a repeated identifier"),
        ("DEMO03", "Say when a date is missing"),
        ("DEMO04", "Say when a date is missing"),
    ];
    T.iter().cloned().collect()
}

fn resource_root() -> PathBuf {
    if let Ok(v) = std::env::var("KM_RESOURCE_ROOT") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(real) = exe.canonicalize() {
            if let Some(dir) = real.parent() {
                if dir.join("preview").is_dir() {
                    return dir.to_path_buf();
                }
            }
        }
    }
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        if d.join("preview").is_dir() && d.join("desktop").join("ui").is_dir() {
            return d;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn ui_dir(root: &Path) -> PathBuf {
    if let Ok(v) = std::env::var("KM_UI_ROOT") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    root.join("desktop").join("ui")
}

fn token_urlsafe() -> String {
    let mut bytes = [0u8; 24];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    } else {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((t >> (i % 16 * 4)) & 0xff) as u8 ^ (i as u8).wrapping_mul(37);
        }
    }
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, b| (a << 8) | *b as u32) << (8 * (3 - chunk.len()));
        for shift in [18, 12, 6, 0].into_iter().take(chunk.len() + 1) {
            out.push(ALPHA[((n >> shift) & 63) as usize] as char);
        }
    }
    out
}

struct Session {
    settings: Settings,
    env: Mutex<BTreeMap<String, String>>,
    root: PathBuf,
    preview_dir: PathBuf,
    cancel: Arc<AtomicBool>,
    busy: Arc<Mutex<bool>>,
    token: String,
    http_port: Mutex<Option<u16>>,
    external_base: Mutex<Option<String>>,
    names: Mutex<BTreeMap<String, String>>,
}

impl Session {
    fn new() -> Session {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let root = resource_root();
        let runs = PathBuf::from(
            env.get("MJ_RUNS_DIR")
                .cloned()
                .unwrap_or_else(|| root.join("runs").to_string_lossy().into_owned()),
        );
        let settings = Settings {
            db_path: env
                .get("MJ_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| runs.join("results.db")),
            suites_dir: env
                .get("MJ_SUITES_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("suites")),
            prices_path: env
                .get("MJ_PRICES")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("prices.yaml")),
            max_usd: env
                .get("MJ_MAX_USD")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            usd_to_inr: env
                .get("MJ_USD_INR")
                .and_then(|v| v.parse().ok())
                .unwrap_or(88.0),
            runs_dir: runs,
        };
        Session {
            preview_dir: root.join("preview"),
            settings,
            env: Mutex::new(env),
            root,
            cancel: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(Mutex::new(false)),
            token: token_urlsafe(),
            http_port: Mutex::new(None),
            external_base: Mutex::new(None),
            names: Mutex::new(BTreeMap::new()),
        }
    }

    fn env_snapshot(&self) -> BTreeMap<String, String> {
        self.env.lock().unwrap().clone()
    }

    fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        for value in self.env_snapshot().values() {
            if value.len() >= 8 && out.contains(value.as_str()) {
                out = out.replace(value.as_str(), "[REDACTED]");
            }
        }
        krutrim_evaluator::provenance::redact_str(&out, &[])
    }

    fn prices(&self) -> PriceTable {
        let p = if self.settings.prices_path.is_file() {
            Some(self.settings.prices_path.as_path())
        } else {
            None
        };
        load_prices(p).unwrap_or_default()
    }

    fn suites(&self) -> Result<Vec<Suite>, String> {
        let dir = effective_suites_dir(&self.settings);
        let mut suites = Vec::new();
        for p in discover_suites(&dir) {
            suites.push(load_suite(&p)?);
        }
        const ORDER: &[&str] = &["india-pilot", "railway-pilot", "example"];
        suites.sort_by_key(|s| {
            (
                ORDER
                    .iter()
                    .position(|n| *n == s.name)
                    .unwrap_or(ORDER.len()),
                s.name.clone(),
            )
        });
        Ok(suites)
    }

    fn load_named(&self, name: &str) -> Result<Suite, String> {
        let valid = !name.is_empty()
            && name.len() <= 81
            && name
                .chars()
                .next()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false)
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !valid {
            return Err("Choose a test pack from the list.".into());
        }
        self.suites()?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("No test pack named {name}."))
    }
}

fn plain_problem(text: &str) -> String {
    if text.contains("is not set") || text.contains("no Anthropic credentials") {
        return "This model needs an API key. Add it on the keys screen, or pick a practice check. Nothing was called.".into();
    }
    if text.contains("no price") {
        return "There is no checked price for this model, so a paid run is refused. Nothing was called.".into();
    }
    if text.contains("exceeds") && text.contains("cap") {
        return "The worst-case cost is above your spending cap. Raise the cap or run a smaller pack. Nothing was called.".into();
    }
    text.to_string()
}

fn check_base_url(value: &str) -> Result<String, String> {
    let v = value.trim();
    let scheme_ok = v.starts_with("http://") || v.starts_with("https://");
    let after = v.split("://").nth(1).unwrap_or("");
    let host = after.split(['/', '?', '#']).next().unwrap_or("");
    if !scheme_ok || host.is_empty() {
        return Err("The address must start with http:// or https://.".into());
    }
    let authority = host.rsplit('@').next().unwrap_or(host);
    if host.contains('@') || authority.is_empty() || v.contains('?') || v.contains('#') {
        return Err("The address must not include a password, query, or fragment.".into());
    }
    Ok(v.to_string())
}

impl Session {
    fn ping(&self) -> Value {
        json!({"ok": true, "version": env!("CARGO_PKG_VERSION"),
               "honesty": honesty(), "engine": "kritrim_mulyankan"})
    }

    fn set_keys(&self, keys: &Map<String, Value>) -> Result<Value, String> {
        let allowed: Vec<&str> = KEY_ENVS.iter().map(|(_, e)| *e).collect();
        let mut saved = Vec::new();
        for (name, value) in keys {
            if !allowed.contains(&name.as_str()) {
                return Err("Unknown key name.".into());
            }
            let Some(v) = value.as_str().filter(|s| s.len() <= 4096) else {
                return Err("A key must be a short string.".into());
            };
            if v.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
                return Err("A key cannot contain a line break.".into());
            }
            if v.is_empty() {
                self.env.lock().unwrap().remove(name);
            } else {
                self.env.lock().unwrap().insert(name.clone(), v.to_string());
                saved.push(name.clone());
            }
        }
        Ok(json!({"saved": saved}))
    }

    fn set_urls(&self, urls: &Map<String, Value>) -> Result<Value, String> {
        let mut applied = Vec::new();
        for (name, value) in urls {
            if !URL_ENVS.contains(&name.as_str()) {
                return Err("Unknown address.".into());
            }
            let v = value.as_str().unwrap_or("");
            if v.is_empty() {
                self.env.lock().unwrap().remove(name);
            } else {
                let checked = check_base_url(v)?;
                self.env.lock().unwrap().insert(name.clone(), checked);
            }
            applied.push(name.clone());
        }
        Ok(json!({"applied": applied}))
    }

    fn key_status(&self) -> Value {
        let mut m = Map::new();
        for (provider, env_name) in KEY_ENVS {
            m.insert(
                provider.to_string(),
                json!({"env": env_name,
                       "saved": self.env_snapshot().get(*env_name).map(|v| !v.is_empty()).unwrap_or(false)}),
            );
        }
        Value::Object(m)
    }

    fn runner(&self, spec: &str, prices: &PriceTable) -> Result<Runner, String> {
        if spec != "echo" && spec != "oracle" && !spec.starts_with("model:") {
            return Err("This app runs practice checks and model APIs. Other adapters stay on the command line.".into());
        }
        make_runner(spec, prices, &self.env_snapshot())
    }

    fn clean_specs(&self, params: &Map<String, Value>) -> Result<Vec<String>, String> {
        let specs: Vec<String> = params
            .get("runners")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if specs.is_empty() {
            return Err("Pick at least one model or practice check.".into());
        }
        if specs.len() > MAX_RUNNERS {
            return Err(format!("Pick at most {MAX_RUNNERS} models."));
        }
        let unique: std::collections::HashSet<&String> = specs.iter().collect();
        if unique.len() != specs.len() {
            return Err("The same model was picked twice.".into());
        }
        Ok(specs)
    }

    fn catalog(&self) -> Result<Value, String> {
        let prices = self.prices();
        let mut models: Vec<Value> = Vec::new();
        for (spec, name, name_hi, blurb, blurb_hi) in [
            (
                "oracle",
                "Correct-answer check",
                "सही उत्तर वाली जाँच",
                "Returns the expected answer so you can see the scorer work. Not an AI model.",
                "अपेक्षित उत्तर देता है, ताकि स्कोर जाँचा जा सके। यह एआई मॉडल नहीं है।",
            ),
            (
                "echo",
                "Echo check",
                "दोहराने वाली जाँच",
                "Repeats the question and should fail. That shows the scorer is awake.",
                "प्रश्न को दोहराता है और फेल होना चाहिए। इससे पता चलता है कि स्कोर जाग रहा है।",
            ),
        ] {
            models.push(json!({"spec": spec, "name": name, "name_hi": name_hi,
                "provider": "practice", "blurb": blurb, "blurb_hi": blurb_hi,
                "free": true, "suggested": true, "runnable": true,
                "price_usd": {"input": 0, "output": 0}}));
        }
        const SUGGESTED: &[&str] = &["claude-haiku-4-5", "claude-sonnet-4-6", "claude-opus-4-6"];
        for (key, price) in &prices.prices {
            let Some((provider, model)) = key.split_once('/') else {
                continue;
            };
            if ["ollama", "vllm"].contains(&provider) || model == "*" {
                continue;
            }
            let blurb = model_blurb(model);
            models.push(json!({
                "spec": format!("model:{provider}/{model}"), "name": model, "name_hi": model,
                "provider": provider, "blurb": blurb, "blurb_hi": blurb, "free": false,
                "suggested": SUGGESTED.contains(&model), "runnable": true,
                "price_usd": {"input": price.input, "output": price.output},
                "price_as_of": price.as_of}));
        }
        for (provider, title) in [
            ("openai", "OpenAI models"),
            ("gemini", "Google Gemini models"),
            ("xai", "xAI Grok models"),
            ("openrouter", "OpenRouter models"),
        ] {
            if models
                .iter()
                .any(|c| c["provider"] == provider && c["runnable"] == Value::Bool(true))
            {
                continue;
            }
            models.push(json!({
                "spec": null, "name": title, "name_hi": title, "provider": provider,
                "blurb": "No checked price is on file, so a paid run is refused. Add a price, then pick a model. Nothing is called until you do.",
                "blurb_hi": "जाँची हुई कीमत दर्ज नहीं है, इसलिए भुगतान वाला रन रुक जाता है। कीमत जोड़ने के बाद मॉडल चुनें।",
                "free": false, "suggested": false, "runnable": false, "price_usd": null}));
        }
        let mut names = BTreeMap::new();
        for card in &models {
            if let (Some(spec), Some(name)) = (card["spec"].as_str(), card["name"].as_str()) {
                names.insert(spec.to_string(), name.to_string());
            }
        }
        *self.names.lock().unwrap() = names;
        let mut packs = Vec::new();
        for suite in self.suites()? {
            let (name, name_hi, badge, summary, summary_hi) = pack_copy(&suite);
            let mut languages: Vec<&str> =
                suite.tasks.iter().map(|t| t.language.as_str()).collect();
            languages.sort();
            languages.dedup();
            packs.push(json!({
                "id": suite.name, "name": name, "name_hi": name_hi, "badge": badge,
                "summary": summary, "summary_hi": summary_hi,
                "recommended": suite.name == "india-pilot",
                "tasks": suite.tasks.len(), "languages": languages,
                "validation_status": suite.validation_status}));
        }
        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"), "honesty": honesty(),
            "fx": {"usd_to_inr": self.settings.usd_to_inr,
                   "note": "Rupee amounts use the rate stored in the app, not a live bank rate."},
            "providers": providers(), "keys": self.key_status(),
            "models": models, "packs": packs,
            "default_pack": "india-pilot", "default_models": ["oracle", "echo"],
            "default_cap_usd": self.settings.max_usd}))
    }

    fn fx_settings(&self, usd_to_inr: Option<f64>) -> Result<Settings, String> {
        let mut s = self.settings.clone();
        if let Some(rate) = usd_to_inr {
            if !rate.is_finite() || rate <= 0.0 {
                return Err("The rupee rate must be a positive number.".into());
            }
            s.usd_to_inr = rate;
        }
        Ok(s)
    }

    fn estimate(&self, params: &Map<String, Value>) -> Result<Value, String> {
        let suite_name = params.get("suite").and_then(|v| v.as_str()).unwrap_or("");
        let repeats = params.get("repeats").and_then(|v| v.as_i64()).unwrap_or(1);
        let max_usd = params
            .get("max_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let settings = self.fx_settings(params.get("usd_to_inr").and_then(|v| v.as_f64()))?;
        let loaded = self.load_named(suite_name)?;
        let specs = self.clean_specs(params)?;
        if !(1..=MAX_REPEATS).contains(&repeats) {
            return Err(format!("Tries must be between 1 and {MAX_REPEATS}."));
        }
        if !max_usd.is_finite() || max_usd < 0.0 {
            return Err("The spending cap must be zero or more.".into());
        }
        let prices = self.prices();
        let mut built = Vec::new();
        for spec in &specs {
            built.push(self.runner(spec, &prices)?);
        }
        let check = preflight(&loaded.tasks, &built, max_usd, repeats);
        let rate = settings.usd_to_inr;
        let names = self.names.lock().unwrap().clone();
        let mut rows = Vec::new();
        for runner in &built {
            let mine: Vec<_> = check
                .estimates
                .iter()
                .filter(|e| e.runner == runner.name)
                .collect();
            let unknown = mine.iter().any(|e| e.cost_usd.is_none());
            let known: f64 =
                repeats as f64 * mine.iter().map(|e| e.cost_usd.unwrap_or(0.0)).sum::<f64>();
            let problems = runner.problems();
            rows.push(json!({
                "spec": runner.name,
                "label": names.get(&runner.name).cloned().unwrap_or_else(|| runner.name.clone()),
                "attempts": mine.len() as i64 * repeats,
                "worst_case_usd": if unknown { Value::Null } else { json!(known) },
                "worst_case_inr": if unknown { Value::Null } else { json!(known * rate) },
                "ready": problems.is_empty() && !unknown,
                "problems": problems,
                "plain_problems": problems.iter().map(|p| plain_problem(p)).collect::<Vec<_>>()}));
        }
        Ok(json!({
            "ok": check.ok(), "problems": check.problems,
            "plain_problems": check.problems.iter().map(|p| plain_problem(p)).collect::<Vec<_>>(),
            "warnings": check.warnings,
            "worst_case_usd": check.worst_case_usd,
            "worst_case_inr": check.worst_case_usd * rate,
            "cap_usd": max_usd, "cap_inr": max_usd * rate,
            "usd_to_inr": rate, "fx_note": "Configured rate, not a live bank rate.",
            "attempts": loaded.tasks.len() as i64 * built.len() as i64 * repeats,
            "repeats": repeats, "paid": check.worst_case_usd > 0.0,
            "runners": rows, "honesty": honesty(), "pack": loaded.name}))
    }
}

fn model_blurb(model: &str) -> &'static str {
    match model {
        "claude-haiku-4-5" | "claude-haiku-5-5" => "A smaller Claude model. Lower price per token.",
        "claude-sonnet-4-6" | "claude-sonnet-5" | "claude-sonnet-5-5" => "A mid-size Claude model.",
        "claude-opus-4-6" | "claude-opus-4-7" | "claude-opus-4-8" | "claude-opus-5"
        | "claude-opus-5-5" => "A larger Claude model. Higher price per token.",
        _ => "Priced from the checked price table.",
    }
}

fn pack_copy(suite: &Suite) -> (String, String, String, String, String) {
    match suite.name.as_str() {
        "india-pilot" => (
            "India pilot".into(), "भारत पायलट".into(), "Pilot set".into(),
            "Start here. Short synthetic tasks about rupees, records and a missing date. Safe to share. Unvalidated pilot. Not a ranking.".into(),
            "यहीं से शुरू करें। रुपये, रिकॉर्ड और एक गायब तारीख पर छोटे कृत्रिम प्रश्न। बाँटने योग्य। जाँच प्रमाणित नहीं। यह रैंकिंग नहीं है।".into(),
        ),
        "railway-pilot" => (
            "Railway smoke test".into(), "रेलवे जाँच".into(), "Extra fixture".into(),
            "One extra synthetic task. Not a ranking.".into(),
            "एक अतिरिक्त कृत्रिम प्रश्न। यह रैंकिंग नहीं है।".into(),
        ),
        "example" => (
            "Quick example".into(), "छोटा उदाहरण".into(), "Pipeline check".into(),
            "Five tiny tasks to see the pipeline finish. Not a ranking.".into(),
            "पाइपलाइन देखने के लिए पाँच छोटे प्रश्न। यह रैंकिंग नहीं है।".into(),
        ),
        _ => (
            suite.name.clone(), suite.name.clone(), suite.validation_status.clone(),
            suite.description.clone(), suite.description.clone(),
        ),
    }
}

type Emit = Arc<dyn Fn(Value) + Send + Sync>;

impl Session {
    fn run(&self, params: &Map<String, Value>, emit_progress: &Emit) -> Result<Value, String> {
        if params.get("confirmed") != Some(&Value::Bool(true)) {
            return Err(
                "Confirm the cost on the previous screen before running. Nothing was called."
                    .into(),
            );
        }
        let estimate = self.estimate(params)?;
        if estimate["ok"] != Value::Bool(true) {
            let plain: Vec<String> = estimate["plain_problems"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            return Err(if plain.is_empty() {
                "The run was refused. Nothing was called.".into()
            } else {
                plain.join(" ")
            });
        }
        let settings = self.fx_settings(params.get("usd_to_inr").and_then(|v| v.as_f64()))?;
        let loaded = self.load_named(params.get("suite").and_then(|v| v.as_str()).unwrap_or(""))?;
        let prices = self.prices();
        let specs = self.clean_specs(params)?;
        let mut built = Vec::new();
        for spec in &specs {
            built.push(self.runner(spec, &prices)?);
        }
        let titles = titles();
        let total = estimate["attempts"].as_i64().unwrap_or(0);
        if self.names.lock().unwrap().is_empty() {
            let _ = self.catalog();
        }
        {
            let mut busy = self.busy.lock().unwrap();
            if *busy {
                return Err("A check is already running.".into());
            }
            *busy = true;
        }
        self.cancel.store(false, Ordering::SeqCst);
        let result = self.run_inner(
            &loaded,
            &built,
            &settings,
            params,
            total,
            &titles,
            emit_progress,
        );
        *self.busy.lock().unwrap() = false;
        result
    }

    fn run_inner(
        &self,
        loaded: &Suite,
        built: &[Runner],
        settings: &Settings,
        params: &Map<String, Value>,
        total: i64,
        titles: &BTreeMap<&str, &str>,
        emit_progress: &Emit,
    ) -> Result<Value, String> {
        let repeats = params.get("repeats").and_then(|v| v.as_i64()).unwrap_or(1);
        let max_usd = params
            .get("max_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let seed = params.get("seed").and_then(|v| v.as_i64()).unwrap_or(0);
        let names = self.names.lock().unwrap().clone();
        let done = Mutex::new(0i64);
        let on_start = |run_id: &str| {
            emit_progress(json!({"type": "start", "run_id": run_id, "total": total}));
        };
        let on_attempt = |task_id: &str, runner: &str| {
            emit_progress(json!({
                "type": "asking", "task_id": task_id,
                "title": titles.get(task_id).copied().unwrap_or(task_id),
                "runner": runner,
                "runner_label": names.get(runner).cloned().unwrap_or_else(|| runner.to_string()),
                "index": *done.lock().unwrap(), "total": total}));
        };
        let on_result = |record: &ResultRecord| {
            let mut n = done.lock().unwrap();
            *n += 1;
            emit_progress(json!({
                "type": "attempt", "index": *n, "total": total,
                "task_id": record.task_id,
                "title": titles.get(record.task_id.as_str()).copied().unwrap_or(&record.task_id),
                "runner": record.runner,
                "runner_label": names.get(&record.runner).cloned().unwrap_or_else(|| record.runner.clone()),
                "passed": record.passed, "status": record.status,
                "grade_status": record.grade_status, "latency_s": record.latency_s,
                "error": record.error.as_ref().map(|e| self.scrub(e))}));
        };
        let callbacks = RunCallbacks {
            on_start: Some(&on_start),
            on_attempt: Some(&on_attempt),
            on_result: Some(&on_result),
        };
        let store = Store::open(&settings.db_path)?;
        let summary = run_suite(
            loaded,
            built,
            &store,
            settings,
            seed,
            repeats,
            Some(max_usd),
            None,
            &callbacks,
            Some(self.cancel.clone()),
        )
        .map_err(|PreflightError(problems)| {
            let msg = problems.join("; ");
            let scrubbed = self.scrub(&msg);
            if scrubbed.is_empty() {
                "The run was refused.".into()
            } else {
                scrubbed
            }
        })?;
        // The desktop writes results.json here (reports come from Report view).
        let destination = summary.run_dir.join("results.json");
        krutrim_evaluator::export::write_export(&store, &summary.run_id, &destination, false)?;
        drop(store);
        let card = self.share_card(&summary.run_id)?;
        let dashboard = self.dashboard_url(&summary.run_id, "system")?;
        emit_progress(json!({"type": "done", "run_id": summary.run_id,
                             "status": summary.status, "total": total}));
        Ok(json!({
            "run_id": summary.run_id, "status": summary.status,
            "spent_usd": summary.spent_usd,
            "spent_inr": summary.spent_usd * settings.usd_to_inr,
            "export_path": destination.to_string_lossy(),
            "card_path": card["path"], "dashboard_url": dashboard,
            "honesty": honesty()}))
    }

    fn cancel(&self) -> Value {
        self.cancel.store(true, Ordering::SeqCst);
        json!({"cancelling": true})
    }

    fn run_or_raise(&self, run_id: &str) -> Result<(RunRecord, Value), String> {
        if !crate::valid_run_id(run_id) {
            return Err("Unknown run.".into());
        }
        if !self.settings.db_path.is_file() {
            return Err("No results yet.".into());
        }
        let store = Store::open(&self.settings.db_path)?;
        let run = store
            .get_run(run_id)?
            .ok_or_else(|| "That run is not on this computer.".to_string())?;
        let payload = krutrim_evaluator::export::result_export(&store, run_id, false)?.value;
        Ok((run, payload))
    }

    fn decorate(&self, mut payload: Value) -> Value {
        if self.names.lock().unwrap().is_empty() {
            let _ = self.catalog();
        }
        let names = self.names.lock().unwrap().clone();
        if let Some(models) = payload.get_mut("models").and_then(|v| v.as_array_mut()) {
            for model in models.iter_mut() {
                let rid = model["runner_id"].as_str().unwrap_or_default().to_string();
                if model
                    .get("configuration_name")
                    .map(|v| v.is_null())
                    .unwrap_or(true)
                {
                    model["configuration_name"] = json!(names.get(&rid).cloned().unwrap_or(rid));
                }
            }
        }
        payload
    }

    fn report(&self, run_id: &str) -> Result<Value, String> {
        let (_run, payload) = self.run_or_raise(run_id)?;
        Ok(self.decorate(payload))
    }

    fn list_runs(&self) -> Vec<Value> {
        if !self.settings.db_path.is_file() {
            return Vec::new();
        }
        let Ok(store) = Store::open(&self.settings.db_path) else {
            return Vec::new();
        };
        let rate = self.settings.usd_to_inr;
        store
            .list_runs(30)
            .unwrap_or_default()
            .iter()
            .map(|run| {
                json!({"id": run.id, "suite": run.suite_name, "status": run.status,
                       "runners": run.runners, "spent_usd": run.spent_usd,
                       "spent_inr": run.spent_usd * rate, "started_at": run.started_at,
                       "repeats": run.repeats})
            })
            .collect()
    }

    fn share_card(&self, run_id: &str) -> Result<Value, String> {
        let (run, payload) = self.run_or_raise(run_id)?;
        let payload = self.decorate(payload);
        let html = render_share_card(self, &payload, run.spent_usd, run.usd_to_inr);
        let path = PathBuf::from(&run.artifacts_dir).join("share-card.html");
        std::fs::write(&path, &html).map_err(|e| e.to_string())?;
        Ok(json!({"path": path.to_string_lossy(), "html": html,
                  "summary": share_summary(&payload), "honesty": honesty()}))
    }

    fn dashboard_payload(&self, run_id: &str) -> Result<Value, String> {
        let (run, payload) = self.run_or_raise(run_id)?;
        let payload = self.decorate(payload);
        let suite = self.load_named(&run.suite_name)?;
        let titles = titles();
        let tasks: Vec<Value> = suite
            .tasks
            .iter()
            .map(|task| {
                json!({"id": task.id,
                       "title": titles.get(task.id.as_str()).copied().unwrap_or(&task.id),
                       "category": task.category, "language": task.language,
                       "suite": suite.name, "track": task.capability_track,
                       "instruction": "", "context": "", "expected": {},
                       "grading": "Scored on this computer. Questions and answers stay in the run folder.",
                       "full_prompt": "", "prompt_sha256": ""})
            })
            .collect();
        let results = payload["results"].as_array().cloned().unwrap_or_default();
        let count = |f: &dyn Fn(&Value) -> bool| results.iter().filter(|r| f(r)).count();
        let job = json!({
            "id": run.id,
            "status": if run.status == "completed" { "completed".to_string() } else { run.status.clone() },
            "processed": results.len(), "total": results.len(),
            "graded": count(&|r| r["grade_status"] == "graded"),
            "passed": count(&|r| r["grade_status"] == "graded" && r["passed"] == Value::Bool(true)),
            "execution_errors": count(&|r| r["status"] == "error"),
            "grader_errors": count(&|r| r["grade_status"] == "error"),
            "pending": count(&|r| r["grade_status"] == "pending"),
            "skipped": count(&|r| r["status"] == "skipped"),
            "events": [], "error": null, "run_id": run.id,
            "created_at": run.started_at,
            "request": {"adapter_ids": run.runners, "suite": run.suite_name,
                        "repeats": run.repeats}});
        let adapters: Vec<Value> = payload["models"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|model| {
                json!({"id": format!("desktop:{}", model["runner_id"].as_str().unwrap_or_default()),
                       "name": model.get("configuration_name").and_then(|v| v.as_str())
                           .unwrap_or_else(|| model["runner_id"].as_str().unwrap_or_default()),
                       "ready": true,
                       "track": model.get("evaluation_track").and_then(|v| v.as_str()).unwrap_or("unverified"),
                       "cost": "", "reason": ""})
            })
            .collect();
        Ok(
            json!({"tasks": tasks, "capabilities": {"adapters": adapters},
                  "job": job, "export": payload, "runs": self.list_runs()}),
        )
    }

    fn dashboard_url(&self, run_id: &str, theme: &str) -> Result<String, String> {
        let theme = if ["system", "light", "dark"].contains(&theme) {
            theme
        } else {
            "system"
        };
        if !crate::valid_run_id(run_id) {
            return Err("Unknown run.".into());
        }
        // Bind before matching: a scrutinee guard would live across the
        // None arm and deadlock when ensure_dashboard_server re-locks it.
        let external = self.external_base.lock().unwrap().clone();
        let base = match external {
            Some(b) => b,
            None => {
                self.ensure_dashboard_server();
                format!(
                    "http://127.0.0.1:{}",
                    self.http_port.lock().unwrap().unwrap_or(0)
                )
            }
        };
        Ok(format!(
            "{base}/board/{}/?run={run_id}&theme={theme}",
            self.token
        ))
    }

    fn ensure_dashboard_server(&self) {
        if self.http_port.lock().unwrap().is_some() {
            return;
        }
        if let Some(port) = serve_async(Arc::new(self.sibling()), 0, false) {
            *self.http_port.lock().unwrap() = Some(port);
        }
    }
}

fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 81
        && id
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn render_share_card(
    session: &Session,
    payload: &Value,
    spent_usd: f64,
    usd_to_inr: f64,
) -> String {
    let logo = session
        .preview_dir
        .join("brand")
        .join("km-chakra-wordmark.svg");
    let mark = std::fs::read_to_string(&logo)
        .ok()
        .filter(|raw| !raw.to_lowercase().contains("<script"))
        .unwrap_or_default();
    let mut by_runner: BTreeMap<String, (i64, i64, Vec<f64>)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for row in payload["results"].as_array().cloned().unwrap_or_default() {
        let rid = row["runner_id"].as_str().unwrap_or_default().to_string();
        if !by_runner.contains_key(&rid) {
            order.push(rid.clone());
        }
        let bucket = by_runner.entry(rid).or_insert((0, 0, Vec::new()));
        if row["grade_status"] == "graded" {
            bucket.1 += 1;
            if row["passed"] == Value::Bool(true) {
                bucket.0 += 1;
            }
        }
        if let Some(lat) = row["latency_s"].as_f64() {
            bucket.2.push(lat);
        }
    }
    let names: BTreeMap<String, String> = payload["models"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            (
                m["runner_id"].as_str().unwrap_or_default().to_string(),
                m.get("configuration_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| m["runner_id"].as_str().unwrap_or_default().to_string()),
            )
        })
        .collect();
    let mut rows_html = String::new();
    for rid in &order {
        let (right, graded, waits) = &by_runner[rid];
        let mut sorted = waits.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let wait = sorted
            .get(sorted.len() / 2)
            .map(|m| format!("{m:.2} s"))
            .unwrap_or_else(|| "not measured".into());
        rows_html += &format!(
            "<tr><td>{}</td><td>{right} / {graded}</td><td>{wait}</td></tr>",
            esc(names.get(rid).map(|s| s.as_str()).unwrap_or(rid))
        );
    }
    let mut verdicts_html = String::new();
    for v in payload
        .get("verdicts")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let label = if v["verdict"] == "clearly_different" {
            "Clearly different"
        } else {
            "Too close to call"
        };
        let left = names
            .get(v["runner_a"].as_str().unwrap_or_default())
            .cloned()
            .unwrap_or_default();
        let right = names
            .get(v["runner_b"].as_str().unwrap_or_default())
            .cloned()
            .unwrap_or_default();
        verdicts_html += &format!(
            "<li>{} · {}: {}</li>",
            esc(&format!("{left} vs {right}")),
            esc(v["metric"].as_str().unwrap_or_default()),
            esc(label)
        );
    }
    if verdicts_html.is_empty() {
        verdicts_html =
            "<li>One system was checked, so there is no comparison and no ranking.</li>".into();
    }
    let spent = format!("${spent_usd:.4} (₹{:.2})", spent_usd * usd_to_inr);
    let title = esc(payload
        .pointer("/benchmark/name")
        .and_then(|v| v.as_str())
        .unwrap_or("Run"));
    format!(
        "<!doctype html>\n<html lang=\"en-IN\"><head><meta charset=\"utf-8\"><title>Krutrim Mulyankan · {title}</title>\n<style>\nbody{{margin:0;background:#F3F3F1;color:#171717;\nfont:15px/1.5 \"Sora\",\"Noto Sans Devanagari\",sans-serif}}\nmain{{max-width:720px;margin:32px auto;padding:28px;background:#fff;border:1px solid #D8D8D5}}\n.logo svg{{width:240px;height:auto}}\n.chips span{{display:inline-block;margin:0 8px 8px 0;padding:6px 8px;border:1px solid #171717;\nfont-size:11px;letter-spacing:.08em;text-transform:uppercase}}\ntable{{border-collapse:collapse;width:100%;margin:16px 0}}\ntd,th{{text-align:left;padding:8px;border-bottom:1px solid #D8D8D5}}\n</style></head><body><main>\n<div class=\"logo\">{mark}</div>\n<p>Krutrim Mulyankan · result card</p>\n<div class=\"chips\"><span>Unvalidated pilot</span>\n<span>Not a ranking</span><span>Too close to call</span></div>\n<h1>{title}</h1>\n<p>Too close to call means the gap could be chance, not that they are equal.</p>\n<table><tr><th>System</th><th>Right</th><th>Typical wait</th></tr>{rows_html}</table>\n<ul>{verdicts_html}</ul>\n<p>Known spend {spent}. This is not a model ranking. Questions and answers are not on this card.</p>\n</main></body></html>\n"
    )
}

fn share_summary(payload: &Value) -> String {
    let name = payload
        .pointer("/benchmark/name")
        .and_then(|v| v.as_str())
        .unwrap_or("a run");
    format!(
        "Krutrim Mulyankan · {name}\nUnvalidated pilot\nNot a ranking\nToo close to call means the gap could be chance, not that they are equal."
    )
}

fn esc(s: &str) -> String {
    krutrim_evaluator::report::esc(s)
}

impl Session {
    /// A read-mostly twin used by the dashboard HTTP server thread. It shares
    /// settings, token and the env snapshot; runs started over the dashboard
    /// server share the same database and cancel flag.
    fn sibling(&self) -> Session {
        Session {
            settings: self.settings.clone(),
            env: Mutex::new(self.env_snapshot()),
            root: self.root.clone(),
            preview_dir: self.preview_dir.clone(),
            cancel: self.cancel.clone(),
            busy: Arc::new(Mutex::new(false)),
            token: self.token.clone(),
            http_port: Mutex::new(*self.http_port.lock().unwrap()),
            external_base: Mutex::new(self.external_base.lock().unwrap().clone()),
            names: Mutex::new(self.names.lock().unwrap().clone()),
        }
    }
}

fn render_dashboard_html(session: &Session, run_id: &str) -> Result<String, String> {
    let path = session.preview_dir.join("index.html");
    let html = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    const SCRIPT_TAG: &str = "<script src=\"preview-fixtures.js\" defer></script>";
    if !html.contains(SCRIPT_TAG) {
        return Err("preview/index.html no longer has the script tag the desktop app attaches to. The file was not modified.".into());
    }
    let payload = serde_json::to_string(&session.dashboard_payload(run_id)?)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    let theme_boot = "<script>try{var t=new URLSearchParams(location.search).get('theme');\
if(t==='light'||t==='dark')localStorage.setItem('km.theme.v1',t);}catch(e){}</script>";
    let html = html.replacen(
        "<script src=\"theme.js\"></script>",
        &format!("{theme_boot}<script src=\"theme.js\"></script>"),
        1,
    );
    let html = html.replacen(
        SCRIPT_TAG,
        &format!("{SCRIPT_TAG}<script src=\"/desktop/dashboard-shim.js\" defer></script>"),
        1,
    );
    let html = html.replacen(
        "<head>",
        &format!("<head><base href=\"/board/{}/\">", session.token),
        1,
    );
    let html = html.replacen(
        "</head>",
        &format!("<script>window.__KM_DASHBOARD__={payload};</script></head>"),
        1,
    );
    Ok(html)
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.starts_with('/') {
        return None;
    }
    let rel = Path::new(relative);
    if rel.components().any(|c| c.as_os_str() == "..") {
        return None;
    }
    let candidate = root.join(rel);
    let real = candidate.canonicalize().ok()?;
    let root_real = root.canonicalize().ok()?;
    if real.starts_with(&root_real) && real.is_file() {
        Some(real)
    } else {
        None
    }
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

const SHELL_FILES: &[&str] = &[
    "index.html",
    "app.js",
    "app.css",
    "theme.js",
    "i18n.js",
    "bridge.js",
    "dashboard-shim.js",
];
const PREVIEW_FILES: &[(&str, &str)] = &[
    ("style.css", "text/css; charset=utf-8"),
    ("theme.js", "text/javascript; charset=utf-8"),
    ("app.js", "text/javascript; charset=utf-8"),
    ("showcase.js", "text/javascript; charset=utf-8"),
    ("compare.js", "text/javascript; charset=utf-8"),
    ("jobs.js", "text/javascript; charset=utf-8"),
    ("preview-fixtures.js", "text/javascript; charset=utf-8"),
];

struct Request {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    if reader.read_line(&mut first).ok()? == 0 {
        return None;
    }
    let parts: Vec<&str> = first.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let (method, target) = (parts[0].to_string(), parts[1].to_string());
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    let (path, query_raw) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut query = BTreeMap::new();
    for pair in query_raw.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(percent_decode(k), percent_decode(v));
    }
    Some(Request {
        method,
        path: percent_decode(&path),
        query,
        body,
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn send(stream: &mut std::net::TcpStream, status: u16, body: &[u8], ctype: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Content Too Large",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.0 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn handle_http(
    session: &Arc<Session>,
    shell: bool,
    req: Request,
    stream: &mut std::net::TcpStream,
) {
    match req.method.as_str() {
        "POST" => {
            if req.path != "/rpc" {
                send(stream, 404, b"Not found", "text/plain; charset=utf-8");
                return;
            }
            if req.body.len() > 262_144 {
                send(
                    stream,
                    413,
                    b"Message is too large.",
                    "text/plain; charset=utf-8",
                );
                return;
            }
            let Ok(request) = serde_json::from_slice::<Value>(&req.body) else {
                send(
                    stream,
                    400,
                    b"The message could not be read.",
                    "text/plain; charset=utf-8",
                );
                return;
            };
            let _ = stream.write_all(
                b"HTTP/1.0 200 OK\r\nContent-Type: application/x-ndjson; charset=utf-8\r\nCache-Control: no-store\r\n\r\n",
            );
            let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
            let emit: Emit = Arc::new(move |msg: Value| {
                let line = serde_json::to_string(&msg).unwrap_or_default() + "\n";
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(line.as_bytes());
                let _ = w.flush();
            });
            dispatch(session, &request, &emit);
        }
        "GET" => {
            let path = req.path.as_str();
            let ui = ui_dir(&session.root);
            if shell && (path == "/" || path == "/index.html") {
                return send_file(stream, &ui.join("index.html"), "text/html; charset=utf-8");
            }
            if shell && path.starts_with("/brand/") {
                return match safe_join(&ui, &path[1..]) {
                    Some(t) => send_file(stream, &t, mime(&t)),
                    None => send(stream, 404, b"Not found", "text/plain; charset=utf-8"),
                };
            }
            if shell && SHELL_FILES.contains(&path.trim_start_matches('/')) && path != "/index.html"
            {
                let name = path.trim_start_matches('/');
                return send_file(stream, &ui.join(name), mime(Path::new(name)));
            }
            if path == "/desktop/dashboard-shim.js" {
                return send_file(
                    stream,
                    &ui.join("dashboard-shim.js"),
                    "text/javascript; charset=utf-8",
                );
            }
            if let Some(rest) = path.strip_prefix("/board/") {
                let (token, relative) = match rest.split_once('/') {
                    Some((t, r)) => (t, r),
                    None => (rest, ""),
                };
                if !constant_time_eq(token, &session.token) {
                    return send(
                        stream,
                        403,
                        b"Open this page from the app.",
                        "text/plain; charset=utf-8",
                    );
                }
                if relative.is_empty() || relative == "index.html" {
                    let run_id = req.query.get("run").cloned().unwrap_or_default();
                    return match render_dashboard_html(session, &run_id) {
                        Ok(html) => send(stream, 200, html.as_bytes(), "text/html; charset=utf-8"),
                        Err(e) => {
                            let message = session.scrub(&e);
                            let message = if message.is_empty() {
                                "Results are unavailable.".into()
                            } else {
                                message
                            };
                            send(stream, 400, message.as_bytes(), "text/plain; charset=utf-8")
                        }
                    };
                }
                if let Some((_, ctype)) = PREVIEW_FILES.iter().find(|(name, _)| *name == relative) {
                    return send_file(stream, &session.preview_dir.join(relative), ctype);
                }
                return match safe_join(&session.preview_dir, relative) {
                    Some(t) => {
                        let brand = session.preview_dir.join("brand");
                        let ok = brand
                            .canonicalize()
                            .map(|b| t.starts_with(b))
                            .unwrap_or(false);
                        if ok {
                            send_file(stream, &t, mime(&t))
                        } else {
                            send(stream, 404, b"Not found", "text/plain; charset=utf-8")
                        }
                    }
                    None => send(stream, 404, b"Not found", "text/plain; charset=utf-8"),
                };
            }
            send(stream, 404, b"Not found", "text/plain; charset=utf-8");
        }
        _ => send(stream, 404, b"Not found", "text/plain; charset=utf-8"),
    }
}

fn send_file(stream: &mut std::net::TcpStream, path: &Path, ctype: &str) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let kind = if path.extension().and_then(|s| s.to_str()) == Some("woff2") {
                "font/woff2"
            } else {
                ctype
            };
            send(stream, 200, &bytes, kind);
        }
        Err(_) => send(stream, 404, b"Not found", "text/plain; charset=utf-8"),
    }
}

fn serve_async(session: Arc<Session>, port: u16, shell: bool) -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port)).ok()?;
    let bound = listener.local_addr().ok()?.port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let session = session.clone();
            std::thread::spawn(move || {
                if let Some(req) = read_request(&mut stream) {
                    handle_http(&session, shell, req, &mut stream);
                }
            });
        }
    });
    Some(bound)
}

fn dispatch(session: &Arc<Session>, request: &Value, emit: &Emit) {
    let ident = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let empty = Map::new();
    let params = request
        .get("params")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);
    let run_id_param = || {
        params
            .get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let result: Result<Value, String> = (|| {
        Ok(match method {
            "ping" => session.ping(),
            "catalog" => session.catalog()?,
            "key_status" => session.key_status(),
            "set_keys" => session.set_keys(
                params
                    .get("keys")
                    .and_then(|v| v.as_object())
                    .unwrap_or(&empty),
            )?,
            "set_urls" => session.set_urls(
                params
                    .get("urls")
                    .and_then(|v| v.as_object())
                    .unwrap_or(&empty),
            )?,
            "estimate" => session.estimate(params)?,
            "run" => {
                let ident2 = ident.clone();
                let emit2 = emit.clone();
                let on_event: Emit = Arc::new(move |event: Value| {
                    emit2(json!({"id": ident2, "event": "progress", "result": event}));
                });
                session.run(params, &on_event)?
            }
            "cancel" => session.cancel(),
            "report" => session.report(&run_id_param())?,
            "list_runs" => Value::Array(session.list_runs()),
            "share_card" => session.share_card(&run_id_param())?,
            "dashboard_url" => json!({
                "url": session.dashboard_url(
                    &run_id_param(),
                    params.get("theme").and_then(|v| v.as_str()).unwrap_or("system"),
                )?}),
            _ => return Err("Unknown action.".into()),
        })
    })();
    match result {
        Ok(result) => emit(json!({"id": ident, "ok": true, "result": result})),
        Err(e) => {
            let message = session.scrub(&e);
            emit(json!({"id": ident, "ok": false,
                        "error": if message.is_empty() { "Something went wrong.".into() } else { message }}));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("--http") {
        let port: u16 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        let session = Arc::new(Session::new());
        if let Some(bound) = serve_async(session.clone(), port, true) {
            *session.external_base.lock().unwrap() = Some(format!("http://127.0.0.1:{bound}"));
            eprintln!("Krutrim Mulyankan desktop http://127.0.0.1:{bound}/");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        return;
    }
    let session = Arc::new(Session::new());
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let emit: Emit = Arc::new(move |msg: Value| {
        let line = serde_json::to_string(&msg).unwrap_or_default() + "\n";
        let mut w = stdout.lock().unwrap();
        let _ = w.write_all(line.as_bytes());
        let _ = w.flush();
    });
    let mut run_threads = Vec::new();
    let stdin = std::io::stdin();
    for raw in stdin.lock().lines() {
        let Ok(raw) = raw else { break };
        if raw.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&raw) else {
            emit(json!({"id": null, "ok": false, "error": "The message could not be read."}));
            continue;
        };
        if !request.is_object() {
            emit(json!({"id": null, "ok": false, "error": "The message could not be read."}));
            continue;
        }
        if request.get("method").and_then(|v| v.as_str()) == Some("run") {
            let (session2, emit2) = (session.clone(), emit.clone());
            run_threads.push(std::thread::spawn(move || {
                dispatch(&session2, &request, &emit2)
            }));
        } else {
            dispatch(&session, &request, &emit);
        }
    }
    // stdin closed: let an in-flight run finish writing its reply first.
    for handle in run_threads {
        let _ = handle.join();
    }
}
