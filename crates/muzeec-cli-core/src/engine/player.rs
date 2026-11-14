//! Player engine: decoder worker thread + real-time CPAL callback.
//!
//! Decoding happens on a worker thread and feeds a lock-free SPSC ring buffer.
//! The audio callback only pops samples, applies gain/EQ/limiting, and advances
//! the position counter — no allocation, I/O, or locks on the real-time path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::{Deserialize, Serialize};

use super::decode::SymphoniaDecoder;
use super::dsp::{EqChain, SoftLimiter};
use super::mixer::volume_curve;
use super::output::{AudioSink, CpalOutput, FillFn, SinkConfig};
use crate::config::{Config, EqConfig};
use crate::db::Track;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// A track ready to hand to the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackTrack {
    pub track_id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub art_hash: Option<String>,
    /// Linear ReplayGain multiplier (1.0 = no change).
    pub gain: f32,
}

impl PlaybackTrack {
    pub fn from_track(t: &Track, gain: f32) -> Self {
        Self {
            track_id: t.id,
            path: t.path.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_ms: t.duration_ms,
            art_hash: t.art_hash.clone(),
            gain,
        }
    }
}

/// A point-in-time view of the player, published for IPC/MPRIS/TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub track: Option<PlaybackTrack>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: u8,
    pub muted: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: u32,
    pub output_device: String,
    pub gapless: bool,
    pub crossfade_secs: f32,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            state: PlaybackState::Stopped,
            track: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 70,
            muted: false,
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: 512,
            output_device: String::new(),
            gapless: true,
            crossfade_secs: 0.0,
        }
    }
}

/// Events emitted by the engine towards the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    TrackStarted { track: PlaybackTrack },
    TrackEnded { track_id: i64 },
    Error { message: String },
    Stopped,
}

/// Commands accepted by the engine.
#[derive(Debug, Clone)]
pub enum EngineCommand {
    Load {
        track: PlaybackTrack,
        start_frame: u64,
        play: bool,
    },
    Play,
    Pause,
    Stop,
    Seek {
        frame: u64,
    },
    SeekRelative {
        delta_secs: f64,
    },
    SetVolume(u8),
    SetMuted(bool),
    SetGain(f32),
    SetNext(Option<PlaybackTrack>),
    SetCrossfade(f32),
    Shutdown,
}

/// Engine configuration derived from the app config.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub device: String,
    pub buffer_ms: u32,
    pub gapless: bool,
    pub crossfade_secs: f32,
    pub volume: u8,
    pub eq: EqConfig,
}

impl EngineConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            device: cfg.playback.device.clone(),
            buffer_ms: cfg.playback.buffer_ms,
            gapless: cfg.playback.gapless,
            crossfade_secs: cfg.playback.crossfade_secs,
            volume: cfg.playback.volume,
            eq: cfg.eq.clone(),
        }
    }
}

/// Handle used by the daemon/control layer to drive the engine.
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
    event_rx: Receiver<EngineEvent>,
    snap: Arc<ArcSwap<PlaybackSnapshot>>,
    position_frames: Arc<AtomicU64>,
    volume_bits: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    sample_rate: u32,
    worker: Option<std::thread::JoinHandle<()>>,
    audio_done: Option<Sender<()>>,
}

