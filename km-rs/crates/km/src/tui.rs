//! Interactive terminal front-end for bare `km` on a TTY. It mirrors the
//! desktop app: the same three steps (questions, AIs, start), the same
//! words and rupees, one square per question on the report card, and the
//! same "too close to call" comparisons. Non-TTY behaviour never reaches
//! this module.

use crate::keys;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use krutrim_evaluator::*;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph, Wrap};
use ratatui::Terminal;
use std::collections::BTreeMap;
use std::io::Stdout;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

pub enum Outcome {
    Quit,
    OpenUi,
}

pub struct UiOptions {
    pub animate: bool,
    pub color: bool,
    pub help_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Menu,
    Suites,
    Runners,
    Confirm,
    Running,
    Results,
    Keys,
    KeyEdit,
    Help,
}

// Monochrome, the app's dark theme: carbon paper, a raised surface, ink and
// two greys. NO_COLOR falls back to bold, dim and reverse video.
const PAPER: Color = Color::Rgb(0x17, 0x17, 0x17);
const SURFACE: Color = Color::Rgb(0x24, 0x24, 0x22);
const INK: Color = Color::Rgb(0xF3, 0xF3, 0xF1);
const MUTED: Color = Color::Rgb(0xBE, 0xBE, 0xBA);
const FAINT: Color = Color::Rgb(0x8A, 0x8A, 0x86);
const LINE: Color = Color::Rgb(0x41, 0x41, 0x3E);

/// Same limit as the app: a few AIs compare well side by side.
const MAX_AIS: usize = 6;

const MENU: [(&str, &str); 5] = [
    (
        "Start a test",
        "pick questions, pick the AIs, get a report card",
    ),
    ("AI accounts", "connect Anthropic, OpenAI, Google and more"),
    ("Open in your browser", "charts, tasks and comparisons"),
    ("Help", "keys and commands"),
    ("Quit", ""),
];

const HINDI_NAME: &str = "कृत्रिम मूल्यांकन";
const DEMO_GROUP: &str = "Free demo bots: no account needed";
const TOO_CLOSE_WHY: &str = "Too close to call means the gap could be chance. It does not mean they are equal. Ask more questions to be sure.";

#[derive(Clone)]
struct RunnerChoice {
    spec: String,
    label: String,
    note: String,
    group: String,
    selected: bool,
}

#[derive(Clone)]
struct SuiteInfo {
    label: String,
    questions: usize,
    summary: String,
}

enum RunEvent {
    Started(String),
    Result(Box<ResultRecord>),
    Finished(Result<(String, PathBuf, String), String>),
}

#[derive(Default)]
struct Progress {
    run_id: String,
    /// Runner names in the order they were picked.
    order: Vec<String>,
    /// Task ids in suite order: one square each.
    tasks: Vec<String>,
    records: BTreeMap<String, Vec<ResultRecord>>,
    total: usize,
    done: usize,
    cost_usd: f64,
    finished: Option<Result<(String, PathBuf, String), String>>,
}

/// What the start step shows before anything is called.
#[derive(Default)]
struct Confirm {
    ais: Vec<(String, Option<f64>)>,
    problems: Vec<String>,
}

/// The report card once a test ends.
#[derive(Default)]
struct Finished {
    error: Option<String>,
    run_dir: Option<PathBuf>,
    stopped: bool,
    verdicts: Vec<PlainVerdict>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mark {
    Right,
    Wrong,
    Other,
    Pending,
}

/// One AI's line on the running screen and the report card.
struct AiLine {
    label: String,
    marks: Vec<Mark>,
    right: usize,
    done: bool,
    detail: String,
}

/// A row in a picker: a quiet group heading, or something you can choose.
enum Item {
    Heading(String),
    Choice {
        label: String,
        note: String,
        index: usize,
    },
}

pub struct App {
    opts: UiOptions,
    settings: Settings,
    screen: Screen,
    tick: usize,
    menu_index: usize,
    suites: Vec<(String, PathBuf)>,
    suite_info: Vec<SuiteInfo>,
    suite_index: usize,
    suite: Option<Suite>,
    choices: Vec<RunnerChoice>,
    choice_index: usize,
    confirm: Confirm,
    confirm_worst_usd: f64,
    confirm_ok: bool,
    runners: Vec<Runner>,
    channel: Option<Receiver<RunEvent>>,
    progress: Progress,
    finished: Finished,
    key_index: usize,
    key_buffer: String,
    key_touched: bool,
    status: String,
    outcome: Option<Outcome>,
}

impl App {
    pub fn new(opts: UiOptions, settings: Settings) -> App {
        let dir = effective_suites_dir(&settings);
        let mut found: Vec<(String, PathBuf, SuiteInfo)> = discover_suites(&dir)
            .into_iter()
            .map(|path| {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let info = match load_suite(&path) {
                    Ok(suite) => {
                        let (label, _, _, summary, _) = pack_copy(&suite);
                        SuiteInfo {
                            label,
                            questions: suite.tasks.len(),
                            summary,
                        }
                    }
                    Err(_) => SuiteInfo {
                        label: stem.clone(),
                        questions: 0,
                        summary: String::new(),
                    },
                };
                (stem, path, info)
            })
            .collect();
        // The app's recommended first test leads; the rest by name.
        found.sort_by(|a, b| {
            (a.0 != "india-pilot", &a.2.label).cmp(&(b.0 != "india-pilot", &b.2.label))
        });
        let suite_info = found.iter().map(|f| f.2.clone()).collect();
        let suites = found.into_iter().map(|f| (f.0, f.1)).collect();
        App {
            opts,
            settings,
            screen: Screen::Menu,
            tick: 0,
            menu_index: 0,
            suites,
            suite_info,
            suite_index: 0,
            suite: None,
            choices: Vec::new(),
            choice_index: 0,
            confirm: Confirm::default(),
            confirm_worst_usd: 0.0,
            confirm_ok: false,
            runners: Vec::new(),
            channel: None,
            progress: Progress::default(),
            finished: Finished::default(),
            key_index: 0,
            key_buffer: String::new(),
            key_touched: false,
            status: String::new(),
            outcome: None,
        }
    }

    fn go(&mut self, screen: Screen) {
        self.status.clear();
        self.screen = screen;
    }

    fn money(&self, usd: f64) -> String {
        rupees(usd, self.settings.usd_to_inr)
    }

