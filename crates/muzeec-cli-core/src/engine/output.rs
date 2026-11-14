//! Audio output backends behind a small sink abstraction.

use std::io::Write;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::error::{Error, Result};

/// Callback that fills an interleaved f32 buffer.
pub type FillFn = Box<dyn FnMut(&mut [f32]) + Send>;

/// Output format negotiated with the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: u32,
}

/// A playback sink.
pub trait AudioSink: Send {
    fn config(&self) -> SinkConfig;
    fn start(&mut self, fill: FillFn) -> Result<()>;
    fn pause(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
}

/// The real CPAL output.
pub struct CpalOutput {
    device: Option<cpal::Device>,
    sample_format: SampleFormat,
    config: StreamConfig,
    sink: SinkConfig,
    stream: Option<Stream>,
}

impl CpalOutput {
    /// Open the named output device (`"default"` follows the system default).
    pub fn open(device_name: &str) -> Result<Self> {
        let host = cpal::default_host();
        let device = if device_name.is_empty() || device_name == "default" {
            host.default_output_device()
                .ok_or_else(|| Error::Output("no default output device".into()))?
        } else {
            let mut found = None;
            if let Ok(devices) = host.output_devices() {
                for dev in devices {
                    let name = dev
                        .description()
                        .map(|d| d.name().to_string())
                        .unwrap_or_default();
                    let id = dev.id().map(|i| i.to_string()).unwrap_or_default();
                    if name.eq_ignore_ascii_case(device_name)
                        || name.to_lowercase().contains(&device_name.to_lowercase())
                        || id == device_name
                    {
                        found = Some(dev);
                        break;
                    }
                }
            }
            found
                .ok_or_else(|| Error::Output(format!("output device '{device_name}' not found")))?
        };

        let supported = device.default_output_config()?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.config();
        Ok(Self {
            device: Some(device),
            sample_format,
            config,
            sink: SinkConfig {
                sample_rate: config.sample_rate,
                channels: config.channels as u16,
                buffer_frames: 512,
            },
            stream: None,
        })
    }

    pub fn set_buffer_frames(&mut self, frames: u32) {
        self.sink.buffer_frames = frames;
        self.config.buffer_size = cpal::BufferSize::Fixed(frames);
    }

    pub fn sample_format(&self) -> SampleFormat {
        self.sample_format
    }

    /// Enumerate output device names (deduplicated).
    pub fn list_devices() -> Vec<String> {
        use std::collections::BTreeSet;
        let host = cpal::default_host();
        let mut names = BTreeSet::new();
        if let Ok(devices) = host.output_devices() {
            for d in devices {
                if let Ok(desc) = d.description() {
                    names.insert(desc.name().to_string());
                }
            }
        }
        names.into_iter().collect()
    }
}

impl AudioSink for CpalOutput {
    fn config(&self) -> SinkConfig {
        self.sink
    }

