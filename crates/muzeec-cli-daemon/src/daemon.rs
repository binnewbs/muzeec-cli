//! Daemon state and IPC command handling.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use muzeec_cli_core::config::Config;
use muzeec_cli_core::db::{Database, Track};
use muzeec_cli_core::engine::{
    Engine, EngineCommand, EngineConfig, EngineEvent, EngineHandle, PlaybackState, PlaybackTrack,
};
use muzeec_cli_core::error::{Error, Result};
use muzeec_cli_core::ipc::{Event, Handler};
use muzeec_cli_core::library::{ArtCache, ScanOptions, Scanner};
use muzeec_cli_core::mix::{Candidate, Seed, build_mix};
use muzeec_cli_core::paths::Paths;
use muzeec_cli_core::queue::{LoopMode, Queue, QueueState};
use muzeec_cli_core::replaygain;
use muzeec_cli_core::search::SearchQuery;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Fan-out event bus to IPC subscribers.
#[derive(Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<crossbeam_channel::Sender<Event>>>,
}

impl EventBus {
    pub fn subscribe(&self) -> crossbeam_channel::Receiver<Event> {
        let (tx, rx) = crossbeam_channel::unbounded();
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    pub fn publish(&self, event: Event) {
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|tx| tx.send(event.clone()).is_ok());
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    queue: QueueState,
    volume: u8,
    muted: bool,
    position_ms: u64,
}

/// Shared daemon state.
pub struct Daemon {
    pub paths: Paths,
    pub config: Config,
    pub db: Mutex<Database>,
    pub engine: EngineHandle,
    pub queue: Mutex<Queue>,
    pub bus: EventBus,
    pub art: ArtCache,
    /// Track id we manually triggered, so the resulting `TrackStarted` does not
    /// re-position the queue.
    expecting_start: Mutex<Option<i64>>,
    shutting_down: std::sync::atomic::AtomicBool,
}

impl Daemon {
    pub fn new(
        paths: Paths,
        config: Config,
        db: Database,
        art: ArtCache,
        engine: EngineHandle,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths,
            config,
            db: Mutex::new(db),
            engine,
            queue: Mutex::new(Queue::new()),
            bus: EventBus::default(),
            art,
            expecting_start: Mutex::new(None),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Restore persisted queue/volume state.
    pub fn restore(&self) {
        let path = self.paths.state_file();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let Ok(state) = serde_json::from_str::<PersistedState>(&text) else {
            return;
        };
        {
            let mut q = self.queue.lock().unwrap();
            *q = Queue::from_state(state.queue.clone());
        }
        let _ = self.engine.send(EngineCommand::SetVolume(state.volume));
        let _ = self.engine.send(EngineCommand::SetMuted(state.muted));
        if self.config.playback.resume_on_start
            && let Some(track_id) = state
                .queue
                .current
                .and_then(|i| state.queue.items.get(i).map(|item| item.track_id))
        {
            let frame = state.position_ms * self.engine.sample_rate() as u64 / 1000;
            let _ = self.load_track(track_id, frame, false);
        }
    }

    pub fn save_state(&self) {
        let (queue, position_ms) = {
            let q = self.queue.lock().unwrap();
            (q.to_state(), 0u64)
        };
        let snap = self.engine.snapshot();
        let state = PersistedState {
            queue,
            volume: snap.volume,
            muted: snap.muted,
            position_ms: if snap.state == PlaybackState::Stopped {
                0
            } else {
                snap.position_ms
            },
        };
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(self.paths.state_file(), text);
        }
        let _ = position_ms;
    }

