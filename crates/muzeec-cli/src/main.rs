//! muzeec-cli: command-line interface and TUI launcher.

mod client;

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use serde_json::{Value, json};

use client::DaemonClient;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Parser)]
#[command(
    name = "muzeec-cli",
    version = VERSION,
    about = "Terminal music player: gapless, crossfade, library, MPRIS",
    long_about = None
)]
struct Cli {
    /// Machine-readable JSON output where applicable.
    #[arg(long, global = true)]
    json: bool,
    /// Never auto-spawn the daemon.
    #[arg(long, global = true)]
    no_daemon: bool,
    /// Disable colored output.
    #[arg(long, global = true)]
    no_color: bool,
    /// Path to a config file (defaults to XDG location).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the terminal UI.
    Tui {
        #[arg(long)]
        theme: Option<String>,
    },
    /// Play a query, path, or track id.
    Play {
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
        #[arg(long)]
        next: bool,
        #[arg(long = "now")]
        now: bool,
        #[arg(long)]
        append: bool,
    },
    Pause,
    Resume,
    Toggle,
    Stop,
    Next,
    Prev,
    /// Seek by ±duration, percentage, or absolute position.
    Seek {
        #[arg(allow_hyphen_values = true)]
        target: String,
    },
    /// Set volume (0-100) or adjust with +N / -N.
    Volume {
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    Mute,
    /// Show player status.
    Status {
        #[arg(long)]
        waybar: bool,
        #[arg(long)]
        short: bool,
    },
    /// Queue operations.
    Queue {
        #[command(subcommand)]
        cmd: QueueCmd,
    },
    /// Playlist operations.
    Playlist {
        #[command(subcommand)]
        cmd: PlaylistCmd,
    },
    /// Library operations.
    Library {
        #[command(subcommand)]
        cmd: LibraryCmd,
    },
    /// Full-text search with filters.
    Search {
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Build a similarity mix (auto-DJ).
    Mix {
        #[arg(long)]
        seed: Option<String>,
        #[arg(long)]
        from_now: bool,
        #[arg(long)]
        hours: Option<f64>,
        #[arg(long, default_value_t = 10)]
        count: usize,
        #[arg(long)]
        replace: bool,
    },
    /// Set loop mode.
    Loop {
        mode: Option<String>,
    },
    /// Toggle or set shuffle.
    Shuffle {
        state: Option<String>,
    },
    /// Show album art for the current track.
    Art {
        #[arg(long)]
        path: bool,
        #[arg(long)]
        render: bool,
        #[arg(long)]
        save: Option<PathBuf>,
        #[arg(long, default_value = "40x20")]
        size: String,
    },
    /// Import an M3U playlist into the library.
    Import {
        file: PathBuf,
        #[arg(long)]
        replace: bool,
    },
    /// Export the current queue to an M3U playlist.
    Export {
        file: PathBuf,
    },
    /// Run/start the daemon.
    Daemon {
        #[arg(long)]
        foreground: bool,
    },
    /// Diagnose the environment.
    Doctor,
    /// Generate shell completions.
    Completions {
        shell: Shell,
    },
    /// Configuration helpers.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Generate a man page.
    Man,
}

#[derive(Debug, Subcommand)]
enum QueueCmd {
    List,
    Add {
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
    #[command(name = "play-next")]
    PlayNext {
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
    Remove {
        id: u64,
    },
    Move {
        from: usize,
        to: usize,
    },
    Clear,
    Shuffle,
    Save {
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum PlaylistCmd {
    List,
    Show {
        name: String,
    },
    Create {
        name: String,
    },
    Add {
        name: String,
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
    Remove {
        name: String,
        position: usize,
    },
    Move {
        name: String,
        from: usize,
        to: usize,
    },
    Rename {
        old: String,
        new: String,
    },
    Delete {
        name: String,
    },
    Import {
        file: PathBuf,
    },
    Export {
        name: String,
        file: PathBuf,
    },
    Play {
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum LibraryCmd {
    Scan,
    Rescan,
    Stats,
    Artists,
    Albums,
    Tracks {
        #[arg(long, default_value_t = 1000)]
        limit: usize,
    },
    Genres,
}

#[derive(Debug, Subcommand)]
enum ConfigCmd {
    Path,
    Edit,
    Dump,
}

fn main() {
    color_eyre::install().ok();
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let json_out = cli.json;
    match cli.command {
        None | Some(Command::Tui { .. }) => {
            let theme = match &cli.command {
                Some(Command::Tui { theme }) => theme.clone(),
                _ => None,
            };
            let client = DaemonClient::connect(cli.no_daemon)?;
            let config_path = cli
                .config
                .clone()
                .unwrap_or_else(|| muzeec_cli_core::paths::Paths::resolve().config_file());
            muzeec_cli_tui::run(client.into_inner(), theme, config_path).map_err(|e| e.to_string())
        }
        Some(Command::Completions { shell }) => {
            let mut cmd = Cli::command();
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut cmd, "muzeec-cli", &mut buf);
            let _ = std::io::stdout().write_all(&buf);
            Ok(())
        }
        Some(Command::Man) => {
            let man = clap_mangen::Man::new(Cli::command());
            let mut buf = Vec::new();
            man.render(&mut buf).map_err(|e| e.to_string())?;
            let _ = std::io::stdout().write_all(&buf);
            Ok(())
        }
        Some(Command::Config { cmd }) => run_config(cmd, cli.config),
        Some(Command::Daemon { foreground }) => run_daemon(foreground),
        Some(cmd) => {
            let mut client = DaemonClient::connect(cli.no_daemon)?;
            dispatch(cmd, &mut client, json_out)
        }
    }
}

fn run_config(cmd: ConfigCmd, config_override: Option<PathBuf>) -> Result<(), String> {
    let paths = muzeec_cli_core::paths::Paths::resolve();
    let path = config_override.unwrap_or_else(|| paths.config_file());
    match cmd {
        ConfigCmd::Path => {
            println!("{}", path.display());
            Ok(())
        }
        ConfigCmd::Edit => {
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&path, muzeec_cli_core::config::Config::default_toml())
                    .map_err(|e| e.to_string())?;
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
            let status = std::process::Command::new(editor)
                .arg(&path)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("editor exited with an error".into());
            }
            Ok(())
        }
        ConfigCmd::Dump => {
            let cfg =
                muzeec_cli_core::config::Config::load_from(&path).map_err(|e| e.to_string())?;
            print!(
                "{}",
                toml::to_string_pretty(&cfg).map_err(|e| e.to_string())?
            );
            Ok(())
        }
    }
}

fn daemon_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("muzeec-cli-daemon");
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from("muzeec-cli-daemon")
}

fn run_daemon(foreground: bool) -> Result<(), String> {
    let bin = daemon_binary();
    let mut cmd = std::process::Command::new(&bin);
    if !foreground {
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn daemon: {e}"))?;
    if foreground {
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("daemon exited with an error".into());
        }
    }
    Ok(())
}

fn dispatch(cmd: Command, client: &mut DaemonClient, json_out: bool) -> Result<(), String> {
    match cmd {
        Command::Play {
            query,
            next,
            now,
            append,
        } => {
            if query.is_empty() {
                let v = client.call("player.resume", Value::Null)?;
                return emit(v, json_out, print_status);
            }
            let ids = resolve_args(client, &query)?;
            if ids.is_empty() {
                return Err("no matching tracks".into());
            }
            let mode = if now {
                "now"
            } else if next || append {
                "next"
            } else {
                "replace"
            };
            let v = client.call("player.play", json!({"tracks": ids, "mode": mode}))?;
            emit(v, json_out, print_status)
        }
        Command::Pause => emit(
            client.call("player.pause", Value::Null)?,
            json_out,
            print_status,
        ),
        Command::Resume => emit(
            client.call("player.resume", Value::Null)?,
            json_out,
            print_status,
        ),
        Command::Toggle => emit(
            client.call("player.toggle", Value::Null)?,
            json_out,
            print_status,
        ),
        Command::Stop => emit(
            client.call("player.stop", Value::Null)?,
            json_out,
            print_status,
        ),
        Command::Next => emit(
            client.call("player.next", Value::Null)?,
            json_out,
            print_status,
        ),
        Command::Prev => emit(
            client.call("player.prev", Value::Null)?,
            json_out,
            print_status,
        ),
        Command::Seek { target } => {
            let params = parse_seek(&target, client)?;
            emit(client.call("player.seek", params)?, json_out, print_status)
        }
        Command::Volume { value } => {
            let params = if let Some(rest) = value.strip_prefix('+') {
                json!({"delta": rest.parse::<i64>().map_err(|_| "bad volume delta")?})
            } else if let Some(rest) = value.strip_prefix('-') {
                json!({"delta": -(rest.parse::<i64>().map_err(|_| "bad volume delta")?)})
            } else {
                json!({"value": value.parse::<i64>().map_err(|_| "bad volume value")?})
            };
            emit(client.call("player.volume", params)?, json_out, |v| {
                println!("volume: {}%", v["volume"].as_u64().unwrap_or(0))
            })
        }
        Command::Mute => emit(client.call("player.mute", Value::Null)?, json_out, |v| {
            println!("muted: {}", v["muted"].as_bool().unwrap_or(false))
        }),
        Command::Status { waybar, short } => {
            let v = client.call("status", Value::Null)?;
            if json_out {
                print_json(&v);
            } else if waybar {
                print_waybar(&v);
            } else if short {
                print_short(&v);
            } else {
                print_status(&v);
            }
            Ok(())
        }
        Command::Queue { cmd } => dispatch_queue(cmd, client, json_out),
        Command::Playlist { cmd } => dispatch_playlist(cmd, client, json_out),
        Command::Library { cmd } => dispatch_library(cmd, client, json_out),
        Command::Search { query, limit } => {
            let q = query.join(" ");
            let v = client.call("search", json!({"query": q, "limit": limit}))?;
            if json_out {
                print_json(&v);
            } else {
                print_tracks(&v);
            }
            Ok(())
        }
        Command::Mix {
            seed,
            from_now,
            hours,
            count,
            replace,
        } => {
            let mut params = json!({"count": count});
            if let Some(seed) = seed {
                let ids = resolve_args(client, &[seed])?;
                if let Some(id) = ids.first() {
                    params["seed_id"] = json!(id);
                }
            }
            let _ = from_now;
            if let Some(h) = hours {
                params["count"] = json!(((h * 60.0 / 3.5).round() as usize).max(1));
            }
            params["append"] = json!(!replace);
            let v = client.call("mix", params)?;
            emit(v, json_out, |v| {
                let n = v["tracks"].as_array().map(|a| a.len()).unwrap_or(0);
                println!("queued {n} tracks");
            })
        }
        Command::Loop { mode } => {
            let params = match mode {
                Some(m) => json!({"mode": m}),
                None => Value::Null,
            };
            emit(client.call("player.loop", params)?, json_out, |v| {
                println!("loop: {}", v["loop"].as_str().unwrap_or("?"))
            })
        }
        Command::Shuffle { state } => {
            let params = match state.as_deref() {
                Some("on" | "true" | "1") => json!({"value": true}),
                Some("off" | "false" | "0") => json!({"value": false}),
                _ => Value::Null,
            };
            emit(client.call("player.shuffle", params)?, json_out, |v| {
                println!("shuffle: {}", v["shuffle"].as_bool().unwrap_or(false))
            })
        }
        Command::Art {
            path,
            render,
            save,
            size,
        } => cmd_art(client, path, render, save, &size, json_out),
        Command::Import { file, replace } => cmd_import(client, &file, replace, json_out),
        Command::Export { file } => cmd_export(client, &file),
        Command::Doctor => {
            let v = client.call("doctor", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                println!(
                    "library: {} tracks, {} albums, {} artists",
                    v["library"]["tracks"].as_u64().unwrap_or(0),
                    v["library"]["albums"].as_u64().unwrap_or(0),
                    v["library"]["artists"].as_u64().unwrap_or(0)
                );
                println!("fts5: {}", v["fts5"].as_bool().unwrap_or(false));
                println!("art cache: {}", v["art_cache"].as_bool().unwrap_or(false));
                println!("sample rate: {}", v["sample_rate"].as_u64().unwrap_or(0));
                if let Some(devs) = v["output_devices"].as_array() {
                    println!("output devices:");
                    for d in devs {
                        println!("  - {}", d.as_str().unwrap_or("?"));
                    }
                }
            }
            Ok(())
        }
        _ => Err("unsupported command".into()),
    }
}

fn dispatch_queue(cmd: QueueCmd, client: &mut DaemonClient, json_out: bool) -> Result<(), String> {
    match cmd {
        QueueCmd::List => {
            let v = client.call("queue.list", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                for item in v.as_array().cloned().unwrap_or_default() {
                    let marker = if item["current"].as_bool().unwrap_or(false) {
                        "▶"
                    } else {
                        " "
                    };
                    println!(
                        "{marker} {:>3}  {} — {}",
                        item["index"].as_u64().unwrap_or(0) + 1,
                        item["artist"].as_str().unwrap_or(""),
                        item["title"].as_str().unwrap_or("")
                    );
                }
            }
            Ok(())
        }
        QueueCmd::Add { query } => {
            let ids = resolve_args(client, &query)?;
            emit(
                client.call("queue.add", json!({"tracks": ids}))?,
                json_out,
                |_| println!("added to queue"),
            )
        }
        QueueCmd::PlayNext { query } => {
            let ids = resolve_args(client, &query)?;
            emit(
                client.call("queue.play-next", json!({"tracks": ids}))?,
                json_out,
                |_| println!("queued to play next"),
            )
        }
        QueueCmd::Remove { id } => emit(
            client.call("queue.remove", json!({"id": id}))?,
            json_out,
            |v| println!("removed: {}", v["removed"].as_bool().unwrap_or(false)),
        ),
        QueueCmd::Move { from, to } => emit(
            client.call("queue.move", json!({"from": from, "to": to}))?,
            json_out,
            |_| println!("moved"),
        ),
        QueueCmd::Clear => emit(client.call("queue.clear", Value::Null)?, json_out, |_| {
            println!("queue cleared")
        }),
        QueueCmd::Shuffle => emit(client.call("queue.shuffle", Value::Null)?, json_out, |v| {
            println!("shuffled {} items", v["length"].as_u64().unwrap_or(0))
        }),
        QueueCmd::Save { name } => emit(
            client.call("queue.save", json!({"name": name}))?,
            json_out,
            |v| println!("saved {} tracks", v["tracks"].as_u64().unwrap_or(0)),
        ),
    }
}

fn dispatch_playlist(
    cmd: PlaylistCmd,
    client: &mut DaemonClient,
    json_out: bool,
) -> Result<(), String> {
    match cmd {
        PlaylistCmd::List => {
            let v = client.call("playlist.list", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                for p in v.as_array().cloned().unwrap_or_default() {
                    println!(
                        "{}  ({} tracks)",
                        p["name"].as_str().unwrap_or("?"),
                        p["track_count"].as_u64().unwrap_or(0)
                    );
                }
            }
            Ok(())
        }
        PlaylistCmd::Show { name } => {
            let v = client.call("playlist.show", json!({"name": name}))?;
            if json_out {
                print_json(&v);
            } else {
                print_tracks(&v);
            }
            Ok(())
        }
        PlaylistCmd::Create { name } => emit(
            client.call("playlist.create", json!({"name": name}))?,
            json_out,
            |_| println!("playlist created"),
        ),
        PlaylistCmd::Add { name, query } => {
            let ids = resolve_args(client, &query)?;
            emit(
                client.call("playlist.add", json!({"name": name, "tracks": ids}))?,
                json_out,
                |v| println!("added {} tracks", v["added"].as_u64().unwrap_or(0)),
            )
        }
        PlaylistCmd::Remove { name, position } => emit(
            client.call(
                "playlist.remove",
                json!({"name": name, "position": position}),
            )?,
            json_out,
            |_| println!("removed"),
        ),
        PlaylistCmd::Move { name, from, to } => emit(
            client.call(
                "playlist.move",
                json!({"name": name, "from": from, "to": to}),
            )?,
            json_out,
            |_| println!("moved"),
        ),
        PlaylistCmd::Rename { old, new } => emit(
            client.call("playlist.rename", json!({"old": old, "new": new}))?,
            json_out,
            |_| println!("renamed"),
        ),
        PlaylistCmd::Delete { name } => emit(
            client.call("playlist.delete", json!({"name": name}))?,
            json_out,
            |v| println!("deleted: {}", v["deleted"].as_bool().unwrap_or(false)),
        ),
        PlaylistCmd::Import { file } => cmd_import(client, &file, false, json_out),
        PlaylistCmd::Export { name, file } => {
            let v = client.call("playlist.show", json!({"name": name}))?;
            write_m3u(&file, &v)
        }
        PlaylistCmd::Play { name } => emit(
            client.call("playlist.play", json!({"name": name}))?,
            json_out,
            print_status,
        ),
    }
}

fn dispatch_library(
    cmd: LibraryCmd,
    client: &mut DaemonClient,
    json_out: bool,
) -> Result<(), String> {
    match cmd {
        LibraryCmd::Scan | LibraryCmd::Rescan => {
            let v = client.call("library.scan", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                println!(
                    "scanned {} (added {}, updated {}, skipped {}, removed {}, errors {}) in {} ms",
                    v["scanned"].as_u64().unwrap_or(0),
                    v["added"].as_u64().unwrap_or(0),
                    v["updated"].as_u64().unwrap_or(0),
                    v["skipped"].as_u64().unwrap_or(0),
                    v["removed"].as_u64().unwrap_or(0),
                    v["errors"].as_u64().unwrap_or(0),
                    v["duration_ms"].as_u64().unwrap_or(0),
                );
            }
            Ok(())
        }
        LibraryCmd::Stats => {
            let v = client.call("library.stats", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                println!("tracks: {}", v["tracks"].as_u64().unwrap_or(0));
                println!("albums: {}", v["albums"].as_u64().unwrap_or(0));
                println!("artists: {}", v["artists"].as_u64().unwrap_or(0));
                println!("genres: {}", v["genres"].as_u64().unwrap_or(0));
                println!("playlists: {}", v["playlists"].as_u64().unwrap_or(0));
                println!(
                    "total duration: {}",
                    fmt_duration(v["total_duration_ms"].as_u64().unwrap_or(0))
                );
            }
            Ok(())
        }
        LibraryCmd::Artists => {
            let v = client.call("library.artists", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                for a in v.as_array().cloned().unwrap_or_default() {
                    println!(
                        "{}  ({})",
                        a["name"].as_str().unwrap_or("?"),
                        a["tracks"].as_u64().unwrap_or(0)
                    );
                }
            }
            Ok(())
        }
        LibraryCmd::Albums => {
            let v = client.call("library.albums", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                for a in v.as_array().cloned().unwrap_or_default() {
                    println!(
                        "{} — {} ({})",
                        a["albumartist"].as_str().unwrap_or(""),
                        a["name"].as_str().unwrap_or("?"),
                        a["track_count"].as_u64().unwrap_or(0)
                    );
                }
            }
            Ok(())
        }
        LibraryCmd::Tracks { limit } => {
            let v = client.call("library.tracks", json!({"limit": limit}))?;
            if json_out {
                print_json(&v);
            } else {
                print_tracks(&v);
            }
            Ok(())
        }
        LibraryCmd::Genres => {
            let v = client.call("library.genres", Value::Null)?;
            if json_out {
                print_json(&v);
            } else {
                for g in v.as_array().cloned().unwrap_or_default() {
                    println!(
                        "{}  ({})",
                        g["name"].as_str().unwrap_or("?"),
                        g["tracks"].as_u64().unwrap_or(0)
                    );
                }
            }
            Ok(())
        }
    }
}

fn resolve_args(client: &mut DaemonClient, args: &[String]) -> Result<Vec<i64>, String> {
    if args.is_empty() {
        return Ok(Vec::new());
    }
    let all_paths = args.iter().all(|a| Path::new(a).exists());
    let mut ids = Vec::new();
    if all_paths {
        for arg in args {
            let v = client.call("library.resolve", json!({"query": arg}))?;
            ids.extend(json_ids(&v));
        }
    } else {
        let q = args.join(" ");
        let v = client.call("library.resolve", json!({"query": q}))?;
        ids.extend(json_ids(&v));
    }
    Ok(ids)
}

fn json_ids(v: &Value) -> Vec<i64> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .unwrap_or_default()
}

fn parse_seek(target: &str, client: &mut DaemonClient) -> Result<Value, String> {
    if let Some(pct) = target.strip_suffix('%') {
        let pct: f64 = pct.parse().map_err(|_| "bad percentage")?;
        return Ok(json!({"percent": pct}));
    }
    let (sign, rest) = if let Some(r) = target.strip_prefix('+') {
        (1.0, r)
    } else if let Some(r) = target.strip_prefix('-') {
        (-1.0, r)
    } else {
        (0.0, target)
    };
    let ms = muzeec_cli_core::search::parse_duration_ms(rest)
        .ok_or_else(|| format!("cannot parse duration '{target}'"))?;
    if sign != 0.0 {
        return Ok(json!({"delta_secs": sign * ms as f64 / 1000.0}));
    }
    let status = client.call("status", Value::Null)?;
    let rate = status["sample_rate"].as_u64().unwrap_or(48000);
    let frame = (ms as u64) * rate / 1000;
    Ok(json!({"frame": frame}))
}

fn cmd_art(
    client: &mut DaemonClient,
    want_path: bool,
    render: bool,
    save: Option<PathBuf>,
    size: &str,
    json_out: bool,
) -> Result<(), String> {
    let status = client.call("status", Value::Null)?;
    let Some(track_id) = status["track"]["track_id"].as_i64() else {
        return Err("nothing is playing".into());
    };
    let (w, h) = parse_size(size);
    let v = client.call("art.get", json!({"track_id": track_id, "size": w.max(h)}))?;
    if json_out {
        print_json(&v);
        return Ok(());
    }
    let Some(path) = v["path"].as_str() else {
        return Err("no artwork for the current track".into());
    };
    if want_path {
        println!("{path}");
        return Ok(());
    }
    if let Some(out) = save {
        std::fs::copy(path, &out).map_err(|e| e.to_string())?;
        println!("saved to {}", out.display());
        return Ok(());
    }
    if render {
        render_image(Path::new(path), w, h);
        return Ok(());
    }
    println!("{path}");
    Ok(())
}

fn parse_size(s: &str) -> (u32, u32) {
    let mut parts = s.split(['x', 'X']);
    let w = parts.next().and_then(|p| p.parse().ok()).unwrap_or(40);
    let h = parts.next().and_then(|p| p.parse().ok()).unwrap_or(20);
    (w, h)
}

fn render_image(path: &Path, cols: u32, rows: u32) {
    let Ok(img) = image::open(path) else {
        eprintln!("cannot open image");
        return;
    };
    let img = img.thumbnail(cols * 2, rows * 4);
    let (w, h) = (img.width(), img.height());
    let rgb = img.to_rgb8();
    let mut out = std::io::stdout().lock();
    for y in 0..h / 2 {
        for x in 0..w {
            let top = rgb.get_pixel(x, y * 2);
            let bottom = if y * 2 + 1 < h {
                *rgb.get_pixel(x, y * 2 + 1)
            } else {
                image::Rgb([0, 0, 0])
            };
            let _ = write!(
                out,
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                top[0], top[1], top[2], bottom[0], bottom[1], bottom[2]
            );
        }
        let _ = writeln!(out, "\x1b[0m");
    }
}

fn emit(v: Value, json_out: bool, human: impl Fn(&Value)) -> Result<(), String> {
    if json_out {
        print_json(&v);
    } else {
        human(&v);
    }
    Ok(())
}

fn print_json(v: &Value) {
    let s = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    println!("{s}");
}

fn fmt_duration(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn print_status(v: &Value) {
    let state = v["state"].as_str().unwrap_or("stopped");
    let sym = match state {
        "playing" => "▶",
        "paused" => "⏸",
        _ => "■",
    };
    if let Some(track) = v.get("track").filter(|t| !t.is_null()) {
        println!(
            "{sym} {} — {}  [{} / {}]",
            track["artist"].as_str().unwrap_or(""),
            track["title"].as_str().unwrap_or(""),
            fmt_duration(v["position_ms"].as_u64().unwrap_or(0)),
            fmt_duration(v["duration_ms"].as_u64().unwrap_or(0)),
        );
    } else {
        println!("{sym} stopped");
    }
    println!(
        "volume {}%{}  loop:{}  shuffle:{}  queue:{}",
        v["volume"].as_u64().unwrap_or(0),
        if v["muted"].as_bool().unwrap_or(false) {
            " (muted)"
        } else {
            ""
        },
        v["loop"].as_str().unwrap_or("off"),
        if v["shuffle"].as_bool().unwrap_or(false) {
            "on"
        } else {
            "off"
        },
        v["queue_len"].as_u64().unwrap_or(0),
    );
}

fn print_short(v: &Value) {
    let sym = match v["state"].as_str().unwrap_or("stopped") {
        "playing" => "▶",
        "paused" => "⏸",
        _ => "■",
    };
    if let Some(track) = v.get("track").filter(|t| !t.is_null()) {
        println!(
            "{sym} {} — {}",
            track["artist"].as_str().unwrap_or(""),
            track["title"].as_str().unwrap_or("")
        );
    } else {
        println!("{sym}");
    }
}

fn print_waybar(v: &Value) {
    let state = v["state"].as_str().unwrap_or("stopped");
    let (text, class) = match state {
        "playing" => {
            let t = v.get("track").filter(|t| !t.is_null());
            let text = match t {
                Some(t) => format!(
                    "▶ {} — {}",
                    t["artist"].as_str().unwrap_or(""),
                    t["title"].as_str().unwrap_or("")
                ),
                None => "▶".to_string(),
            };
            (text, "playing")
        }
        "paused" => ("⏸ paused".to_string(), "paused"),
        _ => ("■ stopped".to_string(), "stopped"),
    };
    let tooltip = if let Some(t) = v.get("track").filter(|t| !t.is_null()) {
        format!(
            "{}\n{} — {}\n{} / {}",
            t["title"].as_str().unwrap_or(""),
            t["artist"].as_str().unwrap_or(""),
            t["album"].as_str().unwrap_or(""),
            fmt_duration(v["position_ms"].as_u64().unwrap_or(0)),
            fmt_duration(v["duration_ms"].as_u64().unwrap_or(0)),
        )
    } else {
        "muzeec-cli".to_string()
    };
    println!(
        "{}",
        json!({
            "text": text,
            "tooltip": tooltip,
            "class": class,
            "alt": class,
        })
    );
}

fn print_tracks(v: &Value) {
    for t in v.as_array().cloned().unwrap_or_default() {
        println!(
            "{:>5}  {} — {}  ·  {}  [{}]",
            t["id"].as_i64().unwrap_or(0),
            t["artist"].as_str().unwrap_or(""),
            t["title"].as_str().unwrap_or(""),
            t["album"].as_str().unwrap_or(""),
            fmt_duration(t["duration_ms"].as_u64().unwrap_or(0)),
        );
    }
}

fn cmd_import(
    client: &mut DaemonClient,
    file: &Path,
    replace: bool,
    json_out: bool,
) -> Result<(), String> {
    let report = muzeec_cli_core::playlist::import_file(file)
        .map_err(|e| format!("cannot import {}: {e}", file.display()))?;
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("imported")
        .to_string();
    if replace {
        let _ = client.call("playlist.delete", json!({"name": name}));
    }
    client.call("playlist.create", json!({"name": name}))?;
    let mut ids = Vec::new();
    for entry in &report.entries {
        let v = client.call(
            "library.resolve",
            json!({"query": entry.path.to_string_lossy()}),
        )?;
        ids.extend(json_ids(&v));
    }
    client.call("playlist.add", json!({"name": name, "tracks": ids}))?;
    if json_out {
        print_json(&json!({
            "playlist": name,
            "added": ids.len(),
            "missing": report.missing.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "duplicates": report.duplicates,
        }));
    } else {
        println!(
            "imported {} tracks into '{name}'{}",
            ids.len(),
            if report.missing.is_empty() {
                String::new()
            } else {
                format!(" ({} missing)", report.missing.len())
            }
        );
    }
    Ok(())
}

fn cmd_export(client: &mut DaemonClient, file: &Path) -> Result<(), String> {
    let v = client.call("queue.list", Value::Null)?;
    write_m3u(file, &v)
}

fn write_m3u(file: &Path, tracks: &Value) -> Result<(), String> {
    let export: Vec<muzeec_cli_core::playlist::ExportTrack> = tracks
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| {
            let path = t["path"].as_str()?.to_string();
            let title = t["title"].as_str().unwrap_or("").to_string();
            let duration_ms = t["duration_ms"].as_u64().unwrap_or(0);
            Some(muzeec_cli_core::playlist::ExportTrack {
                path: PathBuf::from(path),
                title,
                duration_secs: (duration_ms / 1000) as i64,
            })
        })
        .collect();
    let base = file.parent();
    let text = muzeec_cli_core::playlist::export_m3u(&export, base);
    std::fs::write(file, text).map_err(|e| e.to_string())?;
    println!("wrote {} tracks to {}", export.len(), file.display());
    Ok(())
}
