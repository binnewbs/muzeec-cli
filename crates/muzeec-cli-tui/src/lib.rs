//! muzeec-cli terminal UI: immediate-mode ratatui app driven through IPC.

use std::io::Write;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use serde_json::{Value, json};

use muzeec_cli_core::error::Result;
use muzeec_cli_core::ipc::Client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Artists,
    Albums,
    Tracks,
    Genres,
    Playlists,
    Queue,
}

impl Sidebar {
    const ALL: [Sidebar; 6] = [
        Sidebar::Artists,
        Sidebar::Albums,
        Sidebar::Tracks,
        Sidebar::Genres,
        Sidebar::Playlists,
        Sidebar::Queue,
    ];

    fn label(self) -> &'static str {
        match self {
            Sidebar::Artists => "Artists",
            Sidebar::Albums => "Albums",
            Sidebar::Tracks => "Tracks",
            Sidebar::Genres => "Genres",
            Sidebar::Playlists => "Playlists",
            Sidebar::Queue => "Queue",
        }
    }
}

#[derive(Debug, Clone)]
struct Row {
    /// Track id for playable rows, otherwise unused.
    id: i64,
    primary: String,
    secondary: String,
    duration_ms: u64,
    playable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Main,
}

enum InputMode {
    None,
    Search(String),
    Command(String),
}

struct App {
    client: Client,
    sidebar: Sidebar,
    sidebar_idx: usize,
    focus: Focus,
    rows: Vec<Row>,
    row_idx: usize,
    status: Value,
    input: InputMode,
    filter: Option<String>,
    theme: String,
    quit: bool,
    message: Option<String>,
}

impl App {
    fn new(client: Client, theme: Option<String>) -> Self {
        Self {
            client,
            sidebar: Sidebar::Tracks,
            sidebar_idx: 2,
            focus: Focus::Main,
            rows: Vec::new(),
            row_idx: 0,
            status: Value::Null,
            input: InputMode::None,
            filter: None,
            theme: theme.unwrap_or_else(|| "catppuccin-mocha".into()),
            quit: false,
            message: None,
        }
    }

    fn call(&mut self, cmd: &str, params: Value) -> Option<Value> {
        match self.client.call(cmd, params) {
            Ok(v) => Some(v),
            Err(e) => {
                self.message = Some(e.to_string());
                None
            }
        }
    }

    fn refresh_status(&mut self) {
        if let Some(v) = self.call("status", Value::Null) {
            self.status = v;
        }
    }

    fn refresh_rows(&mut self) {
        self.rows.clear();
        self.row_idx = 0;
        match self.sidebar {
            Sidebar::Tracks => {
                let v = if let Some(filter) = &self.filter {
                    self.call("search", json!({"query": filter, "limit": 1000}))
                } else {
                    self.call("library.tracks", json!({"limit": 1000}))
                };
                if let Some(v) = v {
                    for t in v.as_array().cloned().unwrap_or_default() {
                        self.rows.push(Row {
                            id: t["id"].as_i64().unwrap_or(0),
                            primary: format!(
                                "{} — {}",
                                t["artist"].as_str().unwrap_or(""),
                                t["title"].as_str().unwrap_or("")
                            ),
                            secondary: t["album"].as_str().unwrap_or("").to_string(),
                            duration_ms: t["duration_ms"].as_u64().unwrap_or(0),
                            playable: true,
                        });
                    }
                }
            }
            Sidebar::Artists | Sidebar::Genres => {
                let cmd = if self.sidebar == Sidebar::Artists {
                    "library.artists"
                } else {
                    "library.genres"
                };
                if let Some(v) = self.call(cmd, Value::Null) {
                    for a in v.as_array().cloned().unwrap_or_default() {
                        self.rows.push(Row {
                            id: 0,
                            primary: a["name"].as_str().unwrap_or("").to_string(),
                            secondary: format!("{} tracks", a["tracks"].as_u64().unwrap_or(0)),
                            duration_ms: 0,
                            playable: false,
                        });
                    }
                }
            }
            Sidebar::Albums => {
                if let Some(v) = self.call("library.albums", Value::Null) {
                    for a in v.as_array().cloned().unwrap_or_default() {
                        self.rows.push(Row {
                            id: 0,
                            primary: format!(
                                "{} — {}",
                                a["albumartist"].as_str().unwrap_or(""),
                                a["name"].as_str().unwrap_or("")
                            ),
                            secondary: format!(
                                "{} · {} tracks",
                                a["year"].as_u64().unwrap_or(0),
                                a["track_count"].as_u64().unwrap_or(0)
                            ),
                            duration_ms: 0,
                            playable: false,
                        });
                    }
                }
            }
            Sidebar::Playlists => {
                if let Some(v) = self.call("playlist.list", Value::Null) {
                    for p in v.as_array().cloned().unwrap_or_default() {
                        self.rows.push(Row {
                            id: 0,
                            primary: p["name"].as_str().unwrap_or("").to_string(),
                            secondary: format!("{} tracks", p["track_count"].as_u64().unwrap_or(0)),
                            duration_ms: 0,
                            playable: false,
                        });
                    }
                }
            }
            Sidebar::Queue => {
                if let Some(v) = self.call("queue.list", Value::Null) {
                    for (i, t) in v.as_array().cloned().unwrap_or_default().iter().enumerate() {
                        let marker = if t["current"].as_bool().unwrap_or(false) {
                            "▶"
                        } else {
                            " "
                        };
                        self.rows.push(Row {
                            id: t["track_id"].as_i64().unwrap_or(0),
                            primary: format!(
                                "{marker} {} — {}",
                                t["artist"].as_str().unwrap_or(""),
                                t["title"].as_str().unwrap_or("")
                            ),
                            secondary: format!("#{}", i + 1),
                            duration_ms: t["duration_ms"].as_u64().unwrap_or(0),
                            playable: true,
                        });
                    }
                }
            }
        }
    }