    pub fn shutdown(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.save_state();
        let _ = self.engine.send(EngineCommand::Shutdown);
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Database> {
        self.db.lock().unwrap()
    }

    fn playback_track(&self, track_id: i64) -> Result<PlaybackTrack> {
        let db = self.db();
        let track = db
            .track_by_id(track_id)?
            .ok_or_else(|| Error::NotFound(format!("track {track_id}")))?;
        Ok(self.to_playback(&track))
    }

    fn to_playback(&self, track: &Track) -> PlaybackTrack {
        let gain = replaygain::track_gain(track, self.config.playback.replaygain)
            .map(|info| replaygain::clamped_multiplier(info, -1.0))
            .unwrap_or(1.0);
        PlaybackTrack::from_track(track, gain)
    }

    fn load_track(&self, track_id: i64, start_frame: u64, play: bool) -> Result<()> {
        let track = self.playback_track(track_id)?;
        *self.expecting_start.lock().unwrap() = Some(track_id);
        self.engine.send(EngineCommand::Load {
            track,
            start_frame,
            play,
        })?;
        self.preload_next();
        Ok(())
    }

    fn preload_next(&self) {
        let next_id = {
            let q = self.queue.lock().unwrap();
            q.peek_next()
        };
        let next = next_id.and_then(|id| self.playback_track(id).ok());
        let _ = self.engine.send(EngineCommand::SetNext(next));
    }

    fn status_json(&self) -> Value {
        let snap = self.engine.snapshot();
        let (queue_len, index, loop_mode, shuffle) = {
            let q = self.queue.lock().unwrap();
            (q.len(), q.current_index(), q.loop_mode(), q.shuffle())
        };
        json!({
            "state": snap.state,
            "track": snap.track,
            "position_ms": snap.position_ms,
            "duration_ms": snap.duration_ms,
            "volume": snap.volume,
            "muted": snap.muted,
            "queue_len": queue_len,
            "queue_index": index,
            "loop": loop_mode,
            "shuffle": shuffle,
            "sample_rate": snap.sample_rate,
            "channels": snap.channels,
            "buffer_frames": snap.buffer_frames,
            "device": snap.output_device,
            "gapless": snap.gapless,
            "crossfade_secs": snap.crossfade_secs,
        })
    }

    // ---- Engine event handling ------------------------------------------

    pub fn spawn_event_thread(self: &Arc<Self>) {
        let daemon = self.clone();
        let events = daemon.engine.events().clone();
        std::thread::Builder::new()
            .name("engine-events".into())
            .spawn(move || {
                while let Ok(ev) = events.recv() {
                    daemon.handle_engine_event(ev);
                }
            })
            .ok();
    }

    fn handle_engine_event(&self, ev: EngineEvent) {
        match ev {
            EngineEvent::TrackStarted { track } => {
                let manual = {
                    let mut exp = self.expecting_start.lock().unwrap();
                    if exp.as_ref() == Some(&track.track_id) {
                        *exp = None;
                        true
                    } else {
                        false
                    }
                };
                if !manual {
                    let mut q = self.queue.lock().unwrap();
                    q.select_track_id(track.track_id);
                }
                self.preload_next();
                self.bus.publish(Event::new(
                    "track_changed",
                    json!({
                        "id": track.track_id,
                        "title": track.title,
                        "artist": track.artist,
                        "album": track.album,
                        "duration_ms": track.duration_ms,
                        "art": track.art_hash,
                    }),
                ));
            }
            EngineEvent::TrackEnded { track_id } => {
                self.bus
                    .publish(Event::new("track_ended", json!({"id": track_id})));
                let next = {
                    let mut q = self.queue.lock().unwrap();
                    q.next()
                };
                match next {
                    Some(next_id) => {
                        let _ = self.load_track(next_id, 0, true);
                    }
                    None => {
                        self.bus.publish(Event::new("stopped", json!({})));
                    }
                }
            }
            EngineEvent::Error { message } => {
                tracing::warn!(%message, "engine error");
                self.bus
                    .publish(Event::new("error", json!({"message": message})));
            }
            EngineEvent::Stopped => {
                self.bus.publish(Event::new("stopped", json!({})));
            }
        }
    }

    // ---- Command dispatch ------------------------------------------------

    pub fn dispatch(&self, cmd: &str, params: Value) -> Result<Value> {
        match cmd {
            "status" => Ok(self.status_json()),
            "ping" => Ok(json!({"pong": true})),
            "player.play" => self.cmd_play(&params),
            "player.pause" => {
                self.engine.send(EngineCommand::Pause)?;
                self.wait_for_state(PlaybackState::Paused);
                Ok(self.status_json())
            }
            "player.resume" => {
                self.engine.send(EngineCommand::Play)?;
                self.wait_for_state(PlaybackState::Playing);
                Ok(self.status_json())
            }
            "player.toggle" => {
                let snap = self.engine.snapshot();
                match snap.state {
                    PlaybackState::Playing => {
                        self.engine.send(EngineCommand::Pause)?;
                        self.wait_for_state(PlaybackState::Paused);
                    }
                    PlaybackState::Paused => {
                        self.engine.send(EngineCommand::Play)?;
                        self.wait_for_state(PlaybackState::Playing);
                    }
                    PlaybackState::Stopped => {}
                }
                Ok(self.status_json())
            }
            "player.stop" => {
                self.engine.send(EngineCommand::Stop)?;
                self.wait_for_state(PlaybackState::Stopped);
                Ok(self.status_json())
            }
            "player.next" => self.cmd_next(true),
            "player.prev" => self.cmd_prev(),
            "player.seek" => self.cmd_seek(&params),
            "player.volume" => self.cmd_volume(&params),
            "player.mute" => self.cmd_mute(&params),
            "player.loop" => self.cmd_loop(&params),
            "player.shuffle" => self.cmd_shuffle(&params),
            "queue.list" => self.cmd_queue_list(),
            "queue.add" => self.cmd_queue_add(&params),
            "queue.play-next" => self.cmd_queue_play_next(&params),
            "queue.remove" => self.cmd_queue_remove(&params),
            "queue.move" => self.cmd_queue_move(&params),
            "queue.clear" => {
                self.queue.lock().unwrap().clear();
                self.engine.send(EngineCommand::Stop)?;
                self.save_state();
                Ok(json!({"ok": true}))
            }
            "queue.shuffle" => {
                let seed = now_seed();
                let mut q = self.queue.lock().unwrap();
                q.shuffle_with_seed(seed);
                Ok(json!({"ok": true, "length": q.len()}))
            }
            "queue.save" => self.cmd_queue_save(&params),
            "playlist.list" => {
                let db = self.db();
                Ok(json!(db.playlists()?))
            }
            "playlist.create" => {
                let name = req_str(&params, "name")?;
                let db = self.db();
                db.playlist_create(&name)?;
                Ok(json!({"ok": true}))
            }
            "playlist.delete" => {
                let name = req_str(&params, "name")?;
                let db = self.db();
                Ok(json!({"deleted": db.playlist_delete(&name)?}))
            }
            "playlist.rename" => {
                let old = req_str(&params, "old")?;
                let new = req_str(&params, "new")?;
                let db = self.db();
                db.playlist_rename(&old, &new)?;
                Ok(json!({"ok": true}))
            }
            "playlist.show" => {
                let name = req_str(&params, "name")?;
                let db = self.db();
                Ok(json!(db.playlist_tracks(&name)?))
            }
            "playlist.add" => self.cmd_playlist_add(&params),
            "playlist.remove" => {
                let name = req_str(&params, "name")?;
                let position = req_usize(&params, "position")?;
                let db = self.db();
                db.playlist_remove_position(&name, position)?;
                Ok(json!({"ok": true}))
            }
            "playlist.move" => {
                let name = req_str(&params, "name")?;
                let from = req_usize(&params, "from")?;
                let to = req_usize(&params, "to")?;
                let db = self.db();
                db.playlist_move(&name, from, to)?;
                Ok(json!({"ok": true}))
            }
            "playlist.play" => {
                let name = req_str(&params, "name")?;
                let ids = {
                    let db = self.db();
                    db.playlist_tracks(&name)?
                        .into_iter()
                        .map(|t| t.id)
                        .collect::<Vec<_>>()
                };
                self.play_ids(ids, "replace")
            }
            "library.scan" | "library.rescan" => self.cmd_scan(),
            "library.stats" => {
                let db = self.db();
                Ok(json!(db.stats()?))
            }
            "library.artists" => {
                let db = self.db();
                let artists = db.artists()?;
                Ok(json!(
                    artists
                        .into_iter()
                        .map(|(name, count)| json!({"name": name, "tracks": count}))
                        .collect::<Vec<_>>()
                ))
            }
            "library.genres" => {
                let db = self.db();
                let genres = db.genres()?;
                Ok(json!(
                    genres
                        .into_iter()
                        .map(|(name, count)| json!({"name": name, "tracks": count}))
                        .collect::<Vec<_>>()
                ))
            }
            "library.albums" => {
                let db = self.db();
                Ok(json!(db.albums()?))
            }
            "library.tracks" => {
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(1000) as usize;
                let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let db = self.db();
                Ok(json!(db.all_tracks(limit, offset)?))
            }
            "library.album-tracks" => {
                let album = req_str(&params, "album")?;
                let albumartist = params
                    .get("albumartist")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let db = self.db();
                Ok(json!(db.album_tracks(&album, &albumartist)?))
            }
            "search" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                let sq = SearchQuery::parse(query);
                let db = self.db();
                Ok(json!(db.search(&sq, limit)?))
            }
            "library.resolve" => {
                let query = req_str(&params, "query")?;
                let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
                let db = self.db();
                Ok(json!(db.resolve_tracks(&query, limit)?))
            }
            "mix" => self.cmd_mix(&params),
            "art.get" => self.cmd_art(&params),
            "output.devices" => {
                let devices = muzeec_cli_core::engine::output::CpalOutput::list_devices();
                Ok(json!({"devices": devices}))
            }
            "doctor" => self.cmd_doctor(),
            "config.dump" => Ok(serde_json::to_value(&self.config).unwrap_or(Value::Null)),
            "player.set-ids" => self.cmd_play(&params),
            other => Err(Error::Invalid(format!("unknown command '{other}'"))),
        }
    }

