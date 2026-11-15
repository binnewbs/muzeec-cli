//! muzeec-cli terminal UI: immediate-mode ratatui app driven through IPC.

use std::io::Write;
use std::path::PathBuf;
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
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{FilterType, Resize, StatefulImage};
use serde_json::{Value, json};

use muzeec_cli_core::config::{ArtMode, Config, ReplayGainMode, VisualizerMode};
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
    albumartist: String,
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

struct ConfigMenu {
    idx: usize,
    editing: Option<String>,
}

/// Config fields shown in the menu: (label, is free-text).
const CONFIG_FIELDS: [(&str, bool); 23] = [
    ("Music dirs", true),
    ("Follow symlinks", false),
    ("Watch library", false),
    ("Scan workers", false),
    ("Output device", true),
    ("Sample rate", false),
    ("Buffer (ms)", false),
    ("Gapless", false),
    ("Crossfade (s)", false),
    ("Crossfade album only", false),
    ("ReplayGain", false),
    ("Volume", false),
    ("Resume on start", false),
    ("Idle stop", false),
    ("Theme", true),
    ("Album art", false),
    ("Visualizer", false),
    ("FPS", false),
    ("MPRIS", false),
    ("Notifications", false),
    ("Notify art", false),
    ("EQ enabled", false),
    ("EQ preamp (dB)", false),
];

const SAMPLE_RATES: [Option<u32>; 5] = [
    None,
    Some(44_100),
    Some(48_000),
    Some(96_000),
    Some(192_000),
];
const ART_MODES: [ArtMode; 6] = [
    ArtMode::Auto,
    ArtMode::Kitty,
    ArtMode::Sixel,
    ArtMode::Iterm2,
    ArtMode::Halfblocks,
    ArtMode::Off,
];
const VISUALIZER_MODES: [VisualizerMode; 3] = [
    VisualizerMode::Bars,
    VisualizerMode::Osc,
    VisualizerMode::Off,
];
const REPLAYGAIN_MODES: [ReplayGainMode; 3] = [
    ReplayGainMode::Off,
    ReplayGainMode::Track,
    ReplayGainMode::Album,
];

fn protocol_for(mode: ArtMode, detected: ProtocolType) -> ProtocolType {
    match mode {
        ArtMode::Auto => detected,
        ArtMode::Kitty => ProtocolType::Kitty,
        ArtMode::Sixel => ProtocolType::Sixel,
        ArtMode::Iterm2 => ProtocolType::Iterm2,
        ArtMode::Halfblocks | ArtMode::Off => ProtocolType::Halfblocks,
    }
}

fn make_picker(mode: ArtMode) -> Picker {
    match mode {
        ArtMode::Halfblocks | ArtMode::Off => Picker::halfblocks(),
        _ => Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks()),
    }
}

fn on_off(value: bool) -> String {
    if value {
        "on".to_string()
    } else {
        "off".to_string()
    }
}

fn field_value(config: &Config, idx: usize) -> String {
    match idx {
        0 => config.library.music_dirs.join(", "),
        1 => on_off(config.library.follow_symlinks),
        2 => on_off(config.library.watch),
        3 => config.library.scan_workers.to_string(),
        4 => config.playback.device.clone(),
        5 => config
            .playback
            .engine_sample_rate
            .map(|rate| rate.to_string())
            .unwrap_or_else(|| "auto".into()),
        6 => config.playback.buffer_ms.to_string(),
        7 => on_off(config.playback.gapless),
        8 => format!("{:.1}", config.playback.crossfade_secs),
        9 => on_off(config.playback.crossfade_on_album_change_only),
        10 => format!("{:?}", config.playback.replaygain).to_lowercase(),
        11 => config.playback.volume.to_string(),
        12 => on_off(config.playback.resume_on_start),
        13 => on_off(config.playback.idle_stop),
        14 => config.ui.theme.clone(),
        15 => format!("{:?}", config.ui.album_art).to_lowercase(),
        16 => format!("{:?}", config.ui.visualizer).to_lowercase(),
        17 => config.ui.fps.to_string(),
        18 => on_off(config.integrations.mpris),
        19 => on_off(config.integrations.notifications),
        20 => on_off(config.integrations.notify_art),
        21 => on_off(config.eq.enabled),
        22 => format!("{:.1}", config.eq.preamp_db),
        _ => String::new(),
    }
}