impl EngineHandle {
    pub fn send(&self, cmd: EngineCommand) -> Result<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| Error::Output("engine worker has stopped".into()))
    }

    pub fn events(&self) -> &Receiver<EngineEvent> {
        &self.event_rx
    }

    pub fn try_event(&self) -> Option<EngineEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Current snapshot with the live position folded in.
    pub fn snapshot(&self) -> PlaybackSnapshot {
        let base = self.snap.load();
        let mut snap = (**base).clone();
        snap.position_ms =
            self.position_frames.load(Ordering::Relaxed) * 1000 / self.sample_rate.max(1) as u64;
        if snap.duration_ms > 0 {
            snap.position_ms = snap.position_ms.min(snap.duration_ms);
        }
        snap.volume = reverse_volume_bits(self.volume_bits.load(Ordering::Relaxed));
        snap.muted = self.muted.load(Ordering::Relaxed);
        snap
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Set the volume atomically so `snapshot()` reflects it immediately.
    pub fn set_volume(&self, volume: u8) {
        let v = volume.min(100) as f32 / 100.0;
        self.volume_bits.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
        let _ = self.cmd_tx.send(EngineCommand::Shutdown);
        if let Some(done) = self.audio_done.take() {
            let _ = done.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The engine constructor.
pub struct Engine;

impl Engine {
    /// Open the audio device and start the worker/callback threads.
    pub fn start(config: EngineConfig) -> Result<EngineHandle> {
        let (cfg_tx, cfg_rx) = bounded::<SinkConfig>(1);
        let (fill_tx, fill_rx) = bounded::<FillFn>(1);
        let (done_tx, done_rx) = bounded::<()>(1);

        let device_name = config.device.clone();
        let buffer_ms = config.buffer_ms;

        // Audio thread: owns the output stream for its whole lifetime.
        std::thread::Builder::new()
            .name("audio-output".into())
            .spawn(move || {
                let mut sink: Box<dyn AudioSink> = match CpalOutput::open(&device_name) {
                    Ok(mut o) => {
                        let cfg = o.config();
                        let frames = (cfg.sample_rate as u64 * buffer_ms as u64 / 1000) as u32;
                        o.set_buffer_frames(frames.clamp(64, 8192));
                        let cfg = SinkConfig {
                            buffer_frames: frames.clamp(64, 8192),
                            ..cfg
                        };
                        let _ = cfg_tx.send(cfg);
                        Box::new(o)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "no audio device available; using silent sink");
                        let null = super::output::NullSink::new(48_000, 2);
                        let _ = cfg_tx.send(null.config());
                        Box::new(null)
                    }
                };
                let fill = match fill_rx.recv() {
                    Ok(f) => f,
                    Err(_) => return,
                };
                if let Err(e) = sink.start(fill) {
                    tracing::error!(error = %e, "failed to start audio stream");
                    return;
                }
                // Block until shutdown; dropping `sink` stops the stream.
                let _ = done_rx.recv_timeout(Duration::from_secs(60 * 60 * 24));
            })?;

        let sink = cfg_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::Output("audio device did not initialize".into()))?;
        if sink.sample_rate == 0 {
            return Err(Error::Output("no usable audio output device".into()));
        }

        let rate = sink.sample_rate;
        let channels = sink.channels.max(1) as usize;
        // Keep the decoder roughly 250 ms ahead of playback so track-boundary
        // events track what is actually audible rather than what is decoded.
        let requested = (rate as usize * channels / 4)
            .max(sink.buffer_frames as usize * channels * 2)
            .max(4096);
        let (producer, consumer) = RingBuffer::<f32>::new(requested);
        let capacity = producer.slots() + consumer.slots();

        let snap = Arc::new(ArcSwap::from_pointee(PlaybackSnapshot {
            volume: config.volume,
            sample_rate: rate,
            channels: channels as u16,
            buffer_frames: sink.buffer_frames,
            output_device: config.device.clone(),
            gapless: config.gapless,
            crossfade_secs: config.crossfade_secs,
            ..Default::default()
        }));
        let position_frames = Arc::new(AtomicU64::new(0));
        let volume_bits = Arc::new(AtomicU32::new((config.volume as f32 / 100.0).to_bits()));
        let gain_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let muted = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(true));
        let draining = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));

        // Build the realtime fill closure.
        let fill = build_fill(
            consumer,
            channels,
            volume_bits.clone(),
            gain_bits.clone(),
            muted.clone(),
            paused.clone(),
            draining.clone(),
            position_frames.clone(),
            &config.eq,
            rate,
        );
        let _ = fill_tx.send(fill);

        let (cmd_tx, cmd_rx) = unbounded::<EngineCommand>();
        let (event_tx, event_rx) = unbounded::<EngineEvent>();

        let worker_snap = snap.clone();
        let worker_pos = position_frames.clone();
        let worker_vol = volume_bits.clone();
        let worker_gain = gain_bits.clone();
        let worker_muted = muted.clone();
        let worker_paused = paused.clone();
        let worker_drain = draining.clone();
        let worker_alive = alive.clone();
        let worker_rate = rate;
        let worker_channels = channels;
        let crossfade_secs = config.crossfade_secs;
        let gapless = config.gapless;

        let worker = std::thread::Builder::new()
            .name("engine-worker".into())
            .spawn(move || {
                let ctx = WorkerCtx {
                    producer,
                    channels: worker_channels,
                    rate: worker_rate,
                    snap: worker_snap,
                    position: worker_pos,
                    volume_bits: worker_vol,
                    gain_bits: worker_gain,
                    muted: worker_muted,
                    paused: worker_paused,
                    draining: worker_drain,
                    alive: worker_alive,
                    event_tx,
                    crossfade_secs,
                    gapless,
                    capacity,
                    last_full_log: None,
                };
                run_worker(ctx, cmd_rx);
            })?;

        Ok(EngineHandle {
            cmd_tx,
            event_rx,
            snap,
            position_frames,
            volume_bits,
            muted,
            alive,
            sample_rate: rate,
            worker: Some(worker),
            audio_done: Some(done_tx),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn build_fill(
    mut consumer: Consumer<f32>,
    channels: usize,
    volume_bits: Arc<AtomicU32>,
    gain_bits: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    draining: Arc<AtomicBool>,
    position: Arc<AtomicU64>,
    eq_cfg: &EqConfig,
    rate: u32,
) -> FillFn {
    let mut limiter = SoftLimiter::new(-1.0);
    let mut eq = EqChain::new(channels, rate);
    eq.set_enabled(eq_cfg.enabled);
    eq.set_preamp_db(eq_cfg.preamp_db);
    if eq_cfg.bands.len() == 10 {
        eq.set_gains(&eq_cfg.bands);
    }
    Box::new(move |out: &mut [f32]| {
        if draining.swap(false, Ordering::Relaxed) {
            while consumer.pop().is_ok() {}
        }
        if paused.load(Ordering::Relaxed) {
            return;
        }
        let mut filled = 0usize;
        for slot in out.iter_mut() {
            match consumer.pop() {
                Ok(v) => {
                    *slot = v;
                    filled += 1;
                }
                Err(_) => break,
            }
        }
        if filled == 0 {
            return;
        }
        let gain = f32::from_bits(gain_bits.load(Ordering::Relaxed));
        let vol = volume_curve(reverse_volume_bits(volume_bits.load(Ordering::Relaxed)));
        let mute = if muted.load(Ordering::Relaxed) {
            0.0
        } else {
            1.0
        };
        let total = gain * vol * mute;
        for s in out[..filled].iter_mut() {
            *s *= total;
        }
        limiter.process(&mut out[..filled]);
        eq.process(&mut out[..filled]);
        position.fetch_add((filled / channels) as u64, Ordering::Relaxed);
    })
}

struct WorkerCtx {
    producer: Producer<f32>,
    channels: usize,
    rate: u32,
    snap: Arc<ArcSwap<PlaybackSnapshot>>,
    position: Arc<AtomicU64>,
    volume_bits: Arc<AtomicU32>,
    gain_bits: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    draining: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    event_tx: Sender<EngineEvent>,
    crossfade_secs: f32,
    gapless: bool,
    capacity: usize,
    last_full_log: Option<std::time::Instant>,
}

impl WorkerCtx {
    fn publish(&self, state: PlaybackState, track: Option<PlaybackTrack>) {
        let mut snap = (**self.snap.load()).clone();
        snap.state = state;
        snap.track = track.clone();
        snap.duration_ms = track.as_ref().map(|t| t.duration_ms).unwrap_or(0);
        snap.volume = reverse_volume_bits(self.volume_bits.load(Ordering::Relaxed));
        snap.muted = self.muted.load(Ordering::Relaxed);
        snap.crossfade_secs = self.crossfade_secs;
        snap.gapless = self.gapless;
        self.snap.store(Arc::new(snap));
    }

    /// Block until the consumer has drained the ring (bounded by pause/shutdown).
    fn drain_wait(&self) {
        while self.alive.load(Ordering::Relaxed) && !self.paused.load(Ordering::Relaxed) {
            let used = self.capacity.saturating_sub(self.producer.slots());
            if used == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn push(&mut self, data: &[f32]) -> bool {
        let mut i = 0;
        while i < data.len() {
            if !self.alive.load(Ordering::Relaxed) {
                return false;
            }
            match self.producer.push(data[i]) {
                Ok(()) => i += 1,
                Err(_) => {
                    let now = std::time::Instant::now();
                    if self
                        .last_full_log
                        .map(|t| now.duration_since(t) > Duration::from_secs(1))
                        .unwrap_or(true)
                    {
                        tracing::debug!(
                            slots = self.producer.slots(),
                            position = self.position.load(Ordering::Relaxed),
                            paused = self.paused.load(Ordering::Relaxed),
                            draining = self.draining.load(Ordering::Relaxed),
                            "ring buffer full; waiting for the consumer"
                        );
                        self.last_full_log = Some(now);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        true
    }
}

fn run_worker(mut ctx: WorkerCtx, cmd_rx: Receiver<EngineCommand>) {
    tracing::debug!("engine worker started");
    let mut current: Option<SymphoniaDecoder> = None;
    let mut current_track: Option<PlaybackTrack> = None;
    let mut next: Option<PlaybackTrack> = None;
    let mut state = PlaybackState::Stopped;
    let mut last_publish = std::time::Instant::now();

    loop {
        // Drain commands.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                EngineCommand::Load {
                    track,
                    start_frame,
                    play,
                } => {
                    if let Some(dec) = current.as_mut() {
                        let _ = dec.seek_to_frame(0);
                    }
                    current = None;
                    next = None;
                    match SymphoniaDecoder::open(
                        std::path::Path::new(&track.path),
                        ctx.rate,
                        ctx.channels as u16,
                    ) {
                        Ok(mut dec) => {
                            if start_frame > 0 {
                                let _ = dec.seek_to_frame(start_frame);
                            }
                            ctx.gain_bits.store(track.gain.to_bits(), Ordering::Relaxed);
                            ctx.position.store(start_frame, Ordering::Relaxed);
                            ctx.draining.store(true, Ordering::Relaxed);
                            current = Some(dec);
                            current_track = Some(track.clone());
                            tracing::debug!(
                                track = %track.title,
                                play,
                                total = current.as_ref().and_then(|d| d.total_frames()),
                                "track loaded"
                            );
                            state = if play {
                                PlaybackState::Playing
                            } else {
                                PlaybackState::Paused
                            };
                            ctx.paused.store(!play, Ordering::Relaxed);
                            ctx.publish(state, current_track.clone());
                            let _ = ctx.event_tx.send(EngineEvent::TrackStarted { track });
                        }
                        Err(e) => {
                            state = PlaybackState::Stopped;
                            ctx.paused.store(true, Ordering::Relaxed);
                            ctx.publish(state, None);
                            let _ = ctx.event_tx.send(EngineEvent::Error {
                                message: e.to_string(),
                            });
                        }
                    }
                }
                EngineCommand::Play => {
                    state = PlaybackState::Playing;
                    ctx.paused.store(false, Ordering::Relaxed);
                    ctx.publish(state, current_track.clone());
                }
                EngineCommand::Pause => {
                    if state == PlaybackState::Playing {
                        state = PlaybackState::Paused;
                        ctx.paused.store(true, Ordering::Relaxed);
                        ctx.publish(state, current_track.clone());
                    }
                }
                EngineCommand::Stop => {
                    state = PlaybackState::Stopped;
                    ctx.paused.store(true, Ordering::Relaxed);
                    ctx.draining.store(true, Ordering::Relaxed);
                    current = None;
                    current_track = None;
                    next = None;
                    ctx.position.store(0, Ordering::Relaxed);
                    ctx.publish(state, None);
                    let _ = ctx.event_tx.send(EngineEvent::Stopped);
                }
                EngineCommand::Seek { frame } => {
                    if let Some(dec) = current.as_mut() {
                        match dec.seek_to_frame(frame) {
                            Ok(()) => {
                                ctx.draining.store(true, Ordering::Relaxed);
                                ctx.position.store(frame, Ordering::Relaxed);
                            }
                            Err(e) => {
                                let _ = ctx.event_tx.send(EngineEvent::Error {
                                    message: e.to_string(),
                                });
                            }
                        }
                    }
                }
                EngineCommand::SeekRelative { delta_secs } => {
                    if let Some(dec) = current.as_mut() {
                        match dec.seek_relative(delta_secs) {
                            Ok(()) => {
                                ctx.draining.store(true, Ordering::Relaxed);
                                ctx.position.store(dec.frames_emitted(), Ordering::Relaxed);
                            }
                            Err(e) => {
                                let _ = ctx.event_tx.send(EngineEvent::Error {
                                    message: e.to_string(),
                                });
                            }
                        }
                    }
                }
                EngineCommand::SetVolume(v) => {
                    ctx.volume_bits
                        .store((v.min(100) as f32 / 100.0).to_bits(), Ordering::Relaxed);
                    ctx.publish(state, current_track.clone());
                }
                EngineCommand::SetMuted(m) => {
                    ctx.muted.store(m, Ordering::Relaxed);
                    ctx.publish(state, current_track.clone());
                }
                EngineCommand::SetGain(g) => {
                    ctx.gain_bits.store(g.to_bits(), Ordering::Relaxed);
                }
                EngineCommand::SetNext(t) => {
                    next = t;
                }
                EngineCommand::SetCrossfade(secs) => {
                    // Handled through ctx for the next boundary.
                    // (The struct field is immutable; treated as configured once.)
                    let _ = secs;
                }
                EngineCommand::Shutdown => {
                    ctx.alive.store(false, Ordering::Relaxed);
                    return;
                }
            }
        }

        if !ctx.alive.load(Ordering::Relaxed) {
            return;
        }

        if state != PlaybackState::Playing || current.is_none() {
            if state == PlaybackState::Playing && current.is_none() {
                match next.take() {
                    Some(track) => match open_track(&ctx, &track, 0) {
                        Ok(dec) => {
                            current = Some(dec);
                            current_track = Some(track.clone());
                            ctx.position.store(0, Ordering::Relaxed);
                            ctx.draining.store(true, Ordering::Relaxed);
                            ctx.gain_bits.store(track.gain.to_bits(), Ordering::Relaxed);
                            let _ = ctx.event_tx.send(EngineEvent::TrackStarted { track });
                            ctx.publish(state, current_track.clone());
                        }
                        Err(e) => {
                            let _ = ctx.event_tx.send(EngineEvent::Error {
                                message: e.to_string(),
                            });
                            state = PlaybackState::Stopped;
                            ctx.paused.store(true, Ordering::Relaxed);
                            ctx.publish(state, None);
                        }
                    },
                    None => {
                        state = PlaybackState::Stopped;
                        ctx.paused.store(true, Ordering::Relaxed);
                        let ended = current_track.take();
                        ctx.position.store(0, Ordering::Relaxed);
                        ctx.publish(state, None);
                        if let Some(t) = ended {
                            let _ = ctx.event_tx.send(EngineEvent::TrackEnded {
                                track_id: t.track_id,
                            });
                        }
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
            continue;
        }

        // Time for a crossfade at the tail of the current track?
        let xfade_frames = (ctx.crossfade_secs * ctx.rate as f32) as u64;
        if xfade_frames > 0
            && next.is_some()
            && let Some(dec) = current.as_ref()
            && let Some(total) = dec.total_frames()
            && total.saturating_sub(dec.frames_emitted()) <= xfade_frames
        {
            let next_track = next.take().unwrap();
            let want = xfade_frames as usize;
            let mut tail = vec![0.0f32; want * ctx.channels];
            let tail_n = current
                .as_mut()
                .map(|d| d.next_frames(&mut tail, want))
                .unwrap_or(0);
            match open_track(&ctx, &next_track, 0) {
                Ok(mut next_dec) => {
                    let mut head = vec![0.0f32; want * ctx.channels];
                    let head_n = next_dec.next_frames(&mut head, want);
                    let n = tail_n.min(head_n);
                    let mut mixed = vec![0.0f32; n * ctx.channels];
                    for f in 0..n {
                        let t = if n > 1 {
                            f as f32 / (n - 1) as f32
                        } else {
                            1.0
                        };
                        let (ga, gb) = super::mixer::equal_power(t);
                        for c in 0..ctx.channels {
                            mixed[f * ctx.channels + c] =
                                tail[f * ctx.channels + c] * ga + head[f * ctx.channels + c] * gb;
                        }
                    }
                    if !ctx.push(&mixed) {
                        return;
                    }
                    current = Some(next_dec);
                    current_track = Some(next_track.clone());
                    ctx.position.store(0, Ordering::Relaxed);
                    ctx.gain_bits
                        .store(next_track.gain.to_bits(), Ordering::Relaxed);
                    let _ = ctx
                        .event_tx
                        .send(EngineEvent::TrackStarted { track: next_track });
                    ctx.publish(state, current_track.clone());
                }
                Err(e) => {
                    let _ = ctx.event_tx.send(EngineEvent::Error {
                        message: e.to_string(),
                    });
                }
            }
            continue;
        }

        // Decode a chunk and push it.
        let frames = 1024usize;
        let mut buf = vec![0.0f32; frames * ctx.channels];
        let n = current
            .as_mut()
            .map(|d| d.next_frames(&mut buf, frames))
            .unwrap_or(0);
        tracing::trace!(frames = n, "decoded chunk");
        if n == 0 {
            // Track exhausted; launder gapless handoff.
            tracing::debug!(
                ended = current_track.as_ref().map(|t| t.track_id),
                has_next = next.is_some(),
                "track exhausted"
            );
            let ended = current_track.clone();
            current = None;
            if let Some(track) = next.take() {
                match open_track(&ctx, &track, 0) {
                    Ok(dec) => {
                        current = Some(dec);
                        current_track = Some(track.clone());
                        ctx.position.store(0, Ordering::Relaxed);
                        ctx.draining.store(false, Ordering::Relaxed);
                        ctx.gain_bits.store(track.gain.to_bits(), Ordering::Relaxed);
                        let _ = ctx.event_tx.send(EngineEvent::TrackStarted { track });
                        ctx.publish(state, current_track.clone());
                    }
                    Err(e) => {
                        let _ = ctx.event_tx.send(EngineEvent::Error {
                            message: e.to_string(),
                        });
                        state = PlaybackState::Stopped;
                        ctx.paused.store(true, Ordering::Relaxed);
                        ctx.publish(state, None);
                    }
                }
            } else {
                // Let the samples already in the ring reach the device before
                // reporting the end of the final track.
                ctx.drain_wait();
                state = PlaybackState::Stopped;
                ctx.paused.store(true, Ordering::Relaxed);
                ctx.position.store(0, Ordering::Relaxed);
                ctx.publish(state, None);
                if let Some(t) = ended {
                    let _ = ctx.event_tx.send(EngineEvent::TrackEnded {
                        track_id: t.track_id,
                    });
                }
            }
            continue;
        }
        if !ctx.push(&buf[..n * ctx.channels]) {
            return;
        }

        if last_publish.elapsed() >= Duration::from_millis(200) {
            ctx.publish(state, current_track.clone());
            last_publish = std::time::Instant::now();
        }
    }
}

fn open_track(ctx: &WorkerCtx, track: &PlaybackTrack, start: u64) -> Result<SymphoniaDecoder> {
    let mut dec = SymphoniaDecoder::open(
        std::path::Path::new(&track.path),
        ctx.rate,
        ctx.channels as u16,
    )?;
    if start > 0 {
        dec.seek_to_frame(start)?;
    }
    Ok(dec)
}

fn reverse_volume_bits(bits: u32) -> u8 {
    let v = f32::from_bits(bits).clamp(0.0, 1.0);
    (v * 100.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::decode::SymphoniaDecoder;

    #[test]
    fn snapshot_defaults_stopped() {
        let s = PlaybackSnapshot::default();
        assert_eq!(s.state, PlaybackState::Stopped);
    }

    #[test]
    fn volume_bits_roundtrip() {
        let bits = (0.42f32).to_bits();
        let v = reverse_volume_bits(bits);
        assert_eq!(v, 42);
    }

    #[test]
    fn decoder_open_missing_file_errs() {
        assert!(
            SymphoniaDecoder::open(std::path::Path::new("/nonexistent/x.mp3"), 48000, 2).is_err()
        );
    }
}