    fn cmd_play(&self, params: &Value) -> Result<Value> {
        let ids = if let Some(arr) = params.get("tracks").and_then(|v| v.as_array()) {
            arr.iter().filter_map(|v| v.as_i64()).collect::<Vec<_>>()
        } else if let Some(q) = params.get("query").and_then(|v| v.as_str()) {
            let query = q.trim();
            if query.is_empty() {
                Vec::new()
            } else if let Some(track_id) = self.resolve_single(query) {
                vec![track_id]
            } else {
                let db = self.db();
                db.resolve_tracks(query, 100)?
            }
        } else {
            Vec::new()
        };
        if ids.is_empty() {
            return Err(Error::NotFound("no matching tracks".into()));
        }
        let mode = params
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("replace");
        self.play_ids(ids, mode)
    }

    /// Resolve an exact path or numeric id to a track id.
    fn resolve_single(&self, query: &str) -> Option<i64> {
        if let Ok(id) = query.parse::<i64>() {
            if self.db().track_by_id(id).ok().flatten().is_some() {
                return Some(id);
            }
        }
        let path = std::path::Path::new(query);
        if path.exists() {
            let abs = path.canonicalize().ok()?;
            let s = abs.to_string_lossy().to_string();
            return self.db().track_by_path(&s).ok().flatten().map(|t| t.id);
        }
        None
    }