fn cycle_index(idx: usize, len: usize, dir: i32) -> usize {
    if dir >= 0 { (idx + 1) % len } else { (idx + len - 1) % len }
}

fn cycle<T: PartialEq + Copy>(opts: &[T], current: T, dir: i32) -> T {
    let idx = opts.iter().position(|o| *o == current).unwrap_or(0);
    opts[cycle_index(idx, opts.len(), dir)]
}

fn field_adjust(config: &mut Config, idx: usize, dir: i32) {
    let step = dir.signum() as i64;
    match idx {
        1 => config.library.follow_symlinks = !config.library.follow_symlinks,
        2 => config.library.watch = !config.library.watch,
        3 => {
            config.library.scan_workers =
                (config.library.scan_workers as i64 + step).clamp(0, 64) as usize
        }
        5 => {
            let idx = SAMPLE_RATES
                .iter()
                .position(|r| *r == config.playback.engine_sample_rate)
                .unwrap_or(0);
            config.playback.engine_sample_rate = SAMPLE_RATES[cycle_index(idx, SAMPLE_RATES.len(), dir)];
        }
        6 => {
            config.playback.buffer_ms =
                (config.playback.buffer_ms as i64 + step * 5).clamp(10, 100) as u32
        }
        7 => config.playback.gapless = !config.playback.gapless,
        8 => {
            config.playback.crossfade_secs =
                (config.playback.crossfade_secs + step as f32 * 0.5).clamp(0.0, 12.0)
        }
        9 => {
            config.playback.crossfade_on_album_change_only =
                !config.playback.crossfade_on_album_change_only
        }
        10 => config.playback.replaygain = cycle(&REPLAYGAIN_MODES, config.playback.replaygain, dir),
        11 => config.playback.volume = (config.playback.volume as i64 + step * 5).clamp(0, 100) as u8,
        12 => config.playback.resume_on_start = !config.playback.resume_on_start,
        13 => config.playback.idle_stop = !config.playback.idle_stop,
        15 => config.ui.album_art = cycle(&ART_MODES, config.ui.album_art, dir),
        16 => config.ui.visualizer = cycle(&VISUALIZER_MODES, config.ui.visualizer, dir),
        17 => config.ui.fps = (config.ui.fps as i64 + step * 5).clamp(1, 240) as u32,
        18 => config.integrations.mpris = !config.integrations.mpris,
        19 => config.integrations.notifications = !config.integrations.notifications,
        20 => config.integrations.notify_art = !config.integrations.notify_art,
        21 => config.eq.enabled = !config.eq.enabled,
        22 => config.eq.preamp_db = (config.eq.preamp_db + step as f32 * 0.5).clamp(-20.0, 20.0),
        _ => {}
    }
}

fn field_set_text(config: &mut Config, idx: usize, text: &str) {
    match idx {
        0 => {
            config.library.music_dirs = text
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        }
        4 => config.playback.device = text.trim().to_string(),
        14 => config.ui.theme = text.trim().to_string(),
        _ => {}
    }
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
    config: Config,
    config_path: PathBuf,
    menu: Option<ConfigMenu>,
    art: Option<StatefulProtocol>,
    art_hash: Option<String>,
    picker: Picker,
    detected_protocol: ProtocolType,
}

