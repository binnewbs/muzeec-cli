//! ReplayGain support: reading tags, applying gain, and EBU R128 analysis.

use crate::db::Track;
use crate::error::{Error, Result};

/// Effective replay-gain information for a track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgInfo {
    pub gain_db: f32,
    pub peak: Option<f32>,
}

/// Choose gain based on the configured mode (`off`/`track`/`album`).
pub fn track_gain(track: &Track, mode: crate::config::ReplayGainMode) -> Option<RgInfo> {
    use crate::config::ReplayGainMode::*;
    match mode {
        Off => None,
        Track => match (track.rg_track_gain, track.lufs) {
            (Some(g), _) => Some(RgInfo {
                gain_db: g,
                peak: track.rg_track_peak,
            }),
            (None, Some(lufs)) => Some(RgInfo {
                gain_db: -18.0 - lufs,
                peak: track.true_peak,
            }),
            _ => None,
        },
        Album => match (track.rg_album_gain.or(track.rg_track_gain), track.lufs) {
            (Some(g), _) => Some(RgInfo {
                gain_db: g,
                peak: track.rg_album_peak.or(track.rg_track_peak),
            }),
            (None, Some(lufs)) => Some(RgInfo {
                gain_db: -18.0 - lufs,
                peak: track.true_peak,
            }),
            _ => None,
        },
    }
}

/// Convert a dB gain to a linear multiplier.
pub fn db_to_multiplier(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Convert a linear multiplier to dB.
pub fn multiplier_to_db(m: f32) -> f32 {
    20.0 * m.max(1e-9).log10()
}

/// Compute the gain multiplier, clamping so the true peak does not exceed the
/// ceiling (default -1 dBFS).
pub fn clamped_multiplier(info: RgInfo, ceiling_db: f32) -> f32 {
    let mut mult = db_to_multiplier(info.gain_db);
    if let Some(peak) = info.peak {
        let ceiling = db_to_multiplier(ceiling_db);
        if peak > 0.0 {
            let allowed = ceiling / peak;
            if mult > allowed {
                mult = allowed;
            }
        }
    }
    mult
}

/// Loudness analysis result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness in LUFS.
    pub lufs: f32,
    /// True peak (linear).
    pub true_peak: f32,
}

/// Analyze interleaved f32 samples with EBU R128.
pub fn analyze(samples: &[f32], channels: u16, sample_rate: u32) -> Result<Loudness> {
    use ebur128::{EbuR128, Mode};
    if samples.is_empty() || channels == 0 || sample_rate == 0 {
        return Err(Error::Invalid("empty audio for loudness analysis".into()));
    }
    let mode = Mode::I | Mode::TRUE_PEAK;
    let mut meter = EbuR128::new(channels as u32, sample_rate, mode)
        .map_err(|e| Error::Other(format!("ebur128 init: {e}")))?;
    // Add in chunks to bound per-call work.
    for chunk in samples.chunks(channels as usize * 4096) {
        meter
            .add_frames_f32(chunk)
            .map_err(|e| Error::Other(format!("ebur128 add: {e}")))?;
    }
    let lufs = meter
        .loudness_global()
        .map_err(|e| Error::Other(format!("ebur128 loudness: {e}")))?;
    let mut true_peak = 0.0f64;
    for ch in 0..channels {
        let peak = meter
            .true_peak(ch as u32)
            .map_err(|e| Error::Other(format!("ebur128 true peak: {e}")))?;
        true_peak = true_peak.max(peak);
    }
    Ok(Loudness {
        lufs: lufs as f32,
        true_peak: true_peak as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_conversion_roundtrip() {
        let m = db_to_multiplier(-6.0);
        assert!((m - 0.501187).abs() < 1e-4);
        assert!((multiplier_to_db(m) + 6.0).abs() < 1e-4);
    }

    #[test]
    fn peak_clamps_gain() {
        let info = RgInfo {
            gain_db: 12.0,
            peak: Some(0.99),
        };
        let mult = clamped_multiplier(info, -1.0);
        // ceiling 0.891 / 0.99 = 0.9
        assert!(mult < 1.0);
    }

    #[test]
    fn analyze_silence() {
        let samples = vec![0.0f32; 4800 * 2];
        let l = analyze(&samples, 2, 48000).unwrap();
        assert!(l.true_peak < 0.001);
    }
}
