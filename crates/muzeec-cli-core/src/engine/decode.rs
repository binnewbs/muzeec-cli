//! Symphonia-backed decoder producing interleaved f32 at the engine rate.

use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;

use symphonia::core::audio::AudioSpec;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::common::Tier;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use super::resample::LinearResampler;
use crate::error::{Error, Result};

/// A decoder that converts any supported file to interleaved f32 at a target
/// sample rate and channel count.
pub struct SymphoniaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    src_rate: u32,
    src_channels: usize,
    target_rate: u32,
    target_channels: usize,
    /// Interleaved target-channel frames at the source rate, awaiting resample.
    pending: VecDeque<f32>,
    resampler: LinearResampler,
    eof: bool,
    finished: bool,
    frames_emitted: u64,
    duration_frames: Option<u64>,
    /// Scratch for decoded packet samples.
    tmp: Vec<f32>,
}

impl SymphoniaDecoder {
    /// Open `path`, decoding to `target_rate`/`target_channels`.
    pub fn open(path: &Path, target_rate: u32, target_channels: u16) -> Result<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| Error::Decode(format!("probe {}: {e}", path.display())))?;
        let format = format;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| Error::Decode("no audio track".into()))?;
        let codec_params = track
            .codec_params
            .as_ref()
            .and_then(|c| c.audio())
            .ok_or_else(|| Error::Decode("track has no audio codec parameters".into()))?;
        let src_rate = codec_params.sample_rate.unwrap_or(target_rate).max(1);
        let src_channels = codec_params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(target_channels as usize);
        let track_id = track.id;
        let num_frames = track.num_frames;
        let decoder = build_registry()
            .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
            .map_err(|e| Error::Decode(format!("codec: {e}")))?;

        let target_channels = target_channels.max(1) as usize;
        let resampler = LinearResampler::new(src_rate, target_rate, target_channels);
        let duration_frames =
            num_frames.map(|n| ((n as u128 * target_rate as u128) / src_rate as u128) as u64);

        Ok(Self {
            format,
            decoder,
            track_id,
            src_rate,
            src_channels,
            target_rate,
            target_channels,
            pending: VecDeque::new(),
            resampler,
            eof: false,
            finished: false,
            frames_emitted: 0,
            duration_frames,
            tmp: Vec::new(),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.target_rate
    }

    pub fn channels(&self) -> usize {
        self.target_channels
    }

    pub fn frames_emitted(&self) -> u64 {
        self.frames_emitted
    }

    pub fn duration_frames(&self) -> Option<u64> {
        self.duration_frames
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_frames
            .map(|f| f * 1000 / self.target_rate as u64)
            .unwrap_or(0)
    }

    /// Decode one packet into the pending queue. Returns false at EOF.
    fn decode_packet(&mut self) -> bool {
        if self.eof {
            return false;
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => {
                    self.eof = true;
                    return false;
                }
                Err(SymError::ResetRequired) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, "decode read error");
                    self.eof = true;
                    return false;
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(audio) => {
                    let spec: &AudioSpec = audio.spec();
                    let rate = spec.rate();
                    let ch = spec.channels().count().max(1);
                    if rate != self.src_rate || ch != self.src_channels {
                        self.src_rate = rate;
                        self.src_channels = ch;
                    }
                    let n = audio.samples_interleaved();
                    self.tmp.resize(n, 0.0);
                    audio.copy_to_slice_interleaved(&mut self.tmp[..]);
                    self.push_converted();
                    return true;
                }
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, "decoder error");
                    self.eof = true;
                    return false;
                }
            }
        }
    }

    /// Convert `tmp` (source channels) into target-channel frames in `pending`.
    fn push_converted(&mut self) {
        let src_ch = self.src_channels.max(1);
        let dst_ch = self.target_channels;
        if src_ch == dst_ch {
            self.pending.extend(self.tmp.iter().copied());
            return;
        }
        if dst_ch == 1 {
            for frame in self.tmp.chunks_exact(src_ch) {
                let sum: f32 = frame.iter().sum();
                self.pending.push_back(sum / src_ch as f32);
            }
        } else if dst_ch == 2 {
            if src_ch == 1 {
                for &s in &self.tmp {
                    self.pending.push_back(s);
                    self.pending.push_back(s);
                }
            } else if src_ch == 2 {
                self.pending.extend(self.tmp.iter().copied());
            } else {
                // Many channels -> mono -> stereo.
                for frame in self.tmp.chunks_exact(src_ch) {
                    let sum: f32 = frame.iter().sum();
                    let m = sum / src_ch as f32;
                    self.pending.push_back(m);
                    self.pending.push_back(m);
                }
            }
        } else {
            // Up-mix: replicate the first two channels, zero the rest.
            for frame in self.tmp.chunks_exact(src_ch) {
                for c in 0..dst_ch {
                    let v = if c < src_ch {
                        frame[c]
                    } else if src_ch > 0 {
                        frame[c % src_ch]
                    } else {
                        0.0
                    };
                    self.pending.push_back(v);
                }
            }
        }
    }

    /// Ensure at least one frame is available in `pending`.
    fn ensure_frame(&mut self) -> bool {
        while self.pending.len() < self.target_channels {
            if !self.decode_packet() {
                return false;
            }
        }
        true
    }

    fn pop_frame(&mut self, out: &mut [f32]) -> bool {
        if !self.ensure_frame() {
            return false;
        }
        for slot in out.iter_mut().take(self.target_channels) {
            *slot = self.pending.pop_front().unwrap_or(0.0);
        }
        true
    }

    /// Fill `out` (interleaved at target format) with up to `frames` frames.
    /// Returns the number of frames written.
    pub fn next_frames(&mut self, out: &mut [f32], frames: usize) -> usize {
        let ch = self.target_channels;
        let mut written = 0usize;
        if self.finished {
            return 0;
        }

        // Prime the interpolator.
        if !self.resampler.is_primed() {
            let mut first = vec![0.0f32; ch];
            let mut second = vec![0.0f32; ch];
            if !self.pop_frame(&mut first) {
                self.finished = true;
                return 0;
            }
            if !self.pop_frame(&mut second) {
                // Single-frame track: emit it directly.
                out[..ch].copy_from_slice(&first);
                self.frames_emitted += 1;
                self.finished = true;
                return 1;
            }
            self.resampler.prime(&first, &second);
        }

        let mut next = vec![0.0f32; ch];
        while written < frames {
            self.resampler
                .interpolate(&mut out[written * ch..(written + 1) * ch]);
            self.frames_emitted += 1;
            written += 1;

            let shifts = self.resampler.advance();
            for _ in 0..shifts {
                if self.pop_frame(&mut next) {
                    self.resampler.push_frame(&next);
                } else {
                    // End of stream: the frame just written is the last valid one.
                    self.finished = true;
                    return written;
                }
            }
        }
        written
    }

    /// Total frames that will be produced (if known).
    pub fn total_frames(&self) -> Option<u64> {
        self.duration_frames
    }

    /// Seek to a target frame (at the engine rate), resetting resampler state.
    pub fn seek_to_frame(&mut self, frame: u64) -> Result<()> {
        let secs = frame / self.target_rate.max(1) as u64;
        let frac_ns = ((frame % self.target_rate.max(1) as u64) * 1_000_000_000
            / self.target_rate.max(1) as u64) as u32;
        let time = if secs < 60 {
            Time::from_ss(secs as u8, frac_ns)
        } else {
            Time::from_hhmmss(
                (secs / 3600).min(u32::MAX as u64) as u32,
                ((secs % 3600) / 60) as u8,
                (secs % 60) as u8,
                frac_ns,
            )
        }
        .ok_or_else(|| Error::Invalid("invalid seek time".into()))?;
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| Error::Decode(format!("seek: {e}")))?;
        self.pending.clear();
        self.resampler.reset();
        self.eof = false;
        self.finished = false;
        self.frames_emitted = frame;
        Ok(())
    }

    /// Seek by seconds relative to the current position.
    pub fn seek_relative(&mut self, delta_secs: f64) -> Result<()> {
        let cur = self.frames_emitted as f64 / self.target_rate.max(1) as f64;
        let target = (cur + delta_secs).max(0.0);
        let frame = (target * self.target_rate as f64) as u64;
        self.seek_to_frame(frame)
    }

    pub fn track_id(&self) -> u32 {
        self.track_id
    }
}