impl App {
    fn new(client: Client, theme: Option<String>, config_path: PathBuf) -> Self {
        let config = Config::load_from(&config_path).unwrap_or_default();
        let mut picker = make_picker(config.ui.album_art);
        let detected_protocol = picker.protocol_type();
        picker.set_protocol_type(protocol_for(config.ui.album_art, detected_protocol));
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
            theme: theme.unwrap_or_else(|| config.ui.theme.clone()),
            quit: false,
            message: None,
            config,
            config_path,
            menu: None,
            art: None,
            art_hash: None,
            picker,
            detected_protocol,
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
            if self.config.ui.album_art == ArtMode::Off {
                self.art = None;
                self.art_hash = None;
            } else {
                let hash = v["track"]["art_hash"].as_str().map(str::to_string);
                if hash != self.art_hash {
                    self.art_hash = hash;
                    self.art = self.load_art(&v);
                }
            }
            self.status = v;
        }
    }

    fn load_art(&mut self, status: &Value) -> Option<StatefulProtocol> {
        let track_id = status["track"]["track_id"].as_i64()?;
        let v = self.call("art.get", json!({"track_id": track_id, "size": 1024}))?;
        let path = v["path"].as_str()?;
        let img = image::open(path).ok()?;
        Some(self.picker.new_resize_protocol(img))
    }

    fn refresh_rows(&mut self) {
        let prev_idx = self.row_idx;
        self.rows.clear();
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
                            albumartist: t["albumartist"].as_str().unwrap_or("").to_string(),
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
                            albumartist: String::new(),
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
                            albumartist: String::new(),
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
                            albumartist: String::new(),
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
                            albumartist: String::new(),
                            duration_ms: t["duration_ms"].as_u64().unwrap_or(0),
                            playable: true,
                        });
                    }
                }
            }
        }
        self.row_idx = prev_idx.min(self.rows.len().saturating_sub(1));
    }

    fn switch_sidebar(&mut self, index: usize) {
        self.sidebar_idx = index % Sidebar::ALL.len();
        self.sidebar = Sidebar::ALL[self.sidebar_idx];
        self.filter = None;
        self.row_idx = 0;
        self.refresh_rows();
    }

    fn activate(&mut self) {
        let Some(row) = self.rows.get(self.row_idx).cloned() else {
            return;
        };
        match self.sidebar {
            Sidebar::Tracks => {
                let ids: Vec<i64> = if row.secondary.is_empty() {
                    Vec::new()
                } else {
                    self.call(
                        "library.album-tracks",
                        json!({"album": row.secondary, "albumartist": row.albumartist}),
                    )
                    .and_then(|v| {
                        v.as_array()
                            .map(|a| a.iter().filter_map(|t| t["id"].as_i64()).collect())
                    })
                    .unwrap_or_default()
                };
                if ids.len() > 1 {
                    let mut ordered = ids;
                    match ordered.iter().position(|id| *id == row.id) {
                        Some(pos) => ordered.rotate_left(pos),
                        None => ordered.insert(0, row.id),
                    }
                    self.call("player.play", json!({"tracks": ordered, "mode": "replace"}));
                } else {
                    self.call("player.play", json!({"tracks": [row.id], "mode": "now"}));
                }
            }
            Sidebar::Queue => {
                self.call("player.play", json!({"tracks": [row.id], "mode": "now"}));
            }
            Sidebar::Artists => {
                self.filter = Some(format!("artist:{}", quote_value(&row.primary)));
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.row_idx = 0;
                self.refresh_rows();
            }
            Sidebar::Genres => {
                self.filter = Some(format!("genre:{}", quote_value(&row.primary)));
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.row_idx = 0;
                self.refresh_rows();
            }
            Sidebar::Albums => {
                let album = row.primary.split(" — ").nth(1).unwrap_or(&row.primary);
                self.filter = Some(format!("album:{}", quote_value(album)));
                self.sidebar = Sidebar::Tracks;
                self.sidebar_idx = 2;
                self.row_idx = 0;
                self.refresh_rows();
            }
            Sidebar::Playlists => {
                self.call("playlist.play", json!({"name": row.primary}));
            }
        }
        self.refresh_status();
    }

    fn handle_menu_key(&mut self, key: KeyEvent) {
        let Some(idx) = self.menu.as_ref().map(|menu| menu.idx) else {
            return;
        };

        if let Some(mut buf) = self.menu.as_ref().and_then(|menu| menu.editing.clone()) {
            match key.code {
                KeyCode::Esc => self.menu.as_mut().unwrap().editing = None,
                KeyCode::Enter => {
                    self.menu.as_mut().unwrap().editing = None;
                    field_set_text(&mut self.config, idx, &buf);
                    self.sync_theme();
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.menu.as_mut().unwrap().editing = Some(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.menu.as_mut().unwrap().editing = Some(buf);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.menu = None,
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(menu) = &mut self.menu {
                    menu.idx = (menu.idx + 1) % CONFIG_FIELDS.len();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(menu) = &mut self.menu {
                    menu.idx = (menu.idx + CONFIG_FIELDS.len() - 1) % CONFIG_FIELDS.len();
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                field_adjust(&mut self.config, idx, -1);
                self.sync_theme();
            }
            KeyCode::Right | KeyCode::Char('l') => {
                field_adjust(&mut self.config, idx, 1);
                self.sync_theme();
            }
            KeyCode::Enter if CONFIG_FIELDS[idx].1 => {
                let value = field_value(&self.config, idx);
                self.menu.as_mut().unwrap().editing = Some(value);
            }
            KeyCode::Enter => {
                field_adjust(&mut self.config, idx, 1);
                self.sync_theme();
            }
            KeyCode::Char('s') => self.save_config(),
            _ => {}
        }
    }

    fn sync_theme(&mut self) {
        self.theme = self.config.ui.theme.clone();
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.validate() {
            self.message = Some(format!("invalid config: {e}"));
            return;
        }
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let result = toml::to_string_pretty(&self.config)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&self.config_path, text).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.sync_theme();
                self.picker
                    .set_protocol_type(protocol_for(self.config.ui.album_art, self.detected_protocol));
                self.art = None;
                self.art_hash = None;
                self.message = Some("config saved · restart daemon to apply".into());
            }
            Err(e) => self.message = Some(format!("config save failed: {e}")),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.menu.is_some() {
            self.handle_menu_key(key);
            return;
        }
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
                            self.row_idx = 0;
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
            KeyCode::Char('C') => {
                self.menu = Some(ConfigMenu {
                    idx: 0,
                    editing: None,
                })
            }
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
            KeyCode::Char('c') if self.sidebar == Sidebar::Queue => {
                self.call("queue.clear", Value::Null);
                self.refresh_rows();
                self.message = Some("queue cleared".into());
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
                self.row_idx = 0;
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

fn draw(frame: &mut Frame<'_>, app: &mut App) {
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

    let bars = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(24)])
        .split(chunks[2]);
    draw_progress(frame, bars[0], app);
    draw_volume(frame, bars[1], app);
    draw_footer(frame, chunks[3], app);
    draw_config_menu(frame, area, app);
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

fn draw_now_playing(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let block = Block::default()
        .title(" Now Playing ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut body = inner;
    if let Some(art) = &mut app.art {
        let art_height = inner.width.min(inner.height.saturating_sub(6)).min(16);
        if art_height >= 2 {
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(art_height), Constraint::Min(0)])
                .split(inner);
            frame.render_stateful_widget(
                StatefulImage::new().resize(Resize::Fit(Some(FilterType::Lanczos3))),
                parts[0],
                art,
            );
            body = parts[1];
        }
    }

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
        body,
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

fn draw_volume(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let vol = app.status["volume"].as_u64().unwrap_or(0).min(100);
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Volume "))
        .gauge_style(Style::default().fg(Color::Green))
        .ratio(vol as f64 / 100.0)
        .label(format!("{vol}%"));
    frame.render_widget(gauge, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let (prompt, line) = match &app.input {
        InputMode::Search(buf) => ("/ ", buf.clone()),
        InputMode::Command(buf) => (":", buf.clone()),
        InputMode::None => (
            "",
            "C config  space play/pause  n/N next/prev  [/] seek  +/- vol  m mute  s stop  a add  c clear  r shuffle  R loop"
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

fn draw_config_menu(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(menu) = &app.menu else {
        return;
    };
    let width = area.width.saturating_sub(4).min(64);
    let height = (CONFIG_FIELDS.len() as u16 + 4).min(area.height.saturating_sub(2));
    if width == 0 || height == 0 {
        return;
    }
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" Config ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let visible = inner.height.saturating_sub(1) as usize;
    if visible == 0 {
        return;
    }
    let offset = menu.idx.saturating_sub(visible - 1);
    let lines: Vec<Line> = CONFIG_FIELDS
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, (label, _))| {
            let value = match &menu.editing {
                Some(buf) if i == menu.idx => format!("{buf}▏"),
                _ => field_value(&app.config, i),
            };
            let style = if i == menu.idx {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!(" {label:<24} {value}"), style))
        })
        .collect();

    let list_area = Rect::new(inner.x, inner.y, inner.width, visible as u16);
    frame.render_widget(Paragraph::new(lines), list_area);

    let hint = if menu.editing.is_some() {
        "enter apply  esc cancel"
    } else {
        "j/k move  h/l change  enter edit/toggle  s save  esc close"
    };
    let hint_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
        hint_area,
    );
}

/// Run the TUI until the user quits.
pub fn run(client: Client, theme: Option<String>, config_path: PathBuf) -> Result<()> {
    let mut app = App::new(client, theme, config_path);
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
