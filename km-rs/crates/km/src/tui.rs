//! Interactive terminal front-end for bare `km` on a TTY: branded welcome
//! screen, suite/runner pickers, API-key manager, live run progress and a
//! results table. Non-TTY behaviour never reaches this module.

use crate::keys;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use krutrim_evaluator::*;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
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

const MENU: [(&str, &str); 5] = [
    ("Run a suite", "pick a suite and models, then watch it run"),
    (
        "Manage API keys",
        "OpenRouter, OpenAI, Anthropic and friends",
    ),
    ("Open km ui", "the local workbench in your browser"),
    ("Help", "commands and keyboard shortcuts"),
    ("Quit", "see you soon"),
];

#[derive(Clone)]
struct RunnerChoice {
    spec: String,
    label: String,
    note: String,
    selected: bool,
}

enum RunEvent {
    Started(String),
    Attempt(String, String),
    Result(Box<ResultRecord>),
    Finished(Result<(String, PathBuf, String), String>),
}

#[derive(Default)]
struct Progress {
    run_id: String,
    current: String,
    per_runner: BTreeMap<String, (usize, usize)>,
    total: usize,
    done: usize,
    passed: usize,
    input_tokens: i64,
    output_tokens: i64,
    cost_known: bool,
    cost_usd: f64,
    log: Vec<String>,
    finished: Option<Result<(String, PathBuf, String), String>>,
}

pub struct App {
    opts: UiOptions,
    settings: Settings,
    screen: Screen,
    tick: usize,
    menu_index: usize,
    suites: Vec<(String, PathBuf)>,
    suite_index: usize,
    suite: Option<Suite>,
    choices: Vec<RunnerChoice>,
    choice_index: usize,
    confirm_lines: Vec<String>,
    confirm_worst_usd: f64,
    confirm_ok: bool,
    runners: Vec<Runner>,
    channel: Option<Receiver<RunEvent>>,
    progress: Progress,
    results_lines: Vec<String>,
    key_index: usize,
    key_buffer: String,
    key_touched: bool,
    status: String,
    outcome: Option<Outcome>,
}

impl App {
    pub fn new(opts: UiOptions, settings: Settings) -> App {
        let dir = effective_suites_dir(&settings);
        let mut suites: Vec<(String, PathBuf)> = discover_suites(&dir)
            .into_iter()
            .map(|path| {
                (
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path,
                )
            })
            .collect();
        suites.sort();
        App {
            opts,
            settings,
            screen: Screen::Menu,
            tick: 0,
            menu_index: 0,
            suites,
            suite_index: 0,
            suite: None,
            choices: Vec::new(),
            choice_index: 0,
            confirm_lines: Vec::new(),
            confirm_worst_usd: 0.0,
            confirm_ok: false,
            runners: Vec::new(),
            channel: None,
            progress: Progress::default(),
            results_lines: Vec::new(),
            key_index: 0,
            key_buffer: String::new(),
            key_touched: false,
            status: String::new(),
            outcome: None,
        }
    }

