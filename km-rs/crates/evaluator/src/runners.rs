//! Runners, mirroring runners/. Only the free/local runners (echo, oracle)
//! execute in the Rust build for now; model/harness/agent specs are refused
//! at preflight with a clear problem, so nothing is ever called or spent.

use crate::pricing::PriceTable;
use crate::suite::{GraderSpec, Task};
use serde_json::{json, Map, Value};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct RunOutput {
    pub output: String,
    pub usage: Usage,
    pub steps: Option<i64>,
    pub tool_calls: Option<i64>,
    pub error: Option<String>,
    pub raw: Map<String, Value>,
}

impl RunOutput {
    pub fn error(message: String) -> RunOutput {
        RunOutput {
            error: Some(message),
            output: String::new(),
            ..Default::default()
        }
    }

    pub fn dump_json(&self) -> Value {
        json!({
            "output": self.output,
            "usage": {
                "input_tokens": self.usage.input_tokens,
                "output_tokens": self.usage.output_tokens,
                "cost_usd": self.usage.cost_usd,
            },
            "steps": self.steps,
            "tool_calls": self.tool_calls,
            "error": self.error,
            "raw": self.raw,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerKind {
    Fake,
    Model,
    Harness,
    Agent,
}

impl RunnerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunnerKind::Fake => "fake",
            RunnerKind::Model => "model",
            RunnerKind::Harness => "harness",
            RunnerKind::Agent => "agent",
        }
    }
}

pub struct Runner {
    pub name: String,
    pub kind: RunnerKind,
    pub fake: Option<FakeKind>,
    pub unported: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FakeKind {
    Echo,
    Oracle,
}

pub fn approx_tokens(text: &str) -> i64 {
    if text.is_empty() {
        0
    } else {
        // Python len() counts code points, not bytes.
        ((text.chars().count() / 4) as i64).max(1)
    }
}

impl Runner {
    fn fake(kind: FakeKind) -> Runner {
        Runner {
            name: match kind {
                FakeKind::Echo => "echo".into(),
                FakeKind::Oracle => "oracle".into(),
            },
            kind: RunnerKind::Fake,
            fake: Some(kind),
            unported: None,
        }
    }

    pub fn problems(&self) -> Vec<String> {
        match &self.unported {
            Some(msg) => vec![msg.clone()],
            None => Vec::new(),
        }
    }

    pub fn estimate_cost_usd(&self, _task: &Task, _prompt: &str) -> Option<f64> {
        match self.fake {
            Some(_) => Some(0.0),
            None => None,
        }
    }

    pub fn run(&self, task: &Task, prompt: &str) -> RunOutput {
        match self.fake {
            Some(FakeKind::Echo) => free_output(prompt, prompt),
            Some(FakeKind::Oracle) => {
                let answer = match &task.grader {
                    GraderSpec::Exact { expected, .. } => expected[0].clone(),
                    GraderSpec::Json { expected } => py_json_compact(expected),
                    GraderSpec::Numeric { expected_raw, .. } => expected_raw.clone(),
                    _ => task.reference.clone().unwrap_or_default(),
                };
                free_output(prompt, &answer)
            }
            None => RunOutput::error(
                self.unported
                    .clone()
                    .unwrap_or_else(|| "runner is not implemented".into()),
            ),
        }
    }

    pub fn configuration(&self) -> Map<String, Value> {
        let track = match self.kind {
            RunnerKind::Fake => "mock_validation",
            RunnerKind::Model => "model_api",
            RunnerKind::Harness => "harness",
            RunnerKind::Agent => "personal_agent",
        };
        let mut m = Map::new();
        m.insert("runner_id".into(), json!(self.name));
        m.insert("kind".into(), json!(self.kind.as_str()));
        m.insert(
            "evaluation_mode".into(),
            json!(if self.kind == RunnerKind::Fake {
                "mock"
            } else {
                "live"
            }),
        );
        m.insert("framework".into(), json!("kritrim_mulyankan"));
        m.insert("framework_version".into(), json!(VERSION));
        m.insert("evaluation_track".into(), json!(track));
        m.insert("tools".into(), json!([]));
        m.insert("system_prompt".into(), Value::Null);
        m
    }
}

fn free_output(prompt: &str, answer: &str) -> RunOutput {
    RunOutput {
        output: answer.to_string(),
        usage: Usage {
            input_tokens: Some(approx_tokens(prompt)),
            output_tokens: Some(approx_tokens(answer)),
            cost_usd: Some(0.0),
        },
        steps: Some(1),
        tool_calls: Some(0),
        error: None,
        raw: Map::new(),
    }
}

/// json.dumps(value, ensure_ascii=False) with default (compact-ish) separators
/// (", ", ": ") — what Python's OracleRunner replies for json tasks.
fn py_json_compact(v: &Value) -> String {
    crate::pypy::py_json_dumps(v)
}

/// (spec, kind, implemented, description), shown by `km list runners`.
pub const RUNNER_CATALOG: &[(&str, &str, bool, &str)] = &[
    (
        "echo",
        "fake",
        true,
        "Replies with the prompt. Free; a floor score.",
    ),
    (
        "oracle",
        "fake",
        true,
        "Replies with the expected/reference answer. Free; checks graders.",
    ),
    (
        "model:<provider>/<model>[,k=v]",
        "model",
        true,
        "Direct API call. Providers: anthropic, openai, xai, gemini, openrouter, ollama, \
vllm, compatible. Options: effort, max_tokens, temperature, retries.",
    ),
    (
        "harness:<cli>",
        "harness",
        false,
        "Headless coding/agent CLI in a temp workdir.",
    ),
    (
        "agent:<name>",
        "agent",
        false,
        "Personal agent via imported transcripts.",
    ),
];

pub fn make_runner(spec: &str, _prices: &PriceTable) -> Result<Runner, String> {
    match spec.split_once(':') {
        None => match spec {
            "echo" => Ok(Runner::fake(FakeKind::Echo)),
            "oracle" => Ok(Runner::fake(FakeKind::Oracle)),
            _ => Err(format!("unknown runner '{spec}'; see `km list runners`")),
        },
        Some((kind, arg)) if !arg.is_empty() => {
            let (rkind, message) = match kind {
                "model" => (
                    RunnerKind::Model,
                    "model runners are not yet ported to the Rust km; use the Python km for paid runs (nothing was called)".to_string(),
                ),
                "harness" => (RunnerKind::Harness, "HarnessRunner is not implemented yet".to_string()),
                "agent" => (RunnerKind::Agent, "AgentRunner is not implemented yet".to_string()),
                _ => return Err(format!("unknown runner '{spec}'; see `km list runners`")),
            };
            // The runner name is the full spec, as in Python (ModelRunner
            // names itself spec-style; the stubs use harness:<cli>/agent:<n>).
            let name = match kind {
                "model" => format!("model:{arg}"),
                other => format!("{other}:{arg}"),
            };
            Ok(Runner {
                name,
                kind: rkind,
                fake: None,
                unported: Some(message),
            })
        }
        _ => Err(format!("unknown runner '{spec}'; see `km list runners`")),
    }
}