    fn play_ids(&self, ids: Vec<i64>, mode: &str) -> Result<Value> {
        match mode {
            "append" | "next" => {
                let mut q = self.queue.lock().unwrap();
                for id in &ids {
                    q.append(*id);
                }
            }
            "now" => {
                let mut q = self.queue.lock().unwrap();
                if let Some(id) = ids.first() {
                    q.play_next(*id);
                    drop(q);
                    self.load_track(*id, 0, true)?;
                }
            }
            _ => {
                let mut q = self.queue.lock().unwrap();
                q.replace(ids.clone(), true);
            }
        }
        let first = ids.first().copied();
        if let Some(id) = first
            && (mode == "now" || mode == "replace")
        {
            self.load_track(id, 0, true)?;
            self.wait_until_started();
        }
        self.save_state();
        Ok(self.status_json())
    }

    /// Briefly wait for the worker to publish the new playing state so the CLI
    /// does not report `stopped` immediately after `play`.
    fn wait_until_started(&self) {
        let deadline = std::time::Instant::now() + Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            if self.engine.snapshot().state != PlaybackState::Stopped {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_state(&self, want: PlaybackState) {
        let deadline = std::time::Instant::now() + Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            if self.engine.snapshot().state == want {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn cmd_next(&self, play: bool) -> Result<Value> {
        let next = {
            let mut q = self.queue.lock().unwrap();
            q.next()
        };
        match next {
            Some(id) => {
                self.load_track(id, 0, play)?;
                self.wait_until_started();
                self.save_state();
                Ok(self.status_json())
            }
            None => {
                self.engine.send(EngineCommand::Stop)?;
                Ok(self.status_json())
            }
        }
    }

    fn cmd_prev(&self) -> Result<Value> {
        let prev = {
            let mut q = self.queue.lock().unwrap();
            q.prev()
        };
        if let Some(id) = prev {
            self.load_track(id, 0, true)?;
        }
        self.save_state();
        Ok(self.status_json())
    }

    fn cmd_seek(&self, params: &Value) -> Result<Value> {
        let snap = self.engine.snapshot();
        if let Some(frame) = params.get("frame").and_then(|v| v.as_u64()) {
            self.engine.send(EngineCommand::Seek { frame })?;
        } else if let Some(delta) = params.get("delta_secs").and_then(|v| v.as_f64()) {
            self.engine
                .send(EngineCommand::SeekRelative { delta_secs: delta })?;
        } else if let Some(pct) = params.get("percent").and_then(|v| v.as_f64()) {
            let target_ms = (snap.duration_ms as f64 * pct / 100.0) as u64;
            let frame = target_ms * self.engine.sample_rate() as u64 / 1000;
            self.engine.send(EngineCommand::Seek { frame })?;
        } else {
            return Err(Error::Invalid(
                "seek needs frame, delta_secs or percent".into(),
            ));
        }
        Ok(self.status_json())
    }

    fn cmd_volume(&self, params: &Value) -> Result<Value> {
        let current = self.engine.snapshot().volume as i64;
        let value = if let Some(v) = params.get("value").and_then(|v| v.as_i64()) {
            v
        } else if let Some(delta) = params.get("delta").and_then(|v| v.as_i64()) {
            current + delta
        } else {
            return Err(Error::Invalid("volume needs value or delta".into()));
        }
        .clamp(0, 100) as u8;
        self.engine.set_volume(value);
        self.engine.send(EngineCommand::SetVolume(value))?;
        Ok(json!({"volume": value}))
    }

    fn cmd_mute(&self, params: &Value) -> Result<Value> {
        let value = params
            .get("value")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| !self.engine.snapshot().muted);
        self.engine.set_muted(value);
        self.engine.send(EngineCommand::SetMuted(value))?;
        Ok(json!({"muted": value}))
    }

    fn cmd_loop(&self, params: &Value) -> Result<Value> {
        let mut q = self.queue.lock().unwrap();
        let mode = if let Some(mode) = params.get("mode").and_then(|v| v.as_str()) {
            LoopMode::parse(mode)
                .ok_or_else(|| Error::Invalid(format!("bad loop mode '{mode}'")))?
        } else {
            q.loop_mode().cycle()
        };
        q.set_loop_mode(mode);
        drop(q);
        self.preload_next();
        Ok(json!({"loop": mode}))
    }

    fn cmd_shuffle(&self, params: &Value) -> Result<Value> {
        let mut q = self.queue.lock().unwrap();
        let value = params
            .get("value")
            .and_then(|v| v.as_bool())
            .unwrap_or(!q.shuffle());
        q.set_shuffle(value);
        if value {
            let seed = now_seed();
            q.shuffle_with_seed(seed);
        }
        Ok(json!({"shuffle": value}))
    }

    fn cmd_queue_list(&self) -> Result<Value> {
        let (items, current) = {
            let q = self.queue.lock().unwrap();
            (q.items().cloned().collect::<Vec<_>>(), q.current_index())
        };
        let db = self.db();
        let mut out = Vec::new();
        for (idx, item) in items.iter().enumerate() {
            let track = db.track_by_id(item.track_id)?;
            out.push(json!({
                "queue_id": item.id,
                "track_id": item.track_id,
                "index": idx,
                "current": Some(idx) == current,
                "title": track.as_ref().map(|t| t.title.clone()),
                "artist": track.as_ref().map(|t| t.artist.clone()),
                "album": track.as_ref().map(|t| t.album.clone()),
                "path": track.as_ref().map(|t| t.path.clone()),
                "duration_ms": track.as_ref().map(|t| t.duration_ms),
            }));
        }
        Ok(json!(out))
    }

    fn cmd_queue_add(&self, params: &Value) -> Result<Value> {
        let ids = req_ids(params)?;
        let mut q = self.queue.lock().unwrap();
        let mut added = Vec::new();
        for id in ids {
            added.push(q.append(id));
        }
        Ok(json!({"added": added}))
    }

    fn cmd_queue_play_next(&self, params: &Value) -> Result<Value> {
        let ids = req_ids(params)?;
        let mut q = self.queue.lock().unwrap();
        let mut added = Vec::new();
        for id in ids {
            added.push(q.play_next(id));
        }
        drop(q);
        self.preload_next();
        Ok(json!({"added": added}))
    }

    fn cmd_queue_remove(&self, params: &Value) -> Result<Value> {
        let id = req_u64(params, "id")?;
        let removed = self.queue.lock().unwrap().remove(id)?;
        Ok(json!({"removed": removed}))
    }

    fn cmd_queue_move(&self, params: &Value) -> Result<Value> {
        let from = req_usize(params, "from")?;
        let to = req_usize(params, "to")?;
        self.queue.lock().unwrap().move_item(from, to)?;
        Ok(json!({"ok": true}))
    }

    fn cmd_queue_save(&self, params: &Value) -> Result<Value> {
        let name = req_str(params, "name")?;
        let ids = {
            let q = self.queue.lock().unwrap();
            q.track_ids()
        };
        let db = self.db();
        db.playlist_create(&name)?;
        db.playlist_add(&name, &ids)?;
        Ok(json!({"ok": true, "tracks": ids.len()}))
    }

    fn cmd_playlist_add(&self, params: &Value) -> Result<Value> {
        let name = req_str(params, "name")?;
        let ids = req_ids(params)?;
        let db = self.db();
        db.playlist_add(&name, &ids)?;
        Ok(json!({"ok": true, "added": ids.len()}))
    }

    fn cmd_scan(&self) -> Result<Value> {
        let opts = ScanOptions {
            dirs: self.config.library.resolved_dirs(),
            follow_symlinks: self.config.library.follow_symlinks,
            workers: self.config.library.scan_workers,
        };
        let db = self.db();
        let scanner = Scanner::new(&db, &self.art);
        let report = scanner.scan(&opts, &|_| {})?;
        Ok(json!({
            "scanned": report.scanned,
            "added": report.added,
            "updated": report.updated,
            "skipped": report.skipped,
            "removed": report.removed,
            "errors": report.errors,
            "duration_ms": report.duration_ms,
        }))
    }

    fn cmd_mix(&self, params: &Value) -> Result<Value> {
        let snap = self.engine.snapshot();
        let seed_track = if let Some(id) = params.get("seed_id").and_then(|v| v.as_i64()) {
            self.db().track_by_id(id)?
        } else {
            snap.track
                .as_ref()
                .and_then(|t| self.db().track_by_id(t.track_id).ok().flatten())
        };
        let seed = seed_track
            .as_ref()
            .map(Seed::from_track)
            .unwrap_or_default();
        let n = params.get("count").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let tracks = self.db().all_tracks(5000, 0)?;
        let candidates: Vec<Candidate> = {
            let ratings_db = self.db();
            tracks
                .into_iter()
                .map(|t| Candidate {
                    rating: ratings_db.rating(t.id).unwrap_or(0),
                    play_count: 0,
                    track: t,
                })
                .collect()
        };
        let avoid: Vec<i64> = {
            let q = self.queue.lock().unwrap();
            q.track_ids()
        };
        let mut rng = rand::rngs::StdRng::seed_from_u64(now_seed());
        use rand::SeedableRng;
        let mix = build_mix(&seed, &candidates, n, &avoid, &mut rng);
        if params
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
        {
            let mut q = self.queue.lock().unwrap();
            for id in &mix {
                q.append(*id);
            }
        }
        self.preload_next();
        Ok(json!({"tracks": mix}))
    }

    fn cmd_art(&self, params: &Value) -> Result<Value> {
        let track_id = req_i64(params, "track_id")?;
        let size = params.get("size").and_then(|v| v.as_u64()).unwrap_or(480) as u32;
        let db = self.db();
        let track = db
            .track_by_id(track_id)?
            .ok_or_else(|| Error::NotFound(format!("track {track_id}")))?;
        drop(db);
        let path = PathBuf::from(&track.path);
        let hash = self.art.resolve(&path, track.art_hash.as_deref());
        let out = match hash {
            Some(h) => {
                let thumb = self.art.thumbnail(&h, size).ok();
                json!({
                    "hash": h,
                    "path": thumb.map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| self.art.path_for(&h).to_string_lossy().to_string()),
                })
            }
            None => json!({"hash": null, "path": null}),
        };
        Ok(out)
    }

    fn cmd_doctor(&self) -> Result<Value> {
        let db = self.db();
        let fts5 = db.fts5_available();
        let stats = db.stats()?;
        drop(db);
        let devices = muzeec_cli_core::engine::output::CpalOutput::list_devices();
        let art_ok = self.art.dir().is_dir();
        let socket = self.paths.socket_path();
        let socket_ok = socket.parent().map(|p| p.exists()).unwrap_or(false);
        Ok(json!({
            "fts5": fts5,
            "library": stats,
            "output_devices": devices,
            "art_cache": art_ok,
            "socket_dir": socket_ok,
            "config_path": self.paths.config_file().to_string_lossy(),
            "db_path": self.paths.db_file().to_string_lossy(),
            "sample_rate": self.engine.sample_rate(),
        }))
    }
}

impl Handler for Daemon {
    fn handle(&self, cmd: &str, params: Value) -> Result<Value> {
        self.dispatch(cmd, params)
    }

    fn subscribe(&self) -> crossbeam_channel::Receiver<Event> {
        self.bus.subscribe()
    }
}

// ---- small JSON helpers -------------------------------------------------

fn req_str(v: &Value, key: &str) -> Result<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("missing string param '{key}'")))
}