    fn start(&mut self, fill: FillFn) -> Result<()> {
        let device = self
            .device
            .take()
            .ok_or_else(|| Error::Output("output device already consumed".into()))?;
        let config = self.config;
        let sample_format = self.sample_format;
        let stream = build_stream(&device, config, sample_format, fill)?;
        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if let Some(s) = &self.stream {
            s.pause()?;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if let Some(s) = &self.stream {
            s.play()?;
        }
        Ok(())
    }
}

fn build_stream(
    device: &cpal::Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    fill: FillFn,
) -> Result<Stream> {
    match sample_format {
        SampleFormat::F32 => run::<f32>(device, config, fill),
        SampleFormat::I16 => run::<i16>(device, config, fill),
        SampleFormat::I32 => run::<i32>(device, config, fill),
        SampleFormat::I64 => run::<i64>(device, config, fill),
        SampleFormat::U8 => run::<u8>(device, config, fill),
        SampleFormat::U16 => run::<u16>(device, config, fill),
        SampleFormat::U32 => run::<u32>(device, config, fill),
        SampleFormat::U64 => run::<u64>(device, config, fill),
        SampleFormat::I8 => run::<i8>(device, config, fill),
        SampleFormat::F64 => run::<f64>(device, config, fill),
        other => Err(Error::Output(format!("unsupported sample format {other}"))),
    }
}

fn run<T>(device: &cpal::Device, config: StreamConfig, mut fill: FillFn) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    // Preallocate a scratch buffer; only grows if the backend hands us a larger
    // callback than expected.
    let mut scratch = vec![0.0f32; 8192 * channels];
    let err_fn = |err: cpal::Error| {
        tracing::warn!(error = %err, "audio stream error");
    };
    let stream = device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
                let n = data.len();
                if scratch.len() < n {
                    scratch.resize(n, 0.0);
                }
                scratch[..n].fill(0.0);
                fill(&mut scratch[..n]);
                for (out, inp) in data.iter_mut().zip(scratch[..n].iter()) {
                    *out = T::from_sample(*inp);
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| Error::Output(e.to_string()))?;
    Ok(stream)
}

/// A silent sink driven by a real-time clock; useful for headless tests and
/// `--record`-style tooling.
pub struct NullSink {
    sink: SinkConfig,
    stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl NullSink {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sink: SinkConfig {
                sample_rate,
                channels,
                buffer_frames: 512,
            },
            stop: None,
            handle: None,
        }
    }
}

impl AudioSink for NullSink {
    fn config(&self) -> SinkConfig {
        self.sink
    }

    fn start(&mut self, mut fill: FillFn) -> Result<()> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let frames = self.sink.buffer_frames as usize;
        let channels = self.sink.channels as usize;
        let sleep =
            std::time::Duration::from_secs_f64(frames as f64 / self.sink.sample_rate as f64);
        let handle = std::thread::Builder::new()
            .name("null-sink".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; frames * channels];
                while !stop2.load(Ordering::Relaxed) {
                    buf.fill(0.0);
                    fill(&mut buf);
                    std::thread::sleep(sleep);
                }
            })?;
        self.stop = Some(stop);
        self.handle = Some(handle);
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if let Some(stop) = &self.stop {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if let Some(stop) = &self.stop {
            stop.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Minimal streaming WAV writer (16-bit PCM) for offline rendering and tests.
pub struct WavWriter {
    file: std::io::BufWriter<std::fs::File>,
    channels: u16,
    sample_rate: u32,
    frames_written: u32,
}

impl WavWriter {
    pub fn create(path: &std::path::Path, sample_rate: u32, channels: u16) -> Result<Self> {
        let file = std::fs::File::create(path)?;
        let mut w = std::io::BufWriter::new(file);
        // Placeholder header; fixed up on close.
        w.write_all(&[0u8; 44])?;
        Ok(Self {
            file: w,
            channels,
            sample_rate,
            frames_written: 0,
        })
    }

    pub fn write_interleaved(&mut self, samples: &[f32]) -> Result<()> {
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            self.file.write_all(&v.to_le_bytes())?;
        }
        self.frames_written += (samples.len() / self.channels.max(1) as usize) as u32;
        Ok(())
    }

    /// Finalize the header and flush.
    pub fn finish(self) -> Result<()> {
        let data_len = self.frames_written * self.channels as u32 * 2;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + data_len).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&self.channels.to_le_bytes());
        header.extend_from_slice(&self.sample_rate.to_le_bytes());
        header.extend_from_slice(&(self.sample_rate * self.channels as u32 * 2).to_le_bytes());
        header.extend_from_slice(&(self.channels * 2).to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_len.to_le_bytes());
        let file = self
            .file
            .into_inner()
            .map_err(|e| Error::Io(e.into_error()))?;
        use std::io::{Seek, SeekFrom};
        let mut file = file;
        file.seek(SeekFrom::Start(0))?;
        std::io::Write::write_all(&mut file, &header)?;
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_writer_roundtrip_header() {
        let path = std::env::temp_dir().join(format!("muzeec-wav-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&path, 48000, 2).unwrap();
        w.write_interleaved(&[0.0, 0.0, 0.5, -0.5]).unwrap();
        w.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(bytes.len(), 44 + 8);
        std::fs::remove_file(&path).ok();
    }
}