    fn accent(&self) -> Style {
        if self.opts.color {
            Style::default().fg(Color::Rgb(255, 153, 51))
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    fn dim(&self) -> Style {
        if self.opts.color {
            Style::default().fg(Color::Rgb(140, 140, 140))
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }

    fn selected_style(&self) -> Style {
        if self.opts.color {
            Style::default()
                .fg(Color::Rgb(20, 20, 20))
                .bg(Color::Rgb(255, 153, 51))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
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

    fn build_choices(&mut self) {
        let mut choices = vec![
            RunnerChoice {
                spec: "oracle".into(),
                label: "oracle".into(),
                note: "free · expected answers; checks graders".into(),
                selected: true,
            },
            RunnerChoice {
                spec: "echo".into(),
                label: "echo".into(),
                note: "free · replies with the prompt; a floor score".into(),
                selected: true,
            },
        ];
        if let Ok(prices) = load_prices(Some(&self.settings.prices_path)) {
            for key in prices.prices.keys() {
                if key.ends_with("/*") {
                    continue;
                }
                choices.push(RunnerChoice {
                    spec: format!("model:{key}"),
                    label: key.clone(),
                    note: "model API · priced".into(),
                    selected: false,
                });
            }
        }
        choices.push(RunnerChoice {
            spec: "harness:claude-draft".into(),
            label: "harness:claude-draft".into(),
            note: "Claude Code draft · needs verified access proof".into(),
            selected: false,
        });
        self.choices = choices;
        self.choice_index = 0;
    }

    fn prepare_confirm(&mut self) {
        self.confirm_lines.clear();
        self.confirm_ok = false;
        let Some(suite) = self.suite.clone() else {
            return;
        };
        let prices = match load_prices(Some(&self.settings.prices_path)) {
            Ok(prices) => prices,
            Err(error) => {
                self.confirm_lines.push(error);
                return;
            }
        };
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let mut runners = Vec::new();
        for choice in self.choices.iter().filter(|c| c.selected) {
            match make_runner(&choice.spec, &prices, &env) {
                Ok(runner) => runners.push(runner),
                Err(error) => self.confirm_lines.push(error),
            }
        }
        if !self.confirm_lines.is_empty() {
            return;
        }
        let tasks = suite.tasks.clone();
        let check = preflight(&tasks, &runners, self.settings.max_usd, 1);
        self.confirm_worst_usd = check.worst_case_usd;
        self.confirm_lines.push(format!(
            "{} · {} task(s) × {} runner(s)",
            suite.name,
            tasks.len(),
            runners.len()
        ));
        for runner in &runners {
            let mine: Vec<_> = check
                .estimates
                .iter()
                .filter(|e| e.runner == runner.name)
                .collect();
            let unknown = mine.iter().any(|e| e.cost_usd.is_none());
            let known: f64 = mine.iter().filter_map(|e| e.cost_usd).sum();
            self.confirm_lines.push(format!(
                "  {} — worst case {}",
                runner.name,
                if unknown {
                    "unknown".into()
                } else {
                    format!("${known:.4}")
                }
            ));
        }
        self.confirm_lines.push(format!(
            "Worst case ${:.4} against a cap of ${:.2}.",
            check.worst_case_usd, self.settings.max_usd
        ));
        for problem in &check.problems {
            self.confirm_lines.push(format!("problem: {problem}"));
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
        let total = suite.tasks.len() * runners.len();
        self.progress = Progress {
            total,
            cost_known: true,
            per_runner: runners
                .iter()
                .map(|r| (r.name.clone(), (0, suite.tasks.len())))
                .collect(),
            ..Progress::default()
        };
        let (tx, rx) = std::sync::mpsc::channel::<RunEvent>();
        self.channel = Some(rx);
        std::thread::spawn(move || {
            let outcome = Store::open(&settings.db_path).and_then(|store| {
                let tx_attempt: Sender<RunEvent> = tx.clone();
                let tx_result: Sender<RunEvent> = tx.clone();
                let tx_start: Sender<RunEvent> = tx.clone();
                let on_start = move |run_id: &str| {
                    let _ = tx_start.send(RunEvent::Started(run_id.to_string()));
                };
                let on_attempt = move |task_id: &str, runner: &str| {
                    let _ =
                        tx_attempt.send(RunEvent::Attempt(task_id.to_string(), runner.to_string()));
                };
                let on_result = move |record: &ResultRecord| {
                    let _ = tx_result.send(RunEvent::Result(Box::new(record.clone())));
                };
                let callbacks = RunCallbacks {
                    on_start: Some(&on_start),
                    on_attempt: Some(&on_attempt),
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
        self.screen = Screen::Running;
    }

    fn drain_events(&mut self) {
        let Some(channel) = &self.channel else { return };
        while let Ok(event) = channel.try_recv() {
            match event {
                RunEvent::Started(run_id) => self.progress.run_id = run_id,
                RunEvent::Attempt(task, runner) => {
                    self.progress.current = format!("{task} @ {runner}");
                }
                RunEvent::Result(record) => {
                    self.progress.done += 1;
                    if let Some(entry) = self.progress.per_runner.get_mut(&record.runner) {
                        entry.0 += 1;
                    }
                    if record.passed == Some(true) {
                        self.progress.passed += 1;
                    }
                    self.progress.input_tokens += record.input_tokens.unwrap_or(0);
                    self.progress.output_tokens += record.output_tokens.unwrap_or(0);
                    match record.cost_usd {
                        Some(cost) => self.progress.cost_usd += cost,
                        None => {
                            if record.status == "ok" {
                                self.progress.cost_known = false;
                            }
                        }
                    }
                    let mark = if record.status != "ok" {
                        "error"
                    } else if record.passed == Some(true) {
                        "✓"
                    } else if record.passed == Some(false) {
                        "✗"
                    } else {
                        "…"
                    };
                    self.progress
                        .log
                        .push(format!("{mark} {} @ {}", record.task_id, record.runner));
                    if self.progress.log.len() > 9 {
                        self.progress.log.remove(0);
                    }
                }
                RunEvent::Finished(outcome) => {
                    self.progress.finished = Some(outcome);
                }
            }
        }
        if let Some(outcome) = self.progress.finished.take() {
            self.results_lines = match &outcome {
                Ok((run_id, run_dir, status)) => {
                    let mut lines = vec![format!("Run {run_id} · {status}")];
                    if let Ok(store) = Store::open(&self.settings.db_path) {
                        if let Ok(report) = build_report(&store, run_id) {
                            lines.push(format!(
                                "Leaderboard — {}",
                                report
                                    .rows
                                    .first()
                                    .map(|_| "best first")
                                    .unwrap_or("no rows")
                            ));
                            for row in &report.rows {
                                lines.push(format!(
                                    "  {}",
                                    krutrim_evaluator::report::leaderboard_cells(row).join("  ")
                                ));
                            }
                        }
                    }
                    lines.push(format!("Artifacts: {}", run_dir.display()));
                    lines
                }
                Err(error) => vec![format!("Run failed: {error}")],
            };
            self.screen = Screen::Results;
        }
    }
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
                    0 => self.screen = Screen::Suites,
                    1 => self.screen = Screen::Keys,
                    2 => self.outcome = Some(Outcome::OpenUi),
                    3 => self.screen = Screen::Help,
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
                                self.screen = Screen::Runners;
                            }
                            Err(error) => self.status = error,
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Menu,
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
                    if let Some(choice) = self.choices.get_mut(self.choice_index) {
                        choice.selected = !choice.selected;
                    }
                }
                KeyCode::Enter => {
                    if self.choices.iter().any(|c| c.selected) {
                        self.prepare_confirm();
                        self.screen = Screen::Confirm;
                    } else {
                        self.status = "Select at least one runner (space).".into();
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Suites,
                _ => {}
            },
            Screen::Confirm => match key.code {
                KeyCode::Char('y') if key.modifiers.is_empty() => {
                    if self.confirm_ok {
                        start_run(self);
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => {
                    self.screen = Screen::Runners;
                }
                _ => {}
            },
            Screen::Running => {}
            Screen::Results => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => {
                    self.screen = Screen::Menu;
                }
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
                    self.screen = Screen::KeyEdit;
                }
                KeyCode::Char('d') => {
                    let (_, env_name, _) = keys::PROVIDER_KEYS[self.key_index];
                    match keys::save_key(&keys::keys_path(), env_name, None) {
                        Ok(()) => self.status = format!("{env_name} removed."),
                        Err(error) => self.status = error,
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Menu,
                _ => {}
            },
            Screen::KeyEdit => match key.code {
                KeyCode::Enter => {
                    let (_, env_name, _) = keys::PROVIDER_KEYS[self.key_index];
                    if self.key_touched && !self.key_buffer.trim().is_empty() {
                        match keys::save_key(
                            &keys::keys_path(),
                            env_name,
                            Some(self.key_buffer.trim()),
                        ) {
                            Ok(()) => self.status = format!("{env_name} saved."),
                            Err(error) => self.status = error,
                        }
                    }
                    self.key_buffer.clear();
                    self.screen = Screen::Keys;
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
                    self.screen = Screen::Keys;
                }
                _ => {}
            },
            Screen::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                    self.screen = Screen::Menu;
                }
                _ => {}
            },
        }
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        match self.screen {
            Screen::Menu => self.render_menu(frame, area),
            Screen::Suites => {
                let items: Vec<String> = self.suites.iter().map(|(name, _)| name.clone()).collect();
                self.render_list(
                    frame,
                    area,
                    "Choose a suite",
                    &items,
                    self.suite_index,
                    "↑↓ move · Enter select · Esc back",
                );
            }
            Screen::Runners => {
                let items: Vec<String> = self
                    .choices
                    .iter()
                    .map(|choice| {
                        format!(
                            "{} {} — {}",
                            if choice.selected { "[x]" } else { "[ ]" },
                            choice.label,
                            choice.note
                        )
                    })
                    .collect();
                self.render_list(
                    frame,
                    area,
                    "Choose models / runners",
                    &items,
                    self.choice_index,
                    "↑↓ move · Space toggle · Enter review cost · Esc back",
                );
            }
            Screen::Confirm => self.render_text(
                frame,
                area,
                "Review before running",
                &self.confirm_lines,
                if !self.confirm_ok {
                    "Fix the problems above · Esc back"
                } else if self.confirm_worst_usd > 0.0 {
                    "y = allow paid calls up to the cap and run · n/Esc back"
                } else {
                    "y = run (free) · n/Esc back"
                },
            ),
            Screen::Running => self.render_running(frame, area),
            Screen::Results => self.render_text(
                frame,
                area,
                "Results",
                &self.results_lines,
                "Enter = back to menu",
            ),
            Screen::Keys | Screen::KeyEdit => self.render_keys(frame, area),
            Screen::Help => {
                let mut lines: Vec<String> =
                    self.opts.help_text.lines().map(str::to_string).collect();
                lines.push(String::new());
                lines.push(
                    "Interactive keys: ↑↓/j/k move, Enter select, Space toggle, Esc back, q quit."
                        .into(),
                );
                lines.push("Tips: NO_COLOR disables colours; --no-anim disables animation.".into());
                lines.push(
                    "Pipes and scripts never see this screen; classic output is unchanged.".into(),
                );
                self.render_text(frame, area, "Help", &lines, "Esc = back");
            }
        }
    }

    fn render_menu(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(15),
                Constraint::Min(8),
                Constraint::Length(2),
            ])
            .split(area);
        let mut header: Vec<Line> = chakra_lines(self.reveal())
            .into_iter()
            .map(|line| Line::from(Span::styled(line, self.accent())))
            .collect();
        header.push(Line::from(Span::styled(
            "K R U T R I M   M U L Y A N K A N",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        header.push(Line::from(Span::styled(
            "कृत्रिम मूल्यांकन · local tests for AI models, harnesses and agents",
            self.dim(),
        )));
        frame.render_widget(
            Paragraph::new(header).alignment(Alignment::Center),
            chunks[0],
        );
        let items: Vec<ListItem> = MENU
            .iter()
            .enumerate()
            .map(|(index, (label, note))| {
                let text = format!("  {label:<20} {note}");
                if index == self.menu_index {
                    ListItem::new(Span::styled(format!("›{text}"), self.selected_style()))
                } else {
                    ListItem::new(Span::raw(format!(" {text}")))
                }
            })
            .collect();
        let list_area = centered(chunks[1], 72, MENU.len() as u16 + 2);
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" What would you like to do? "),
            ),
            list_area,
        );
        let footer = if self.status.is_empty() {
            "↑↓ move · Enter select · q quit".to_string()
        } else {
            self.status.clone()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(footer, self.dim())).alignment(Alignment::Center),
            chunks[2],
        );
    }

    fn render_list(
        &self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        title: &str,
        items: &[String],
        index: usize,
        footer: &str,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(2)])
            .split(area);
        let visible: Vec<ListItem> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if i == index {
                    ListItem::new(Span::styled(format!("› {item}"), self.selected_style()))
                } else {
                    ListItem::new(Span::raw(format!("  {item}")))
                }
            })
            .collect();
        frame.render_widget(
            List::new(visible).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} ")),
            ),
            chunks[0],
        );
        let footer = if self.status.is_empty() {
            footer.to_string()
        } else {
            format!("{} — {}", self.status, footer)
        };
        frame.render_widget(Paragraph::new(Span::styled(footer, self.dim())), chunks[1]);
    }

    fn render_text(
        &self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        title: &str,
        lines: &[String],
        footer: &str,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(2)])
            .split(area);
        frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {title} ")),
                ),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(Span::styled(footer.to_string(), self.dim())),
            chunks[1],
        );
    }

    fn render_running(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][self.tick % 10];
        let mut lines: Vec<Line> = vec![Line::from(vec![
            Span::styled(format!("{spinner} "), self.accent()),
            Span::raw(format!(
                "Running {} · {}/{} done · {} passed",
                self.progress.run_id, self.progress.done, self.progress.total, self.progress.passed
            )),
        ])];
        lines.push(Line::from(Span::styled(
            format!("  now: {}", self.progress.current),
            self.dim(),
        )));
        lines.push(Line::raw(""));
        for (runner, (done, total)) in &self.progress.per_runner {
            let width = 24usize;
            let filled = if *total > 0 { done * width / total } else { 0 };
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
            lines.push(Line::from(vec![
                Span::styled(format!("{bar} "), self.accent()),
                Span::raw(format!("{done}/{total}  {runner}")),
            ]));
        }
        lines.push(Line::raw(""));
        let cost = if self.progress.cost_known {
            format!("${:.4}", self.progress.cost_usd)
        } else {
            "unknown".into()
        };
        lines.push(Line::from(Span::styled(
            format!(
                "tokens: {} in / {} out · cost so far: {cost}",
                self.progress.input_tokens, self.progress.output_tokens
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
        for entry in &self.progress.log {
            let style = if entry.starts_with('✓') && self.opts.color {
                Style::default().fg(Color::Green)
            } else if entry.starts_with('✗') && self.opts.color {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("  {entry}"), style)));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" Running ")),
            area,
        );
    }

    fn render_keys(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let file_entries = keys::read_keys_file(&keys::keys_path());
        let items: Vec<String> = keys::PROVIDER_KEYS
            .iter()
            .map(|(label, env_name, _)| {
                let status = match keys::key_source(env_name, &file_entries) {
                    Some(source) => format!("● set ({source})"),
                    None => "○ not set".to_string(),
                };
                format!("{label:<28} {env_name:<22} {status}")
            })
            .collect();
        self.render_list(
            frame,
            area,
            "API keys",
            &items,
            self.key_index,
            "↑↓ move · Enter set · d delete · Esc back — keys are never shown",
        );
        if self.screen == Screen::KeyEdit {
            let (label, env_name, secret) = keys::PROVIDER_KEYS[self.key_index];
            let shown = if secret {
                "•".repeat(self.key_buffer.chars().count())
            } else {
                self.key_buffer.clone()
            };
            let popup = centered(area, 64, 5);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(format!("{label} ({env_name})\n\n{shown}▌"))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Paste or type the value — Enter saves, Esc cancels "),
                    ),
                popup,
            );
        }
    }
}