fn req_i64(v: &Value, key: &str) -> Result<i64> {
    v.get(key)
        .and_then(|x| x.as_i64())
        .ok_or_else(|| Error::Invalid(format!("missing integer param '{key}'")))
}

fn req_u64(v: &Value, key: &str) -> Result<u64> {
    v.get(key)
        .and_then(|x| x.as_u64())
        .ok_or_else(|| Error::Invalid(format!("missing unsigned param '{key}'")))
}

fn req_usize(v: &Value, key: &str) -> Result<usize> {
    Ok(req_u64(v, key)? as usize)
}

fn req_ids(v: &Value) -> Result<Vec<i64>> {
    let ids = v
        .get("tracks")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect::<Vec<_>>())
        .unwrap_or_default();
    if ids.is_empty() {
        return Err(Error::Invalid("missing 'tracks' array".into()));
    }
    Ok(ids)
}

fn now_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Load config, open the database and start the engine.
pub fn start(paths: Paths, config: Config, art_max_bytes: u64) -> Result<Arc<Daemon>> {
    paths.ensure_dirs()?;
    let db = Database::open(&paths.db_file())?;
    let art = ArtCache::new(paths.art_dir(), art_max_bytes)?;
    let engine = Engine::start(EngineConfig::from_config(&config))?;
    let daemon = Daemon::new(paths, config, db, art, engine);
    daemon.restore();
    daemon.spawn_event_thread();
    Ok(daemon)
}

/// Background file watcher that triggers rescans on change.
pub fn spawn_watcher(daemon: &Arc<Daemon>) {
    if !daemon.config.library.watch {
        return;
    }
    let dirs = daemon.config.library.resolved_dirs();
    let watcher =
        match muzeec_cli_core::library::watch::LibraryWatcher::new(&dirs, Duration::from_secs(2)) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "file watcher disabled");
                return;
            }
        };
    let daemon = daemon.clone();
    std::thread::Builder::new()
        .name("library-watch".into())
        .spawn(move || {
            while let Some(paths) = watcher.recv_timeout(Duration::from_secs(3600)) {
                tracing::info!(changed = paths.len(), "library changed; rescanning");
                if let Err(e) = daemon.dispatch("library.scan", Value::Null) {
                    tracing::warn!(error = %e, "rescan failed");
                }
            }
        })
        .ok();
}
