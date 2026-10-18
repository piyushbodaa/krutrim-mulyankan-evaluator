//! Parse the user's own chat history exports into prompt/answer pairs.
//! Mirrors `history.py`. Local only; no network. Supported: ChatGPT data
//! export (zip or conversations.json), Claude data export (zip or JSON),
//! and Claude Code `*.jsonl` sessions (file or directory).

use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PromptPair {
    pub source: String, // chatgpt | claude | claude_code
    pub conversation_id: String,
    pub conversation_title: String,
    pub turn_index: i64,
    pub created_at: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
    pub answer: String,
}

#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub source: String,
    pub id: String,
    pub title: String,
    pub created_at: Option<String>,
    /// (role, text, model); role is "user" or "assistant".
    pub turns: Vec<(String, String, Option<String>)>,
}

impl Conversation {
    pub fn pairs(&self) -> Vec<PromptPair> {
        let mut pairs = Vec::new();
        let mut pending: Option<(i64, String)> = None;
        let mut turn_index: i64 = -1;
        for (role, text, model) in &self.turns {
            if role == "user" {
                turn_index += 1;
                pending = Some((turn_index, text.clone()));
            } else if let Some((index, prompt)) = pending.take() {
                if !text.trim().is_empty() {
                    pairs.push(PromptPair {
                        source: self.source.clone(),
                        conversation_id: self.id.clone(),
                        conversation_title: self.title.clone(),
                        turn_index: index,
                        created_at: self.created_at.clone(),
                        model: model.clone(),
                        prompt,
                        answer: text.clone(),
                    });
                } else {
                    pending = Some((index, prompt));
                }
            }
        }
        pairs
    }
}

#[derive(Debug, Default)]
pub struct History {
    pub conversations: Vec<Conversation>,
}

impl History {
    pub fn pairs(&self) -> Vec<PromptPair> {
        self.conversations.iter().flat_map(|c| c.pairs()).collect()
    }

    pub fn by_source(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for conv in &self.conversations {
            match counts.iter_mut().find(|(s, _)| *s == conv.source) {
                Some((_, n)) => *n += 1,
                None => counts.push((conv.source.clone(), 1)),
            }
        }
        counts.sort();
        counts
    }
}

/// Flatten the many `content` shapes used across exports into plain text.
fn text_blocks(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.clone()),
                Value::Object(map) => {
                    let kind = map.get("type").and_then(|v| v.as_str());
                    if kind == Some("text") || kind.is_none() {
                        map.get("text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .or_else(|| {
                                map.get("content")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                            })
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(map)) => {
            if let Some(parts) = map.get("parts") {
                return text_blocks(Some(parts));
            }
            map.get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
        _ => String::new(),
    }
}