    fn test_label(&self) -> String {
        self.suite
            .as_ref()
            .map(|suite| pack_copy(suite).0)
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- styles

    fn tone(&self, color: Color, fallback: Modifier) -> Style {
        if self.opts.color {
            Style::default().fg(color)
        } else {
            Style::default().add_modifier(fallback)
        }
    }

    fn ink(&self) -> Style {
        self.tone(INK, Modifier::empty())
    }

    fn strong(&self) -> Style {
        self.ink().add_modifier(Modifier::BOLD)
    }

    fn muted(&self) -> Style {
        self.tone(MUTED, Modifier::empty())
    }

    fn dim(&self) -> Style {
        self.tone(FAINT, Modifier::DIM)
    }

    fn rule(&self) -> Style {
        self.tone(LINE, Modifier::DIM)
    }

    fn selected_style(&self) -> Style {
        if self.opts.color {
            Style::default()
                .fg(INK)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        }
    }

    fn selected_note(&self) -> Style {
        if self.opts.color {
            Style::default().fg(MUTED).bg(SURFACE)
        } else {
            self.selected_style()
        }
    }

    /// "Too close to call": a quiet raised chip, like the app's outlined one.
    fn chip_quiet(&self) -> Style {
        if self.opts.color {
            Style::default()
                .fg(INK)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    /// "Clearly different": a filled chip, like the app's filled one.
    fn chip_filled(&self) -> Style {
        if self.opts.color {
            Style::default()
                .fg(PAPER)
                .bg(INK)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }

    fn reveal(&self) -> usize {
        // Welcome animation: spokes appear one by one over ~0.8 s.
        if self.opts.animate {
            (self.tick / 2).min(12)
        } else {
            12
        }
    }

    // ---------------------------------------------------------------- data

    fn build_choices(&mut self) {
        let mut choices = vec![
            RunnerChoice {
                spec: "oracle".into(),
                label: runner_label("oracle"),
                note: "always right · Free".into(),
                group: DEMO_GROUP.into(),
                selected: true,
            },
            RunnerChoice {
                spec: "echo".into(),
                label: runner_label("echo"),
                note: "repeats your question · Free".into(),
                group: DEMO_GROUP.into(),
                selected: true,
            },
        ];
        let tasks = self
            .suite
            .as_ref()
            .map(|suite| suite.tasks.clone())
            .unwrap_or_default();
        if let Ok(prices) = load_prices(Some(&self.settings.prices_path)) {
            let env: BTreeMap<String, String> = std::env::vars().collect();
            let file_entries = keys::read_keys_file(&keys::keys_path());
            let mut models: Vec<(String, String)> = prices
                .prices
                .keys()
                .filter_map(|key| key.split_once('/'))
                .filter(|(provider, model)| !["ollama", "vllm"].contains(provider) && *model != "*")
                .map(|(provider, model)| (provider.to_string(), model.to_string()))
                .collect();
            models.sort_by_key(|(provider, model)| claude_order(provider, model));
            // Newest of each Claude family first, like the app; older ones after.
            let mut newest: BTreeMap<String, String> = BTreeMap::new();
            for (provider, model) in &models {
                if let Some((family, _)) = claude_version(model).filter(|_| provider == "anthropic")
                {
                    newest.entry(family).or_insert_with(|| model.clone());
                }
            }
            let mut latest = Vec::new();
            let mut older = Vec::new();
            for (provider, model) in models {
                let spec = format!("model:{provider}/{model}");
                let Ok(runner) = make_runner(&spec, &prices, &env) else {
                    continue;
                };
                let rows = engine::estimate(&tasks, std::slice::from_ref(&runner));
                let mut note = if rows.iter().any(|row| row.cost_usd.is_none()) {
                    "price unknown".to_string()
                } else {
                    let worst: f64 = rows.iter().filter_map(|row| row.cost_usd).sum();
                    format!("up to {} for this test", self.money(worst))
                };
                let maker = provider_label(&provider);
                if let Some((_, env_name, _)) = keys::PROVIDER_KEYS
                    .iter()
                    .find(|(label, _, _)| label.eq_ignore_ascii_case(&maker))
                {
                    if keys::key_source(env_name, &file_entries).is_none() {
                        note.push_str(&format!(" · needs your {maker} key"));
                    }
                }
                let is_latest = claude_version(&model)
                    .map(|(family, _)| newest.get(&family) == Some(&model))
                    .unwrap_or(true);
                let group = match (provider.as_str(), is_latest) {
                    ("anthropic", true) => "Claude, made by Anthropic".to_string(),
                    ("anthropic", false) => "Older Claude versions".to_string(),
                    _ => format!("{maker} models"),
                };
                let choice = RunnerChoice {
                    spec,
                    label: pretty_model(&model),
                    note,
                    group,
                    selected: false,
                };
                if is_latest {
                    latest.push(choice);
                } else {
                    older.push(choice);
                }
            }
            choices.extend(latest);
            choices.extend(older);
        }
        self.choices = choices;
        self.choice_index = 0;
    }

    fn prepare_confirm(&mut self) {
        self.confirm = Confirm::default();
        self.confirm_ok = false;
        self.confirm_worst_usd = 0.0;
        let Some(suite) = self.suite.clone() else {
            return;
        };
        let prices = match load_prices(Some(&self.settings.prices_path)) {
            Ok(prices) => prices,
            Err(error) => {
                self.confirm.problems.push(error);
                return;
            }
        };
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let mut runners = Vec::new();
        for choice in self.choices.iter().filter(|c| c.selected) {
            match make_runner(&choice.spec, &prices, &env) {
                Ok(runner) => runners.push(runner),
                Err(error) => self.confirm.problems.push(error),
            }
        }
        if !self.confirm.problems.is_empty() {
            return;
        }
        let check = preflight(&suite.tasks, &runners, self.settings.max_usd, 1);
        self.confirm_worst_usd = check.worst_case_usd;
        for runner in &runners {
            let mine: Vec<_> = check
                .estimates
                .iter()
                .filter(|e| e.runner == runner.name)
                .collect();
            let worst = if mine.iter().any(|e| e.cost_usd.is_none()) {
                None
            } else {
                Some(mine.iter().filter_map(|e| e.cost_usd).sum())
            };
            self.confirm.ais.push((runner_label(&runner.name), worst));
        }
        for problem in &check.problems {
            let plain = plain_problem(problem);
            if !self.confirm.problems.contains(&plain) {
                self.confirm.problems.push(plain);
            }
        }
        self.confirm_ok = check.ok();
        self.runners = runners;
    }

    fn start_run(&mut self) {
        let Some(suite) = self.suite.clone() else {
            return;
        };
        let runners = std::mem::take(&mut self.runners);
        let settings = self.settings.clone();
        self.progress = Progress {
            order: runners.iter().map(|r| r.name.clone()).collect(),
            tasks: suite.tasks.iter().map(|t| t.id.clone()).collect(),
            total: suite.tasks.len() * runners.len(),
            ..Progress::default()
        };
        self.finished = Finished::default();
        let (tx, rx) = std::sync::mpsc::channel::<RunEvent>();
        self.channel = Some(rx);
        std::thread::spawn(move || {
            let outcome = Store::open(&settings.db_path).and_then(|store| {
                let tx_result: Sender<RunEvent> = tx.clone();
                let tx_start: Sender<RunEvent> = tx.clone();
                let on_start = move |run_id: &str| {
                    let _ = tx_start.send(RunEvent::Started(run_id.to_string()));
                };
                let on_result = move |record: &ResultRecord| {
                    let _ = tx_result.send(RunEvent::Result(Box::new(record.clone())));
                };
                let callbacks = RunCallbacks {
                    on_start: Some(&on_start),
                    on_attempt: None,
                    on_result: Some(&on_result),
                };
                let summary = run_suite(
                    &suite, &runners, &store, &settings, 7, 1, None, None, &callbacks, None,
                )
                .map_err(|error| error.to_string())?;
                let _ = krutrim_evaluator::outputs::write_outputs(
                    &store,
                    &summary.run_id,
                    &summary.run_dir,
                );
                Ok((summary.run_id, summary.run_dir, summary.status))
            });
            let _ = tx.send(RunEvent::Finished(outcome));
        });
        self.go(Screen::Running);
    }

    fn drain_events(&mut self) {
        let Some(channel) = &self.channel else { return };
        while let Ok(event) = channel.try_recv() {
            match event {
                RunEvent::Started(run_id) => self.progress.run_id = run_id,
                RunEvent::Result(record) => {
                    self.progress.done += 1;
                    self.progress.cost_usd += record.cost_usd.unwrap_or(0.0);
                    self.progress
                        .records
                        .entry(record.runner.clone())
                        .or_default()
                        .push(*record);
                }
                RunEvent::Finished(outcome) => self.progress.finished = Some(outcome),
            }
        }
        if let Some(outcome) = self.progress.finished.take() {
            self.finished = match outcome {
                Ok((run_id, run_dir, status)) => {
                    let verdicts = Store::open(&self.settings.db_path)
                        .and_then(|store| build_report(&store, &run_id))
                        .map(|report| {
                            report
                                .verdicts
                                .iter()
                                .filter(|v| v.metric == "accuracy")
                                .map(plain_verdict)
                                .collect()
                        })
                        .unwrap_or_default();
                    Finished {
                        error: None,
                        run_dir: Some(run_dir),
                        stopped: status != "completed",
                        verdicts,
                    }
                }
                Err(error) => Finished {
                    error: Some(error),
                    ..Finished::default()
                },
            };
            self.go(Screen::Results);
        }
    }

    /// Each AI's squares, score and the same detail line the app shows.
    fn ai_lines(&self) -> Vec<AiLine> {
        let empty = Vec::new();
        self.progress
            .order
            .iter()
            .map(|runner| {
                let records = self.progress.records.get(runner).unwrap_or(&empty);
                let first = |task: &str| {
                    records
                        .iter()
                        .find(|r| r.task_id == task && r.repeat_index == 0)
                };
                let marks: Vec<Mark> = self
                    .progress
                    .tasks
                    .iter()
                    .map(|task| match first(task) {
                        None => Mark::Pending,
                        Some(r) if r.grade_status.as_deref() == Some("graded") => {
                            if r.passed == Some(true) {
                                Mark::Right
                            } else {
                                Mark::Wrong
                            }
                        }
                        Some(_) => Mark::Other,
                    })
                    .collect();
                let answered: Vec<&ResultRecord> =
                    records.iter().filter(|r| r.status == "ok").collect();
                let time: f64 = answered.iter().filter_map(|r| r.latency_s).sum();
                let cost: f64 = records.iter().filter_map(|r| r.cost_usd).sum();
                let errors = records.iter().filter(|r| r.status == "error").count();
                let pending = records
                    .iter()
                    .filter(|r| r.grade_status.as_deref() == Some("pending"))
                    .count();
                let mut detail = vec![
                    if time >= 1.0 {
                        format!(
                            "{} per answer",
                            duration_label(time / answered.len().max(1) as f64)
                        )
                    } else {
                        "Answered instantly".to_string()
                    },
                    if cost > 0.0 {
                        self.money(cost)
                    } else {
                        "Free".to_string()
                    },
                ];
                if errors > 0 {
                    detail.push(format!("{errors} with no answer"));
                }
                if pending > 0 {
                    detail.push(format!("{pending} need a person to mark"));
                }
                AiLine {
                    label: runner_label(runner),
                    right: marks.iter().filter(|m| **m == Mark::Right).count(),
                    done: !marks.contains(&Mark::Pending),
                    marks,
                    detail: detail.join(" · "),
                }
            })
            .collect()
    }
}

/// "claude-sonnet-5-5" -> ("sonnet", (5, 5)).
fn claude_version(model: &str) -> Option<(String, (u32, u32))> {
    let mut parts = model.split('-');
    if parts.next()? != "claude" {
        return None;
    }
    let family = parts.next()?.to_string();
    let major = parts.next()?.parse().ok()?;
    let minor = match parts.next() {
        Some(minor) => minor.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || !family.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some((family, (major, minor)))
}

/// Haiku, Sonnet, Opus, Fable, newest first; other providers after.
fn claude_order(provider: &str, model: &str) -> (u8, u8, std::cmp::Reverse<(u32, u32)>, String) {
    match claude_version(model).filter(|_| provider == "anthropic") {
        Some((family, version)) => {
            let rank = ["haiku", "sonnet", "opus", "fable"]
                .iter()
                .position(|f| *f == family)
                .unwrap_or(4) as u8;
            (0, rank, std::cmp::Reverse(version), model.to_string())
        }
        None => (
            1,
            0,
            std::cmp::Reverse((0, 0)),
            format!("{provider}/{model}"),
        ),
    }
}

fn provider_label(provider: &str) -> String {
    match provider {
        "anthropic" => "Anthropic".into(),
        "openai" => "OpenAI".into(),
        "gemini" => "Gemini".into(),
        "xai" => "xAI".into(),
        "openrouter" => "OpenRouter".into(),
        other => other.to_string(),
    }
}

/// "ANTHROPIC_API_KEY is not set" becomes what to do about it.
fn plain_problem(problem: &str) -> String {
    for (label, env_name, secret) in keys::PROVIDER_KEYS {
        if secret && problem.contains(env_name) {
            return format!("Connect your {label} key first: AI accounts on the start screen.");
        }
    }
    problem.to_string()
}

impl App {
    fn on_key(&mut self, key: KeyEvent) {
        self.on_key_with_start(key, Self::start_run);
    }

    // Keep the launch boundary injectable so confirmation tests cannot call a provider.
    fn on_key_with_start(&mut self, key: KeyEvent, start_run: impl FnOnce(&mut Self)) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.outcome = Some(Outcome::Quit);
            return;
        }
        match self.screen {
            Screen::Menu => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.menu_index = self.menu_index.checked_sub(1).unwrap_or(MENU.len() - 1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.menu_index = (self.menu_index + 1) % MENU.len();
                }
                KeyCode::Enter => match self.menu_index {
                    0 => self.go(Screen::Suites),
                    1 => self.go(Screen::Keys),
                    2 => self.outcome = Some(Outcome::OpenUi),
                    3 => self.go(Screen::Help),
                    _ => self.outcome = Some(Outcome::Quit),
                },
                KeyCode::Char('q') | KeyCode::Esc => self.outcome = Some(Outcome::Quit),
                _ => {}
            },
            Screen::Suites => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.suite_index = self.suite_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.suite_index + 1 < self.suites.len() {
                        self.suite_index += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some((_, path)) = self.suites.get(self.suite_index).cloned() {
                        match load_suite(&path) {
                            Ok(suite) => {
                                self.suite = Some(suite);
                                self.build_choices();
                                self.go(Screen::Runners);
                            }
                            Err(error) => self.status = error,
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Menu),
                _ => {}
            },
            Screen::Runners => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.choice_index = self.choice_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.choice_index + 1 < self.choices.len() {
                        self.choice_index += 1;
                    }
                }
                KeyCode::Char(' ') => {
                    let picked = self.choices.iter().filter(|c| c.selected).count();
                    if let Some(choice) = self.choices.get_mut(self.choice_index) {
                        if choice.selected || picked < MAX_AIS {
                            choice.selected = !choice.selected;
                            self.status.clear();
                        } else {
                            self.status = format!("You can pick up to {MAX_AIS}.");
                        }
                    }
                }
                KeyCode::Enter => {
                    if self.choices.iter().any(|c| c.selected) {
                        self.prepare_confirm();
                        self.go(Screen::Confirm);
                    } else {
                        self.status = "Pick at least one AI.".into();
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Suites),
                _ => {}
            },
            Screen::Confirm => match key.code {
                KeyCode::Char('y') if key.modifiers.is_empty() => {
                    if self.confirm_ok {
                        start_run(self);
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => {
                    self.go(Screen::Runners);
                }
                _ => {}
            },
            Screen::Running => {}
            Screen::Results => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Menu),
                // Run again goes back through the cost check; nothing starts without y.
                KeyCode::Char('r') if self.suite.is_some() && !self.choices.is_empty() => {
                    self.prepare_confirm();
                    self.go(Screen::Confirm);
                }
                KeyCode::Char('n') => self.go(Screen::Suites),
                _ => {}
            },
            Screen::Keys => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.key_index = self.key_index.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.key_index + 1 < keys::PROVIDER_KEYS.len() {
                        self.key_index += 1;
                    }
                }
                KeyCode::Enter => {
                    self.key_buffer.clear();
                    self.key_touched = false;
                    self.go(Screen::KeyEdit);
                }
                KeyCode::Char('d') => {
                    let (label, env_name, _) = keys::PROVIDER_KEYS[self.key_index];
                    match keys::save_key(&keys::keys_path(), env_name, None) {
                        Ok(()) => self.status = format!("{label} removed."),
                        Err(error) => self.status = error,
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.go(Screen::Menu),
                _ => {}
            },
            Screen::KeyEdit => match key.code {
                KeyCode::Enter => {
                    let (label, env_name, secret) = keys::PROVIDER_KEYS[self.key_index];
                    self.screen = Screen::Keys;
                    if self.key_touched && !self.key_buffer.trim().is_empty() {
                        match keys::save_key(
                            &keys::keys_path(),
                            env_name,
                            Some(self.key_buffer.trim()),
                        ) {
                            Ok(()) if secret => self.status = format!("{label} connected."),
                            Ok(()) => self.status = format!("{label} saved."),
                            Err(error) => self.status = error,
                        }
                    }
                    self.key_buffer.clear();
                }
                KeyCode::Backspace => {
                    self.key_buffer.pop();
                    self.key_touched = true;
                }
                KeyCode::Char(character) => {
                    self.key_buffer.push(character);
                    self.key_touched = true;
                }
                KeyCode::Esc => {
                    self.key_buffer.clear();
                    self.go(Screen::Keys);
                }
                _ => {}
            },
            Screen::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.go(Screen::Menu),
                _ => {}
            },
        }
    }

    // ---------------------------------------------------------------- screens

    fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        if self.opts.color {
            // Paint the whole terminal carbon, whatever the terminal theme is.
            frame.render_widget(Block::new().style(Style::default().fg(INK).bg(PAPER)), area);
        }
        match self.screen {
            Screen::Menu => self.render_menu(frame, area),
            Screen::Suites => self.render_suites(frame, area),
            Screen::Runners => self.render_runners(frame, area),
            Screen::Confirm => self.render_confirm(frame, area),
            Screen::Running => self.render_running(frame, area),
            Screen::Results => self.render_results(frame, area),
            Screen::Keys | Screen::KeyEdit => self.render_keys(frame, area),
            Screen::Help => {
                let body = self.chrome(frame, area, None, "esc back");
                let mut lines = vec![
                    Line::from(Span::styled("Help", self.strong())),
                    Line::raw(""),
                ];
                lines.extend(
                    self.opts
                        .help_text
                        .lines()
                        .map(|line| Line::from(Span::styled(line.to_string(), self.ink()))),
                );
                lines.push(Line::raw(""));
                for tip in [
                    "Keys: ↑↓ or j/k move, enter chooses, space picks, esc goes back, q quits.",
                    "NO_COLOR turns colour off; --no-anim turns the animation off.",
                    "Pipes and scripts never see these screens; their output is unchanged.",
                ] {
                    lines.push(Line::from(Span::styled(tip, self.muted())));
                }
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
            }
        }
    }