/// Build a codec registry including all enabled Symphonia codecs plus the
/// pure-Rust Opus decoder.
fn build_registry() -> CodecRegistry {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder_at_tier::<symphonia_opus::OpusDecoder>(Tier::Fallback);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a tiny 16-bit PCM WAV sine so tests do not need fixtures.
    fn write_wav(path: &Path, rate: u32, secs: f32, freq: f32) {
        let n = (rate as f32 * secs) as u32;
        let mut data = Vec::with_capacity((n * 2) as usize);
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let s = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5;
            let v = (s * i16::MAX as f32) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&v.to_le_bytes());
        }
        let data_len = data.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&2u16.to_le_bytes()); // stereo
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn decodes_wav_and_resamples() {
        let dir = std::env::temp_dir().join(format!("muzeec-dec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_wav(&path, 44100, 0.1, 440.0);

        let mut dec = SymphoniaDecoder::open(&path, 48000, 2).unwrap();
        assert_eq!(dec.sample_rate(), 48000);
        let mut out = vec![0.0f32; 48000 * 2];
        let n = dec.next_frames(&mut out, 48000);
        // ~0.1s at 48k = ~4800 frames.
        assert!(n > 4000 && n < 5200, "unexpected frame count {n}");
        // Signal should be non-silent.
        let peak = out[..n * 2].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.1, "decoded signal too quiet: {peak}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seek_resets_position() {
        let dir = std::env::temp_dir().join(format!("muzeec-dec-seek-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        write_wav(&path, 48000, 1.0, 440.0);
        let mut dec = SymphoniaDecoder::open(&path, 48000, 2).unwrap();
        dec.seek_to_frame(24000).unwrap();
        let mut out = vec![0.0f32; 4800 * 2];
        let n = dec.next_frames(&mut out, 4800);
        assert_eq!(n, 4800);
        std::fs::remove_dir_all(&dir).ok();
    }
}