    fn switch_sidebar(&mut self, index: usize) {
        self.sidebar_idx = index % Sidebar::ALL.len();
        self.sidebar = Sidebar::ALL[self.sidebar_idx];
        self.filter = None;
        self.refresh_rows();
    }

    fn activate(&mut self) {
        let Some(row) = self.rows.get(self.row_idx).cloned() else {
            return;
        };
        match self.sidebar {
            Sidebar::Tracks | Sidebar::Queue => {
                self.call("player.play", json!({"tracks": [row.id], "mode": "now"}));
            }
            Sidebar::Artists => {
                self.filter = Some(format!("artist:{}", quote_value(&row.primary)));
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.refresh_rows();
            }
            Sidebar::Genres => {
                self.filter = Some(format!("genre:{}", quote_value(&row.primary)));
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.refresh_rows();
            }
            Sidebar::Albums => {
                let album = row.primary.split(" — ").nth(1).unwrap_or(&row.primary);
                self.filter = Some(format!("album:{}", quote_value(album)));
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.refresh_rows();
            }
            Sidebar::Playlists => {
                self.call("playlist.play", json!({"name": row.primary}));
            }
        }
        self.refresh_status();
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Input modes first.
        let is_command = matches!(self.input, InputMode::Command(_));
        match &mut self.input {
            InputMode::Search(buf) | InputMode::Command(buf) => {
                match key.code {
                    KeyCode::Esc => self.input = InputMode::None,
                    KeyCode::Enter => {
                        let text = std::mem::take(buf);
                        self.input = InputMode::None;
                        if is_command {
                            self.run_command(&text);
                        } else {
                            self.filter = if text.trim().is_empty() {
                                None
                            } else {
                                Some(text)
                            };
                            self.sidebar = Sidebar::Tracks;
                            self.sidebar_idx = 2;
                            self.refresh_rows();
                        }
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) => buf.push(c),
                    _ => {}
                }
                return;
            }
            InputMode::None => {}
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('/') => self.input = InputMode::Search(String::new()),
            KeyCode::Char(':') => self.input = InputMode::Command(String::new()),
            KeyCode::Tab | KeyCode::Char('l') => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Main,
                    Focus::Main => Focus::Sidebar,
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Main,
                    Focus::Main => Focus::Sidebar,
                }
            }
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Sidebar => self.switch_sidebar(self.sidebar_idx + 1),
                Focus::Main => {
                    if !self.rows.is_empty() {
                        self.row_idx = (self.row_idx + 1) % self.rows.len();
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Sidebar => {
                    let len = Sidebar::ALL.len();
                    self.switch_sidebar((self.sidebar_idx + len - 1) % len);
                }
                Focus::Main => {
                    if !self.rows.is_empty() {
                        self.row_idx = (self.row_idx + self.rows.len() - 1) % self.rows.len();
                    }
                }
            },
            KeyCode::Enter => self.activate(),
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                self.call("player.toggle", Value::Null);
            }
            KeyCode::Char('s') => {
                self.call("player.stop", Value::Null);
            }
            KeyCode::Char('n') => {
                self.call("player.next", Value::Null);
            }
            KeyCode::Char('N') => {
                self.call("player.prev", Value::Null);
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.call("player.volume", json!({"delta": 5}));
            }
            KeyCode::Char('-') => {
                self.call("player.volume", json!({"delta": -5}));
            }
            KeyCode::Char('m') => {
                self.call("player.mute", Value::Null);
            }
            KeyCode::Char('r') => {
                self.call("player.shuffle", Value::Null);
            }
            KeyCode::Char('R') => {
                self.call("player.loop", Value::Null);
            }
            KeyCode::Char('[') => {
                self.call("player.seek", json!({"delta_secs": -5.0}));
            }
            KeyCode::Char(']') => {
                self.call("player.seek", json!({"delta_secs": 5.0}));
            }
            KeyCode::Char('d') if self.sidebar == Sidebar::Queue => {
                if let Some(row) = self.rows.get(self.row_idx) {
                    let queue_id = queue_id_for(&mut self.client, self.row_idx);
                    let _ = row;
                    if let Some(qid) = queue_id {
                        self.call("queue.remove", json!({"id": qid}));
                        self.refresh_rows();
                    }
                }
            }
            KeyCode::Char('a') => {
                if let Some(row) = self.rows.get(self.row_idx).cloned()
                    && row.playable
                {
                    self.call("queue.add", json!({"tracks": [row.id]}));
                    self.message = Some("added to queue".into());
                }
            }
            KeyCode::Char('A') => {
                if let Some(row) = self.rows.get(self.row_idx).cloned()
                    && row.playable
                {
                    self.call("queue.play-next", json!({"tracks": [row.id]}));
                    self.message = Some("queued to play next".into());
                }
            }
            _ => {}
        }
        self.refresh_status();
    }

    fn run_command(&mut self, text: &str) {
        let mut parts = text.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match cmd {
            "q" | "quit" => self.quit = true,
            "loop" => {
                let params = if arg.is_empty() {
                    Value::Null
                } else {
                    json!({"mode": arg})
                };
                self.call("player.loop", params);
            }
            "shuffle" => {
                let params = if arg.is_empty() {
                    Value::Null
                } else {
                    json!({"value": matches!(arg, "on" | "true" | "1")})
                };
                self.call("player.shuffle", params);
            }
            "save" if !arg.is_empty() => {
                self.call("queue.save", json!({"name": arg}));
                self.message = Some(format!("saved queue as '{arg}'"));
            }
            "mix" => {
                self.call("mix", json!({"count": 10, "append": true}));
                self.message = Some("mixed queue appended".into());
            }
            "scan" => {
                self.call("library.scan", Value::Null);
                self.refresh_rows();
                self.message = Some("library scanned".into());
            }
            "search" if !arg.is_empty() => {
                self.filter = Some(arg.to_string());
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.refresh_rows();
            }
            "" => {}
            other => self.message = Some(format!("unknown command '{other}'")),
        }
    }
}