    /// Every screen shares one frame: the name and its Devanagari at the
    /// top, a rule, the body, a status line, a rule and key hints. With a
    /// step, the three-step tracker sits above the body. Returns the body.
    fn chrome(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        step: Option<usize>,
        hints: &str,
    ) -> Rect {
        let [_, header, top_rule, _, body, status, bottom_rule, keys_row, _] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let inset = |rect: Rect| content(rect);
        let header = inset(header);
        frame.render_widget(
            Paragraph::new(Span::styled("Krutrim Mulyankan", self.strong())),
            header,
        );
        write_whole(frame, header.x + 19, header.y, HINDI_NAME, self.dim());
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("km {}", env!("CARGO_PKG_VERSION")),
                self.dim(),
            ))
            .alignment(Alignment::Right),
            header,
        );
        for row in [top_rule, bottom_rule] {
            let row = inset(row);
            frame.render_widget(
                Paragraph::new(Span::styled("─".repeat(row.width as usize), self.rule())),
                row,
            );
        }
        frame.render_widget(
            Paragraph::new(Span::styled(self.status.clone(), self.ink())),
            inset(status),
        );
        frame.render_widget(Paragraph::new(self.hints(hints)), inset(keys_row));
        let body = inset(body);
        match step {
            Some(step) if body.height > 4 => {
                frame.render_widget(Paragraph::new(self.stepper(step)), body);
                Rect {
                    y: body.y + 2,
                    height: body.height - 2,
                    ..body
                }
            }
            _ => body,
        }
    }

    /// ✓ done, ● here, ○ still to come: Questions, AIs, Start.
    fn stepper(&self, step: usize) -> Line<'static> {
        let mut spans = Vec::new();
        for (index, name) in ["Questions", "AIs", "Start"].iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(" ─── ", self.rule()));
            }
            let (mark, style) = match index.cmp(&step) {
                std::cmp::Ordering::Less => ("✓", self.muted()),
                std::cmp::Ordering::Equal => ("●", self.strong()),
                std::cmp::Ordering::Greater => ("○", self.dim()),
            };
            spans.push(Span::styled(format!("{mark} {name}"), style));
        }
        Line::from(spans)
    }

    /// Key hints, opencode style: the key in ink, what it does in grey.
    fn hints(&self, text: &str) -> Line<'static> {
        let mut spans = Vec::new();
        for (index, hint) in text.split(" · ").enumerate() {
            if index > 0 {
                spans.push(Span::raw("   "));
            }
            let (key, what) = hint.split_once(' ').unwrap_or((hint, ""));
            spans.push(Span::styled(key.to_string(), self.ink()));
            if !what.is_empty() {
                spans.push(Span::styled(format!(" {what}"), self.dim()));
            }
        }
        Line::from(spans)
    }

    /// Lines at the top of a body; returns what is left below them.
    fn render_top(
        &self,
        frame: &mut ratatui::Frame,
        body: Rect,
        lines: Vec<Line<'static>>,
    ) -> Rect {
        let width = body.width.max(1) as usize;
        let height: usize = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(width))
            .sum();
        let height = (height as u16).min(body.height);
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            Rect { height, ..body },
        );
        Rect {
            y: body.y + height,
            height: body.height - height,
            ..body
        }
    }

    /// A picker: quiet headings, quiet rows, and the chosen row on a raised
    /// panel behind an ink bar that sits just left of the text.
    fn render_list(&self, frame: &mut ratatui::Frame, area: Rect, items: &[Item], selected: usize) {
        let area = Rect {
            x: area.x.saturating_sub(2),
            width: area.width + 2,
            ..area
        };
        let label_width = items
            .iter()
            .filter_map(|item| match item {
                Item::Choice { label, .. } => Some(Span::raw(label.as_str()).width()),
                Item::Heading(_) => None,
            })
            .max()
            .unwrap_or(0)
            .min(40);
        let at = items
            .iter()
            .position(|item| matches!(item, Item::Choice { index, .. } if *index == selected))
            .unwrap_or(0);
        let height = area.height as usize;
        let offset = (at + 1).saturating_sub(height);
        let lines: Vec<Line> = items
            .iter()
            .skip(offset)
            .take(height)
            .map(|item| match item {
                Item::Heading(text) => Line::from(vec![
                    Span::raw("  "),
                    Span::styled(text.clone(), self.dim()),
                ]),
                Item::Choice { label, note, index } => {
                    self.row(label, note, label_width, *index == selected, area.width)
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn row(
        &self,
        label: &str,
        note: &str,
        label_width: usize,
        selected: bool,
        width: u16,
    ) -> Line<'static> {
        let label = pad(label, label_width);
        let room = (width as usize).saturating_sub(2 + label_width + 1);
        let note = if note.is_empty() {
            String::new()
        } else {
            fit(&format!("    {note}"), room)
        };
        if !selected {
            return Line::from(vec![
                Span::raw("  "),
                Span::styled(label, self.muted()),
                Span::styled(note, self.dim()),
            ]);
        }
        let mut spans = vec![
            Span::styled("┃ ", self.selected_style()),
            Span::styled(label, self.selected_style()),
            Span::styled(note, self.selected_note()),
        ];
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::styled(
            " ".repeat((width as usize).saturating_sub(used)),
            self.selected_style(),
        ));
        Line::from(spans)
    }

    fn render_menu(&self, frame: &mut ratatui::Frame, area: Rect) {
        let body = self.chrome(frame, area, None, "↑↓ move · enter choose · q quit");
        let mut top: Vec<Line> = Vec::new();
        // The chakra needs eleven rows; shorter terminals start with the words.
        if body.height >= 24 {
            top.extend(
                chakra_lines(self.reveal())
                    .into_iter()
                    .map(|line| Line::from(Span::styled(line, self.ink()))),
            );
            top.push(Line::raw(""));
        }
        top.push(Line::from(Span::styled(
            "Test AI with your own questions.",
            self.strong(),
        )));
        top.push(Line::from(Span::styled(
            "See which answers are right, how long each AI took, and what it cost in rupees.",
            self.muted(),
        )));
        top.push(Line::raw(""));
        let rest = self.render_top(frame, body, top);
        let items: Vec<Item> = MENU
            .iter()
            .enumerate()
            .map(|(index, (label, note))| Item::Choice {
                label: label.to_string(),
                note: note.to_string(),
                index,
            })
            .collect();
        let list = Rect {
            height: rest.height.min(MENU.len() as u16),
            ..rest
        };
        self.render_list(frame, list, &items, self.menu_index);
        if rest.height > MENU.len() as u16 + 1 {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Try it free first: two demo bots need no account and cost nothing.",
                    self.dim(),
                )),
                Rect {
                    y: rest.y + MENU.len() as u16 + 1,
                    height: 1,
                    ..rest
                },
            );
        }
    }

    fn render_suites(&self, frame: &mut ratatui::Frame, area: Rect) {
        let body = self.chrome(frame, area, Some(0), "↑↓ move · enter choose · esc back");
        let mut top = vec![
            Line::from(Span::styled(
                "Which questions should the AIs answer?",
                self.strong(),
            )),
            Line::from(Span::styled(
                "Pick a ready-made test. To write your own questions, use the desktop app.",
                self.muted(),
            )),
            Line::raw(""),
        ];
        if self.suites.is_empty() {
            top.push(Line::from(Span::styled(
                format!(
                    "No ready-made tests found in {}.",
                    effective_suites_dir(&self.settings).display()
                ),
                self.ink(),
            )));
        }
        let rest = self.render_top(frame, body, top);
        let items: Vec<Item> = self
            .suite_info
            .iter()
            .enumerate()
            .map(|(index, info)| Item::Choice {
                label: info.label.clone(),
                note: if info.summary.is_empty() {
                    questions_text(info.questions)
                } else {
                    format!("{} · {}", questions_text(info.questions), info.summary)
                },
                index,
            })
            .collect();
        self.render_list(frame, rest, &items, self.suite_index);
    }

    fn render_runners(&self, frame: &mut ratatui::Frame, area: Rect) {
        let body = self.chrome(
            frame,
            area,
            Some(1),
            "↑↓ move · space pick · enter check and start · esc back",
        );
        let picked = self.choices.iter().filter(|c| c.selected).count();
        let top = vec![
            Line::from(Span::styled(
                "Which AIs should take the test?",
                self.strong(),
            )),
            Line::from(vec![
                Span::styled(
                    format!(
                        "Pick up to {MAX_AIS}. Picking a few lets you compare them side by side. "
                    ),
                    self.muted(),
                ),
                Span::styled(format!("{picked} picked"), self.strong()),
            ]),
            Line::raw(""),
        ];
        let rest = self.render_top(frame, body, top);
        let mut items = Vec::new();
        let mut group = "";
        for (index, choice) in self.choices.iter().enumerate() {
            if choice.group != group {
                if !group.is_empty() {
                    items.push(Item::Heading(String::new()));
                }
                group = &choice.group;
                items.push(Item::Heading(group.to_string()));
            }
            items.push(Item::Choice {
                label: format!(
                    "{} {}",
                    if choice.selected { "■" } else { "□" },
                    choice.label
                ),
                note: choice.note.clone(),
                index,
            });
        }
        self.render_list(frame, rest, &items, self.choice_index);
    }

    fn render_confirm(&self, frame: &mut ratatui::Frame, area: Rect) {
        let paid = self.confirm_worst_usd > 0.0;
        let hints = if !self.confirm_ok {
            "esc back"
        } else if paid {
            "y agree and start · n back"
        } else {
            "y start · n back"
        };
        let body = self.chrome(frame, area, Some(2), hints);
        let mut lines = vec![Line::from(Span::styled("Ready to start", self.strong()))];
        let questions = self.suite.as_ref().map(|s| s.tasks.len()).unwrap_or(0);
        let test = self.test_label();
        match self.confirm.ais.as_slice() {
            [] => {}
            [(only, _)] => lines.push(Line::from(Span::styled(
                format!("{only} will answer the {questions} questions in “{test}”."),
                self.ink(),
            ))),
            many => lines.push(Line::from(Span::styled(
                format!(
                    "{} AIs will each answer the {questions} questions in “{test}”.",
                    many.len()
                ),
                self.ink(),
            ))),
        }
        if !self.confirm.ais.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    "{} answers to check in total.",
                    questions * self.confirm.ais.len()
                ),
                self.muted(),
            )));
            lines.push(Line::raw(""));
            if paid {
                lines.push(Line::from(Span::styled(
                    format!("At most {}", self.money(self.confirm_worst_usd)),
                    self.strong(),
                )));
                lines.push(Line::from(Span::styled(
                    format!(
                        "Usually much less: this assumes every AI writes the longest reply allowed. Rupees use a fixed rate of ₹{} per dollar, not today's bank rate.",
                        self.settings.usd_to_inr
                    ),
                    self.dim(),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Free. No money will be spent.",
                    self.strong(),
                )));
            }
            lines.push(Line::raw(""));
            let width = self
                .confirm
                .ais
                .iter()
                .map(|(label, _)| Span::raw(label.as_str()).width())
                .max()
                .unwrap_or(0);
            for (label, worst) in &self.confirm.ais {
                let cost = match worst {
                    Some(usd) if *usd > 0.0 => format!("up to {}", self.money(*usd)),
                    Some(_) => "Free".to_string(),
                    None => "price unknown".to_string(),
                };
                lines.push(Line::from(vec![
                    Span::styled(pad(label, width + 4), self.muted()),
                    Span::styled(cost, self.muted()),
                ]));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                format!(
                    "Spending limit {}. The test will not start if it could cost more.",
                    self.money(self.settings.max_usd)
                ),
                self.dim(),
            )));
        }
        if !self.confirm.problems.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("Fix this first:", self.strong())));
            for problem in &self.confirm.problems {
                lines.push(Line::from(Span::styled(format!("  {problem}"), self.ink())));
            }
        } else if self.confirm_ok && paid {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                format!(
                    "Press y to agree this test may charge your AI accounts up to {}.",
                    self.money(self.confirm_worst_usd)
                ),
                self.strong(),
            )));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    }

    /// One AI per line: name, a square per question, then score and detail
    /// once it has answered everything.
    fn ai_rows(&self, width: u16) -> Vec<Line<'static>> {
        let lines = self.ai_lines();
        let name_width = lines
            .iter()
            .map(|line| Span::raw(line.label.as_str()).width())
            .max()
            .unwrap_or(0)
            .min(28);
        let count = self.progress.tasks.len();
        // Room for the squares after the name, a score and some detail.
        let room = (width as usize).saturating_sub(name_width + 2 + 8 + 16);
        lines
            .into_iter()
            .map(|line| {
                let mut spans = vec![Span::styled(pad(&line.label, name_width + 2), self.ink())];
                if count * 2 <= room || count <= room {
                    let gap = if count * 2 <= room { " " } else { "" };
                    for mark in &line.marks {
                        let (glyph, style) = match mark {
                            Mark::Right => ("■", self.ink()),
                            Mark::Wrong => ("□", self.dim()),
                            Mark::Other => ("–", self.muted()),
                            Mark::Pending => ("·", self.rule()),
                        };
                        spans.push(Span::styled(format!("{glyph}{gap}"), style));
                    }
                } else {
                    // Too many questions for squares: a bar, like the app.
                    let bar = room.max(10);
                    let filled = line.right * bar / count.max(1);
                    spans.push(Span::styled("━".repeat(filled), self.ink()));
                    spans.push(Span::styled("━".repeat(bar - filled), self.rule()));
                    spans.push(Span::raw(" "));
                }
                if line.done {
                    spans.push(Span::styled(
                        pad(&format!(" {}/{}", line.right, count), 8),
                        self.strong(),
                    ));
                    spans.push(Span::styled(line.detail, self.muted()));
                }
                Line::from(spans)
            })
            .collect()
    }

    fn render_running(&self, frame: &mut ratatui::Frame, area: Rect) {
        let body = self.chrome(frame, area, Some(2), "ctrl+c quit");
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][self.tick % 10];
        let mut sub = format!(
            "{} · {}",
            self.test_label(),
            questions_text(self.progress.tasks.len())
        );
        if self.progress.cost_usd > 0.0 {
            sub.push_str(&format!(" · {} so far", self.money(self.progress.cost_usd)));
        }
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Testing…  ", self.strong()),
                Span::styled(
                    format!("{} of {} answers", self.progress.done, self.progress.total),
                    self.muted(),
                ),
                Span::styled(format!("  {spinner}"), self.dim()),
            ]),
            Line::from(Span::styled(sub, self.muted())),
            Line::raw(""),
        ];
        lines.extend(self.ai_rows(body.width));
        frame.render_widget(Paragraph::new(lines), body);
    }

    fn render_results(&self, frame: &mut ratatui::Frame, area: Rect) {
        let body = self.chrome(frame, area, None, "r run again · n new test · enter done");
        let mut lines = Vec::new();
        if let Some(error) = &self.finished.error {
            lines.push(Line::from(Span::styled("The test stopped", self.strong())));
            lines.push(Line::from(Span::styled(error.clone(), self.ink())));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
            return;
        }
        let spent = if self.progress.cost_usd > 0.0 {
            format!("{} in total", self.money(self.progress.cost_usd))
        } else {
            "Free".to_string()
        };
        lines.push(Line::from(Span::styled(self.test_label(), self.dim())));
        lines.push(Line::from(Span::styled("Report card", self.strong())));
        lines.push(Line::from(Span::styled(
            format!(
                "{} · {} AIs · {spent}",
                questions_text(self.progress.tasks.len()),
                self.progress.order.len()
            ),
            self.muted(),
        )));
        if self.finished.stopped {
            lines.push(Line::from(Span::styled(
                "This test did not finish, so some answers are missing.",
                self.ink(),
            )));
        }
        lines.push(Line::raw(""));
        lines.extend(self.ai_rows(body.width));
        if !self.finished.verdicts.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("Comparing them", self.strong())));
            let mut close = false;
            for verdict in &self.finished.verdicts {
                lines.push(match verdict {
                    PlainVerdict::TooClose { a, b } => {
                        close = true;
                        Line::from(vec![
                            Span::styled(" Too close to call ", self.chip_quiet()),
                            Span::styled(
                                format!("  {} and {}", runner_label(a), runner_label(b)),
                                self.ink(),
                            ),
                        ])
                    }
                    PlainVerdict::Clear { winner, loser } => Line::from(vec![
                        Span::styled(" Clearly different ", self.chip_filled()),
                        Span::styled(
                            format!(
                                "  {} got clearly more right than {} on these questions.",
                                runner_label(winner),
                                runner_label(loser)
                            ),
                            self.ink(),
                        ),
                    ]),
                    PlainVerdict::Same { a, b } => Line::from(Span::styled(
                        format!(
                            "{} and {} got the same questions right.",
                            runner_label(a),
                            runner_label(b)
                        ),
                        self.ink(),
                    )),
                });
            }
            if close {
                lines.push(Line::from(Span::styled(TOO_CLOSE_WHY, self.dim())));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "One small test is a guide, not a ranking.",
            self.dim(),
        )));
        if let Some(dir) = &self.finished.run_dir {
            lines.push(Line::from(Span::styled(
                format!("Report saved in {}", dir.display()),
                self.dim(),
            )));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    }

    fn render_keys(&self, frame: &mut ratatui::Frame, area: Rect) {
        let file_entries = keys::read_keys_file(&keys::keys_path());
        let body = self.chrome(
            frame,
            area,
            None,
            "↑↓ move · enter connect · d remove · esc back",
        );
        let top = vec![
            Line::from(Span::styled("AI accounts", self.strong())),
            Line::from(Span::styled(
                "Keys stay on this computer. They are never shown.",
                self.muted(),
            )),
            Line::raw(""),
        ];
        let rest = self.render_top(frame, body, top);
        let items: Vec<Item> = keys::PROVIDER_KEYS
            .iter()
            .enumerate()
            .map(|(index, (label, env_name, secret))| {
                let source = keys::key_source(env_name, &file_entries);
                let (mark, state) = match (source, secret) {
                    (Some(source), true) => ("■", format!("connected, from the {source}")),
                    (Some(source), false) => ("■", format!("set, from the {source}")),
                    (None, true) => ("□", "not connected".to_string()),
                    (None, false) => ("□", "not set".to_string()),
                };
                Item::Choice {
                    label: format!("{mark} {label}"),
                    note: format!("{state} · {env_name}"),
                    index,
                }
            })
            .collect();
        self.render_list(frame, rest, &items, self.key_index);
        if self.screen == Screen::KeyEdit {
            let (label, env_name, secret) = keys::PROVIDER_KEYS[self.key_index];
            let popup = centered(area, 64, 7);
            let shown = if secret {
                "•".repeat(self.key_buffer.chars().count())
            } else {
                self.key_buffer.clone()
            };
            // Long values scroll: keep the end, where typing happens, in view.
            let room = (popup.width as usize).saturating_sub(8);
            let shown: String = shown
                .chars()
                .skip(shown.chars().count().saturating_sub(room))
                .collect();
            let mut value = vec![
                Span::styled("┃ ", self.ink()),
                Span::styled(format!("{shown}▌"), self.ink()),
            ];
            if shown.is_empty() {
                value.push(Span::styled("paste or type the value", self.dim()));
            }
            let block = if self.opts.color {
                Block::new()
                    .style(Style::default().fg(INK).bg(SURFACE))
                    .padding(Padding::new(2, 2, 1, 1))
            } else {
                Block::bordered().padding(Padding::horizontal(1))
            };
            let title = if secret {
                format!("Connect {label}")
            } else {
                format!("Set {label}")
            };
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(title, self.strong()),
                        Span::styled(format!("  {env_name}"), self.dim()),
                    ]),
                    Line::raw(""),
                    Line::from(value),
                    Line::raw(""),
                    self.hints("enter save · esc cancel"),
                ])
                .block(block),
                popup,
            );
        }
    }
}