fn centered(area: ratatui::layout::Rect, width: u16, height: u16) -> ratatui::layout::Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    ratatui::layout::Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The brand chakra (circle with twelve spokes, see
/// `ui_assets/brand/km-chakra-wordmark.svg`) plotted on a braille canvas.
/// `spokes` controls the reveal animation, 0..=12.
pub fn chakra_lines(spokes: usize) -> Vec<String> {
    const W: usize = 56;
    const H: usize = 44;
    let mut dots = vec![vec![false; W]; H];
    let (cx, cy) = (W as f64 / 2.0, H as f64 / 2.0);
    let plot = |dots: &mut Vec<Vec<bool>>, x: f64, y: f64| {
        let (xi, yi) = (x.round() as i32, y.round() as i32);
        if xi >= 0 && yi >= 0 && (xi as usize) < W && (yi as usize) < H {
            dots[yi as usize][xi as usize] = true;
        }
    };
    for step in 0..360 {
        let angle = (step as f64).to_radians();
        for radius in [20.5, 21.5] {
            plot(
                &mut dots,
                cx + radius * angle.cos(),
                cy + radius * angle.sin(),
            );
        }
        for radius in [4.5, 5.5] {
            plot(
                &mut dots,
                cx + radius * angle.cos(),
                cy + radius * angle.sin(),
            );
        }
    }
    for spoke in 0..spokes.min(12) {
        let angle = (spoke as f64 * 30.0 - 90.0).to_radians();
        let mut radius = 5.5;
        while radius <= 20.5 {
            plot(
                &mut dots,
                cx + radius * angle.cos(),
                cy + radius * angle.sin(),
            );
            radius += 0.5;
        }
    }
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
        for paid in [true, false] {
            let app = confirmation_app(paid, true);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("y ="));
            assert!(!text.contains("Enter = run"));
        }
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
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.screen, Screen::Menu);
        app.drain_events();
        assert_eq!(app.screen, Screen::Menu, "finished outcome must fire once");
    }

    #[test]
    fn welcome_frame_contains_brand_and_menu() {
        let app = test_app();
        let backend = TestBackend::new(90, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_string())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("K R U T R I M"));
        assert!(text.contains("Run a suite"));
        assert!(text.contains("Manage API keys"));
        assert!(text.contains("Open km ui"));
    }
}