fn queue_id_for(client: &mut Client, index: usize) -> Option<u64> {
    let v = client.call("queue.list", Value::Null).ok()?;
    v.as_array()?.get(index)?.get("queue_id")?.as_u64()
}

fn quote_value(s: &str) -> String {
    format!("\"{}\"", s.replace('"', ""))
}

fn fmt_duration(ms: u64) -> String {
    let total = ms / 1000;
    format!("{}:{:02}", total / 60, total % 60)
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(18),
            Constraint::Min(30),
            Constraint::Length(32),
        ])
        .split(chunks[1]);

    draw_sidebar(frame, body[0], app);
    draw_main(frame, body[1], app);
    draw_now_playing(frame, body[2], app);
    draw_progress(frame, chunks[2], app);
    draw_footer(frame, chunks[3], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let state = app.status["state"].as_str().unwrap_or("stopped");
    let sym = match state {
        "playing" => "▶",
        "paused" => "⏸",
        _ => "■",
    };
    let title = app
        .status
        .get("track")
        .filter(|t| !t.is_null())
        .map(|t| {
            format!(
                "{} — {}",
                t["artist"].as_str().unwrap_or(""),
                t["title"].as_str().unwrap_or("")
            )
        })
        .unwrap_or_else(|| "nothing playing".into());
    let loop_mode = app.status["loop"].as_str().unwrap_or("off");
    let shuffle = if app.status["shuffle"].as_bool().unwrap_or(false) {
        "on"
    } else {
        "off"
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" muzeec-cli  {sym} "),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(" {title} ")),
        Span::styled(
            format!(
                " vol:{} loop:{loop_mode} shuffle:{shuffle} · {} ",
                app.status["volume"].as_u64().unwrap_or(0),
                app.theme,
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem> = Sidebar::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == app.sidebar_idx {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(format!(" {}", s.label()), style)))
        })
        .collect();
    let border = if app.focus == Focus::Sidebar {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let list = List::new(items).block(
        Block::default()
            .title(" Library ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border)),
    );
    frame.render_widget(list, area);
}

fn draw_main(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|r| {
            let mut line = vec![Span::raw(format!(" {} ", r.primary))];
            if !r.secondary.is_empty() {
                line.push(Span::styled(
                    format!(" · {}", r.secondary),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if r.duration_ms > 0 {
                line.push(Span::styled(
                    format!(" {}", fmt_duration(r.duration_ms)),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(line))
        })
        .collect();
    let title = format!(
        " {} {} ",
        app.sidebar.label(),
        app.filter
            .as_ref()
            .map(|f| format!("/ {f}"))
            .unwrap_or_default()
    );
    let border = if app.focus == Focus::Main {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let mut state = ListState::default();
    if !app.rows.is_empty() {
        state.select(Some(app.row_idx));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_now_playing(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .title(" Now Playing ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    if let Some(track) = app.status.get("track").filter(|t| !t.is_null()) {
        lines.push(Line::from(Span::styled(
            track["title"].as_str().unwrap_or("").to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(track["artist"].as_str().unwrap_or("")));
        lines.push(Line::from(Span::styled(
            track["album"].as_str().unwrap_or("").to_string(),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "{} kbps · {} Hz",
                app.status["sample_rate"].as_u64().unwrap_or(0),
                app.status["channels"].as_u64().unwrap_or(0),
            ),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from("No track playing"));
    }
    if let Some(msg) = &app.message {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn draw_progress(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let pos = app.status["position_ms"].as_u64().unwrap_or(0);
    let dur = app.status["duration_ms"].as_u64().unwrap_or(0).max(1);
    let ratio = (pos as f64 / dur as f64).clamp(0.0, 1.0);
    let label = format!(
        "{} / {}  ({}%)",
        fmt_duration(pos),
        fmt_duration(dur),
        (ratio * 100.0) as u64
    );
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Progress "))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(ratio)
        .label(label);
    frame.render_widget(gauge, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let (prompt, line) = match &app.input {
        InputMode::Search(buf) => ("/ ", buf.clone()),
        InputMode::Command(buf) => (":", buf.clone()),
        InputMode::None => (
            "",
            "j/k move  enter play  space pause  n/N next/prev  / search  : command  ? help  q quit"
                .to_string(),
        ),
    };
    let text = if prompt.is_empty() {
        line
    } else {
        format!("{prompt}{line}▏")
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(Color::DarkGray))),
        area,
    );
}

/// Run the TUI until the user quits.
pub fn run(client: Client, theme: Option<String>) -> Result<()> {
    let mut app = App::new(client, theme);
    app.refresh_status();
    app.refresh_rows();

    enable_raw_mode().map_err(muzeec_cli_core::error::Error::Io)?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(muzeec_cli_core::error::Error::Io)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(muzeec_cli_core::error::Error::Io)?;
    let _ = terminal.clear();

    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    let _ = std::io::stdout().flush();
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut last_status = std::time::Instant::now();
    loop {
        app.refresh_status();
        terminal
            .draw(|f| draw(f, app))
            .map_err(muzeec_cli_core::error::Error::Io)?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(250)).map_err(muzeec_cli_core::error::Error::Io)? {
            if let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = event::read().map_err(muzeec_cli_core::error::Error::Io)?
            {
                if kind == crossterm::event::KeyEventKind::Press {
                    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                        app.quit = true;
                    } else {
                        app.handle_key(KeyEvent {
                            code,
                            modifiers,
                            kind,
                            state: crossterm::event::KeyEventState::NONE,
                        });
                    }
                }
            }
        }
        if last_status.elapsed() > Duration::from_secs(5) {
            app.refresh_rows();
            last_status = std::time::Instant::now();
        }
    }
}