/// The text column: four cells in from each side, so the selection bar
/// fits two cells to its left.
fn content(rect: Rect) -> Rect {
    Rect {
        x: rect.x + 4.min(rect.width / 2),
        width: rect.width.saturating_sub(8),
        ..rect
    }
}

/// Cut text to a width, ending with … when anything was dropped.
fn fit(text: &str, width: usize) -> String {
    if Span::raw(text).width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for character in text.chars() {
        if Span::raw(format!("{out}{character}…").as_str()).width() > width {
            break;
        }
        out.push(character);
    }
    format!("{}…", out.trim_end())
}

/// Hand a whole run of text to the terminal as one piece. Devanagari
/// conjuncts are shaped by the terminal, and cell-by-cell drawing leaves
/// gaps between them, so the run sits in one cell and the cells it covers
/// are skipped.
fn write_whole(frame: &mut ratatui::Frame, x: u16, y: u16, text: &str, style: Style) {
    let area = frame.area();
    let width = Span::raw(text).width() as u16;
    if y >= area.bottom() || x + width > area.right() {
        return;
    }
    let buffer = frame.buffer_mut();
    buffer[(x, y)].set_symbol(text).set_style(style);
    for covered in x + 1..x + width {
        buffer[(covered, y)].set_skip(true);
    }
}