fn load_json(path: &Path) -> Result<Value, String> {
    if path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
    {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        let pick = names
            .iter()
            .find(|n| n.to_lowercase().ends_with("conversations.json"))
            .or_else(|| names.iter().find(|n| n.to_lowercase().ends_with(".json")))
            .cloned()
            .ok_or_else(|| format!("{}: no JSON file inside this export zip", path.display()))?;
        let mut text = String::new();
        archive
            .by_name(&pick)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        return serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn as_conversation_list(data: &Value, path: &Path, what: &str) -> Result<Vec<Value>, String> {
    let list = match data {
        Value::Array(items) => Some(items.clone()),
        Value::Object(map) => map.get("conversations").and_then(|v| v.as_array()).cloned(),
        _ => None,
    };
    list.ok_or_else(|| {
        format!(
            "{}: not a {what} export (expected a list of conversations)",
            path.display()
        )
    })
}

/// ChatGPT export: follow `current_node`'s parent chain (the kept branch).
pub fn parse_chatgpt(path: &Path) -> Result<Vec<Conversation>, String> {
    let data = load_json(path)?;
    let raw_list = as_conversation_list(&data, path, "ChatGPT")?;
    let mut out = Vec::new();
    for raw in &raw_list {
        let Some(mapping) = raw.get("mapping").and_then(|v| v.as_object()) else {
            continue;
        };
        let mut chain: Vec<&Value> = Vec::new();
        let mut node_id = raw
            .get("current_node")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(id) = node_id.clone() {
            let Some(node) = mapping.get(&id) else { break };
            if !seen.insert(id) {
                break;
            }
            chain.push(node);
            node_id = node
                .get("parent")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
        if chain.is_empty() {
            let mut nodes: Vec<&Value> = mapping.values().collect();
            nodes.sort_by(|a, b| {
                let key = |n: &Value| {
                    n.get("message")
                        .and_then(|m| m.get("create_time"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0)
                };
                key(a)
                    .partial_cmp(&key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            chain = nodes;
        } else {
            chain.reverse();
        }
        let mut conv = Conversation {
            source: "chatgpt".into(),
            id: raw
                .get("conversation_id")
                .or_else(|| raw.get("id"))
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| format!("chatgpt-{}", out.len())),
            title: raw
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            created_at: raw.get("create_time").map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
            turns: Vec::new(),
        };
        for node in chain {
            let Some(message) = node.get("message").filter(|m| m.is_object()) else {
                continue;
            };
            let author = message
                .get("author")
                .and_then(|a| a.get("role"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if author != "user" && author != "assistant" {
                continue;
            }
            let hidden = message
                .get("metadata")
                .and_then(|m| m.get("is_visually_hidden_from_conversation"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if hidden {
                continue;
            }
            let text = text_blocks(message.get("content")).trim().to_string();
            let model = message
                .get("metadata")
                .and_then(|m| m.get("model_slug"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if !text.is_empty() {
                conv.turns.push((author.to_string(), text, model));
            }
        }
        if !conv.pairs().is_empty() {
            out.push(conv);
        }
    }
    Ok(out)
}

/// Claude.ai export: `chat_messages` with human/assistant senders.
pub fn parse_claude(path: &Path) -> Result<Vec<Conversation>, String> {
    let data = load_json(path)?;
    let raw_list = as_conversation_list(&data, path, "Claude")?;
    let mut out = Vec::new();
    for raw in &raw_list {
        let mut conv = Conversation {
            source: "claude".into(),
            id: raw
                .get("uuid")
                .or_else(|| raw.get("id"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("claude-{}", out.len())),
            title: raw
                .get("name")
                .or_else(|| raw.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            created_at: raw
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            turns: Vec::new(),
        };
        for message in raw
            .get("chat_messages")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let sender = message
                .get("sender")
                .or_else(|| message.get("role"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let role = match sender {
                "human" | "user" => "user",
                "assistant" => "assistant",
                _ => continue,
            };
            let mut text = text_blocks(message.get("content")).trim().to_string();
            if text.is_empty() {
                if let Some(t) = message.get("text").and_then(|v| v.as_str()) {
                    text = t.trim().to_string();
                }
            }
            let model = message
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if !text.is_empty() {
                conv.turns.push((role.to_string(), text, model));
            }
        }
        if !conv.pairs().is_empty() {
            out.push(conv);
        }
    }
    Ok(out)
}

/// Claude Code `*.jsonl` sessions; a directory is searched recursively.
pub fn parse_claude_code(path: &Path) -> Result<Vec<Conversation>, String> {
    let mut files: Vec<PathBuf> = Vec::new();
    if path.is_dir() {
        collect_jsonl(path, &mut files);
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    let mut out = Vec::new();
    for file in &files {
        let mut conv = Conversation {
            source: "claude_code".into(),
            id: file
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            title: String::new(),
            created_at: None,
            turns: Vec::new(),
        };
        // Stream line by line: session files run to hundreds of MB, most of it
        // tool results we drop anyway. Those lines are skipped before the
        // (expensive) full JSON parse; writers that space their JSON take the
        // slow path through the normal checks below instead.
        let raw = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut reader = std::io::BufReader::new(raw);
        let mut line = String::new();
        loop {
            line.clear();
            let n = std::io::BufRead::read_line(&mut reader, &mut line)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            if n == 0 {
                break;
            }
            let line = line.trim();
            if line.is_empty()
                || line.contains("\"type\":\"tool_result\"")
                || line.contains("\"type\": \"tool_result\"")
                || (!line.contains("user") && !line.contains("assistant"))
            {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if event
                .get("isMeta")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let kind = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "user" && kind != "assistant" {
                continue;
            }
            let message = event.get("message").cloned().unwrap_or(Value::Null);
            let content = message.get("content").cloned().unwrap_or(Value::Null);
            let is_tool_result = content
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_result"))
                })
                .unwrap_or(false);
            if is_tool_result {
                continue;
            }
            let text = text_blocks(Some(&content)).trim().to_string();
            if text.is_empty()
                || text.starts_with("<command-name>")
                || text.starts_with("<local-command-")
                || text.starts_with("Caveat:")
            {
                continue;
            }
            if conv.created_at.is_none() {
                conv.created_at = event
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            let model = message
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            conv.turns.push((kind.to_string(), text, model));
        }
        if !conv.pairs().is_empty() {
            out.push(conv);
        }
    }
    Ok(out)
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// Load one or more exports, auto-detecting each format by content.
pub fn load_history(paths: &[PathBuf], source: &str) -> Result<History, String> {
    let mut conversations = Vec::new();
    for path in paths {
        if !path.exists() {
            return Err(format!("no such file or directory: {}", path.display()));
        }
        match source {
            "chatgpt" => conversations.extend(parse_chatgpt(path)?),
            "claude" => conversations.extend(parse_claude(path)?),
            "claude_code" | "claude-code" => conversations.extend(parse_claude_code(path)?),
            _ => {
                if path.is_dir()
                    || path
                        .extension()
                        .map(|e| e.eq_ignore_ascii_case("jsonl"))
                        .unwrap_or(false)
                {
                    conversations.extend(parse_claude_code(path)?);
                } else {
                    let probe = load_json(path)?;
                    conversations.extend(if looks_like_chatgpt(&probe) {
                        parse_chatgpt(path)?
                    } else {
                        parse_claude(path)?
                    });
                }
            }
        }
    }
    Ok(History { conversations })
}

fn looks_like_chatgpt(data: &Value) -> bool {
    let items = match data {
        Value::Array(items) => Some(items),
        Value::Object(map) => map.get("conversations").and_then(|v| v.as_array()),
        _ => None,
    };
    items
        .and_then(|items| items.first())
        .map(|first| first.get("mapping").is_some())
        .unwrap_or(false)
}
