//! Streaming sample-rate conversion.
//!
//! A small stateful linear-phase interpolator is used for the real-time path:
//! it keeps one frame of look-ahead and runs in O(1) per output frame with no
//! allocation, which keeps the decoder thread cheap and deterministic. Higher
//! quality offline conversion can be layered on later without changing the
//! decoder interface.

/// Number of channels being resampled.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    ratio: f64,
    phase: f64,
    channels: usize,
    prev: Vec<f32>,
    curr: Vec<f32>,
    primed: bool,
}

impl LinearResampler {
    pub fn new(src_rate: u32, dst_rate: u32, channels: usize) -> Self {
        let ratio = if dst_rate == 0 {
            1.0
        } else {
            src_rate as f64 / dst_rate as f64
        };
        Self {
            ratio,
            phase: 0.0,
            channels,
            prev: vec![0.0; channels],
            curr: vec![0.0; channels],
            primed: false,
        }
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.prev.iter_mut().for_each(|s| *s = 0.0);
        self.curr.iter_mut().for_each(|s| *s = 0.0);
        self.primed = false;
    }

    /// Seed the interpolator with the first two source frames.
    pub fn prime(&mut self, first: &[f32], second: &[f32]) {
        self.prev[..self.channels].copy_from_slice(&first[..self.channels]);
        self.curr[..self.channels].copy_from_slice(&second[..self.channels]);
        self.phase = 0.0;
        self.primed = true;
    }

    pub fn is_primed(&self) -> bool {
        self.primed
    }

    /// Write the current interpolated frame.
    pub fn interpolate(&self, out: &mut [f32]) {
        let p = self.phase as f32;
        for c in 0..self.channels {
            out[c] = self.prev[c] * (1.0 - p) + self.curr[c] * p;
        }
    }

    /// Advance the phase; returns the number of source frames that must be
    /// shifted in (0 or 1 in practice).
    pub fn advance(&mut self) -> usize {
        self.phase += self.ratio;
        let mut shifts = 0;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            shifts += 1;
        }
        shifts
    }

    /// Shift `curr` into `prev`, then copy the freshly pulled frame into `curr`.
    pub fn push_frame(&mut self, next: &[f32]) {
        self.prev.copy_from_slice(&self.curr);
        self.curr[..self.channels].copy_from_slice(&next[..self.channels]);
    }

    pub fn channels(&self) -> usize {
        self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_ratio() {
        let mut r = LinearResampler::new(48000, 48000, 1);
        r.prime(&[0.0], &[1.0]);
        let mut out = [0.0f32];
        r.interpolate(&mut out);
        assert!((out[0] - 0.0).abs() < 1e-6);
        let shifts = r.advance();
        assert_eq!(shifts, 1);
        r.push_frame(&[2.0]);
        r.interpolate(&mut out);
        assert!((out[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn upsample_hits_two_points() {
        // 24k -> 48k: each output frame advances half a source frame.
        let mut r = LinearResampler::new(24000, 48000, 1);
        r.prime(&[0.0], &[1.0]);
        let mut out = [0.0f32];
        r.interpolate(&mut out);
        assert!(out[0].abs() < 1e-6);
        assert_eq!(r.advance(), 0);
        r.interpolate(&mut out);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert_eq!(r.advance(), 1);
    }
}