fn questions_text(n: usize) -> String {
    if n == 1 {
        "1 question".into()
    } else {
        format!("{n} questions")
    }
}

fn pad(text: &str, width: usize) -> String {
    let used = Span::raw(text).width();
    format!("{text}{}", " ".repeat(width.saturating_sub(used)))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The brand chakra from `ui_assets/brand/km-chakra-wordmark.svg`, sampled
/// onto a braille canvas from the SVG's own geometry (a 128-unit box): the
/// ring (r 48), twelve circuit spokes (straight in from the ring, then a kink
/// toward the core) and the solid rounded-square core. Strokes keep the SVG's
/// 6-unit width. `spokes` controls the reveal animation, 0..=12.
pub fn chakra_lines(spokes: usize) -> Vec<String> {
    const W: usize = 56;
    const H: usize = 44;
    const HALF: f64 = 3.0;
    // Dots per SVG unit: the ring's outer edge just fits the canvas height.
    let scale = (H as f64 / 2.0 - 0.5) / (48.0 + HALF);
    let segment = |px: f64, py: f64, a: (f64, f64), b: (f64, f64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        (px - a.0 - t * dx).hypot(py - a.1 - t * dy)
    };
    let inside = |px: f64, py: f64| {
        let (x, y) = (px - 64.0, py - 64.0);
        if (x.hypot(y) - 48.0).abs() <= HALF {
            return true;
        }
        // Core: the 28-unit square with 3-unit corners.
        let (ex, ey) = ((x.abs() - 11.0).max(0.0), (y.abs() - 11.0).max(0.0));
        if ex.hypot(ey) <= 3.0 {
            return true;
        }
        (0..spokes.min(12)).any(|spoke| {
            // Undo this spoke's rotation, then test the path M64 16 V38 L54 48.
            let (sin, cos) = (-(spoke as f64) * 30.0).to_radians().sin_cos();
            let (qx, qy) = (64.0 + x * cos - y * sin, 64.0 + x * sin + y * cos);
            segment(qx, qy, (64.0, 16.0), (64.0, 38.0)) <= HALF
                || segment(qx, qy, (64.0, 38.0), (54.0, 48.0)) <= HALF
        })
    };
    let dots: Vec<Vec<bool>> = (0..H)
        .map(|y| {
            (0..W)
                .map(|x| {
                    inside(
                        64.0 + (x as f64 + 0.5 - W as f64 / 2.0) / scale,
                        64.0 + (y as f64 + 0.5 - H as f64 / 2.0) / scale,
                    )
                })
                .collect()
        })
        .collect();
    let mut lines = Vec::new();
    for cell_y in 0..H / 4 {
        let mut line = String::new();
        for cell_x in 0..W / 2 {
            let mut bits = 0u32;
            let map = [
                (0, 0, 0),
                (0, 1, 1),
                (0, 2, 2),
                (1, 0, 3),
                (1, 1, 4),
                (1, 2, 5),
                (0, 3, 6),
                (1, 3, 7),
            ];
            for (dx, dy, bit) in map {
                if dots[cell_y * 4 + dy][cell_x * 2 + dx] {
                    bits |= 1 << bit;
                }
            }
            line.push(char::from_u32(0x2800 + bits).unwrap_or(' '));
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

pub fn run(opts: UiOptions, settings: Settings) -> Outcome {
    if enable_raw_mode().is_err() {
        return Outcome::Quit;
    }
    let mut stdout: Stdout = std::io::stdout();
    if execute!(stdout, EnterAlternateScreen).is_err() {
        let _ = disable_raw_mode();
        return Outcome::Quit;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(_) => {
            let _ = disable_raw_mode();
            return Outcome::Quit;
        }
    };
    let mut app = App::new(opts, settings);
    loop {
        app.drain_events();
        if terminal.draw(|frame| app.render(frame)).is_err() {
            break;
        }
        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if app.reveal() < 12 && app.screen == Screen::Menu {
                    app.tick = 24; // any key skips the reveal
                }
                app.on_key(key);
            }
        }
        app.tick += 1;
        if app.outcome.is_some() {
            break;
        }
    }
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    app.outcome.unwrap_or(Outcome::Quit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn test_app() -> App {
        App::new(
            UiOptions {
                animate: false,
                color: false,
                help_text: "Usage: km".into(),
            },
            Settings::from_map(&BTreeMap::new()),
        )
    }

    fn confirmation_app(paid: bool, preflight_ok: bool) -> App {
        let mut app = test_app();
        app.screen = Screen::Confirm;
        app.confirm_ok = preflight_ok;
        app.confirm_worst_usd = if paid { 0.5 } else { 0.0 };
        app
    }

    fn screen_text(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn record(
        runner: &str,
        task: &str,
        status: &str,
        passed: Option<bool>,
        latency: f64,
        cost: f64,
    ) -> ResultRecord {
        ResultRecord {
            run_id: "run".into(),
            task_id: task.into(),
            task_version: 1,
            task_hash: String::new(),
            category: "maths".into(),
            runner: runner.into(),
            runner_kind: "fake".into(),
            seed: 7,
            status: status.into(),
            created_at: String::new(),
            repeat_index: 0,
            output: None,
            error: None,
            latency_s: Some(latency),
            input_tokens: None,
            output_tokens: None,
            cost_usd: Some(cost),
            cost_inr: None,
            steps: None,
            tool_calls: None,
            grader: None,
            grade_status: Some(if status == "ok" { "graded" } else { "error" }.into()),
            passed,
            score: None,
            grade_details: None,
            human_score: None,
            human_note: None,
            artifact_path: None,
        }
    }

    /// Two demo bots and a priced model, three questions, finished.
    fn report_app() -> App {
        let mut app = test_app();
        app.screen = Screen::Results;
        app.progress.order = vec![
            "oracle".into(),
            "echo".into(),
            "model:anthropic/claude-haiku-5-5".into(),
        ];
        app.progress.tasks = vec!["t1".into(), "t2".into(), "t3".into()];
        for task in ["t1", "t2", "t3"] {
            app.progress
                .records
                .entry("oracle".into())
                .or_default()
                .push(record("oracle", task, "ok", Some(true), 0.0, 0.0));
            app.progress
                .records
                .entry("model:anthropic/claude-haiku-5-5".into())
                .or_default()
                .push(record(
                    "model:anthropic/claude-haiku-5-5",
                    task,
                    "ok",
                    Some(task != "t2"),
                    1.3,
                    0.0002,
                ));
        }
        let echo = app.progress.records.entry("echo".into()).or_default();
        echo.push(record("echo", "t1", "ok", Some(false), 0.0, 0.0));
        echo.push(record("echo", "t2", "ok", Some(false), 0.0, 0.0));
        echo.push(record("echo", "t3", "error", None, 0.0, 0.0));
        app.progress.cost_usd = 0.0006;
        app.finished = Finished {
            run_dir: Some(PathBuf::from("runs/run")),
            verdicts: vec![
                PlainVerdict::Clear {
                    winner: "oracle".into(),
                    loser: "echo".into(),
                },
                PlainVerdict::TooClose {
                    a: "oracle".into(),
                    b: "model:anthropic/claude-haiku-5-5".into(),
                },
            ],
            ..Finished::default()
        };
        app
    }

    #[test]
    fn confirmation_requires_explicit_y_for_paid_and_free_runs() {
        for paid in [true, false] {
            for code in [
                KeyCode::Enter,
                KeyCode::Char(' '),
                KeyCode::Char('Y'),
                KeyCode::Tab,
                KeyCode::Backspace,
            ] {
                let mut app = confirmation_app(paid, true);
                for _ in 0..3 {
                    app.on_key_with_start(KeyEvent::from(code), |_| panic!("unexpected launch"));
                }
                assert_eq!(app.screen, Screen::Confirm);
                assert!(app.channel.is_none());
            }
            let mut app = confirmation_app(paid, true);
            let mut launches = 0;
            app.on_key_with_start(KeyEvent::from(KeyCode::Char('y')), |app| {
                launches += 1;
                app.screen = Screen::Running;
            });
            assert_eq!(launches, 1);
            assert_eq!(app.screen, Screen::Running);
            app.on_key_with_start(KeyEvent::from(KeyCode::Char('y')), |_| {
                panic!("duplicate launch")
            });
        }
    }

    #[test]
    fn confirmation_cancel_and_failed_preflight_never_launch() {
        for paid in [true, false] {
            for code in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('q')] {
                let mut app = confirmation_app(paid, true);
                app.on_key_with_start(KeyEvent::from(code), |_| panic!("cancel launched"));
                assert_eq!(app.screen, Screen::Runners);
                app.on_key_with_start(KeyEvent::from(KeyCode::Char('y')), |_| {
                    panic!("cancel retained approval")
                });
            }
            let mut app = confirmation_app(paid, false);
            app.on_key_with_start(KeyEvent::from(KeyCode::Char('y')), |_| {
                panic!("failed preflight launched")
            });
            assert_eq!(app.screen, Screen::Confirm);
            let mut app = confirmation_app(paid, true);
            app.on_key_with_start(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                |_| panic!("quit launched"),
            );
            assert!(matches!(app.outcome, Some(Outcome::Quit)));
        }
    }

    #[test]
    fn reprepare_clears_previous_approval_when_suite_is_unavailable() {
        let mut app = confirmation_app(true, true);
        app.suite = None;
        app.prepare_confirm();
        assert!(!app.confirm_ok);
        app.on_key_with_start(KeyEvent::from(KeyCode::Char('y')), |_| {
            panic!("stale preflight launched")
        });
        assert_eq!(app.screen, Screen::Confirm);
    }

    #[test]
    fn confirmation_ignores_repeat_release_and_modified_y() {
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let mut app = confirmation_app(true, true);
            let key = KeyEvent::new_with_kind(KeyCode::Char('y'), KeyModifiers::NONE, kind);
            app.on_key_with_start(key, |_| panic!("non-press launched"));
            assert_eq!(app.screen, Screen::Confirm);
        }
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
        ] {
            let mut app = confirmation_app(true, true);
            app.on_key_with_start(KeyEvent::new(KeyCode::Char('y'), modifiers), |_| {
                panic!("modified key launched")
            });
            assert_eq!(app.screen, Screen::Confirm);
        }
    }

    #[test]
    fn confirmation_footer_matches_explicit_opt_in() {
        for (paid, ok, hint) in [
            (true, true, "y agree and start"),
            (false, true, "y start"),
            (true, false, "esc back"),
        ] {
            let mut app = confirmation_app(paid, ok);
            app.confirm.ais = vec![("Claude Haiku 5.5".into(), Some(0.5))];
            let text = screen_text(&app, 110, 30);
            assert!(text.contains(hint), "{hint}");
            assert!(!text.contains("enter start") && !text.contains("Enter = run"));
            assert_eq!(text.contains("Press y to agree"), paid && ok);
        }
    }

    #[test]
    fn missing_keys_read_as_what_to_do() {
        assert_eq!(
            plain_problem("model:anthropic/claude-sonnet-5-5: no Anthropic credentials (set ANTHROPIC_API_KEY or run `ant auth login`)"),
            "Connect your Anthropic key first: AI accounts on the start screen."
        );
        assert_eq!(plain_problem("budget cap exceeded"), "budget cap exceeded");
        assert_eq!(
            fit("    6 questions · a long summary", 16),
            "    6 questions…"
        );
    }

    #[test]
    fn hindi_name_is_drawn_whole() {
        let app = test_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let cell = buffer
            .content()
            .iter()
            .find(|c| c.symbol().contains("कृ"))
            .unwrap();
        assert_eq!(cell.symbol(), HINDI_NAME);
    }

    #[test]
    fn chakra_reveal_grows() {
        let empty = chakra_lines(0)
            .join("")
            .chars()
            .filter(|c| *c != '⠀' && *c != ' ')
            .count();
        let full = chakra_lines(12)
            .join("")
            .chars()
            .filter(|c| *c != '⠀' && *c != ' ')
            .count();
        assert!(full > empty, "spokes add dots: {empty} -> {full}");
        assert_eq!(chakra_lines(12).len(), 11);
        // The core is the logo's solid rounded square, not a hollow circle.
        let middle: Vec<char> = chakra_lines(0)[5].chars().collect();
        assert_eq!(&middle[11..17], &['⣿'; 6]);
    }

    #[test]
    fn menu_navigation_and_quit() {
        let mut app = test_app();
        app.on_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.menu_index, 1);
        app.on_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(app.menu_index, 0);
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Keys);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Menu);
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(matches!(app.outcome, Some(Outcome::Quit)));
    }

    #[test]
    fn results_screen_stays_dismissed() {
        let mut app = test_app();
        let (_tx, rx) = std::sync::mpsc::channel::<RunEvent>();
        app.channel = Some(rx);
        app.progress.finished = Some(Err("boom".into()));
        app.drain_events();
        assert_eq!(app.screen, Screen::Results);
        assert!(screen_text(&app, 100, 30).contains("The test stopped"));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Menu);
        app.drain_events();
        assert_eq!(app.screen, Screen::Menu, "finished outcome must fire once");
    }

    #[test]
    fn welcome_frame_uses_the_app_words() {
        let text = screen_text(&test_app(), 90, 30);
        for words in [
            "Krutrim Mulyankan",
            "Test AI with your own questions.",
            "Start a test",
            "AI accounts",
            "Open in your browser",
            "Try it free first",
        ] {
            assert!(text.contains(words), "{words}");
        }
        for old in ["Run a suite", "Manage API keys", "K R U T R I M", "runner"] {
            assert!(!text.contains(old), "{old}");
        }
    }

    #[test]
    fn report_card_matches_the_app() {
        let text = screen_text(&report_app(), 130, 34);
        for words in [
            "Report card",
            "3 questions · 3 AIs · ₹0.05 in total",
            "Answer-key bot",
            "■ ■ ■",
            "3/3",
            "□ □ –",
            "0/3",
            "Claude Haiku 5.5",
            "■ □ ■",
            "2/3",
            "1.3 s per answer · ₹0.05",
            "Answered instantly · Free · 1 with no answer",
            "Comparing them",
            "Clearly different",
            "Answer-key bot got clearly more right than Parrot bot on these questions.",
            "Too close to call",
            "Answer-key bot and Claude Haiku 5.5",
            TOO_CLOSE_WHY,
            "One small test is a guide, not a ranking.",
        ] {
            assert!(text.contains(words), "missing {words:?} in\n{text}");
        }
        assert!(!text.contains('$'));
    }

    #[test]
    fn running_screen_counts_answers_and_leaves_gaps() {
        let mut app = report_app();
        app.screen = Screen::Running;
        app.progress.records.remove("echo");
        app.progress.total = 9;
        app.progress.done = 6;
        let text = screen_text(&app, 110, 30);
        assert!(text.contains("Testing…  6 of 9 answers"));
        assert!(text.contains("● Start"));
        // The parrot has not answered yet: dots, and no score.
        let parrot = text
            .lines()
            .find(|line| line.contains("Parrot bot"))
            .unwrap();
        assert!(parrot.contains("· · ·") && !parrot.contains("/3"));
    }

    #[test]
    fn ai_picker_groups_like_the_app_and_stops_at_six() {
        let mut app = test_app();
        let suites = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../suites/example.yaml");
        app.suite = Some(load_suite(&suites).unwrap());
        app.build_choices();
        let labels: Vec<&str> = app.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(&labels[..2], &["Answer-key bot", "Parrot bot"]);
        let haiku = app
            .choices
            .iter()
            .find(|c| c.label == "Claude Haiku 5.5")
            .unwrap();
        assert_eq!(haiku.group, "Claude, made by Anthropic");
        assert!(haiku.note.starts_with("up to ₹"), "{}", haiku.note);
        let old = app
            .choices
            .iter()
            .find(|c| c.label == "Claude Haiku 4.5")
            .unwrap();
        assert_eq!(old.group, "Older Claude versions");
        assert!(
            labels.iter().position(|l| *l == "Claude Haiku 5.5")
                < labels.iter().position(|l| *l == "Claude Haiku 4.5")
        );
        assert!(!labels.iter().any(|l| l.contains("harness")));

        app.screen = Screen::Runners;
        for index in 2..app.choices.len() {
            app.choice_index = index;
            app.on_key(KeyEvent::from(KeyCode::Char(' ')));
        }
        assert_eq!(app.choices.iter().filter(|c| c.selected).count(), MAX_AIS);
        assert_eq!(app.status, "You can pick up to 6.");
        let text = screen_text(&app, 110, 40);
        assert!(text.contains("6 picked") && text.contains("■ Claude"));
    }

    #[test]
    fn colour_screens_stay_black_grey_and_white() {
        let mut app = report_app();
        app.opts.color = true;
        app.choices = vec![RunnerChoice {
            spec: "oracle".into(),
            label: "Answer-key bot".into(),
            note: "Free".into(),
            group: DEMO_GROUP.into(),
            selected: true,
        }];
        app.confirm.ais = vec![("Answer-key bot".into(), Some(0.0))];
        for screen in [
            Screen::Menu,
            Screen::Suites,
            Screen::Runners,
            Screen::Confirm,
            Screen::Running,
            Screen::Results,
            Screen::Keys,
            Screen::KeyEdit,
            Screen::Help,
        ] {
            app.screen = screen;
            let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            // Painted carbon edge to edge, never the terminal's own theme.
            assert_eq!(buffer[(0, 0)].bg, PAPER, "{screen:?}");
            let mut hidden = 0;
            for cell in buffer.content() {
                // Cells under a wide character are never drawn.
                if hidden > 0 {
                    hidden -= 1;
                    continue;
                }
                hidden = Span::raw(cell.symbol()).width().saturating_sub(1);
                for colour in [cell.fg, cell.bg] {
                    let Color::Rgb(r, g, b) = colour else {
                        panic!("{screen:?} uses {colour:?}");
                    };
                    let spread = r.max(g).max(b) - r.min(g).min(b);
                    assert!(spread <= 4, "{screen:?} uses a hue: {colour:?}");
                }
            }
        }
    }
}
