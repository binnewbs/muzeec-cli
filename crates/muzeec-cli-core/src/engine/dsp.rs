//! DSP primitives: soft limiter and a 10-band peaking EQ.

/// Soft limiter with a hard ceiling and smooth knee. Cheap enough for the
/// real-time callback: no allocation, no branches beyond the knee check.
#[derive(Debug, Clone)]
pub struct SoftLimiter {
    ceiling: f32,
    knee: f32,
}

impl Default for SoftLimiter {
    fn default() -> Self {
        Self::new(-1.0)
    }
}

impl SoftLimiter {
    /// `ceiling_db` is the maximum output level (e.g. `-1.0`).
    pub fn new(ceiling_db: f32) -> Self {
        Self {
            ceiling: 10f32.powf(ceiling_db / 20.0),
            knee: 0.9,
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        let ceiling = self.ceiling;
        let knee = ceiling * self.knee;
        for s in samples.iter_mut() {
            let x = *s;
            let a = x.abs();
            if a <= knee {
                continue;
            }
            // Smoothly compress the region above the knee towards the ceiling.
            let over = (a - knee) / (ceiling - knee).max(1e-6);
            let shaped = knee + (ceiling - knee) * (over / (1.0 + over));
            *s = shaped.copysign(x);
        }
    }
}

/// RBJ biquad coefficients.
#[derive(Debug, Clone, Copy)]
pub struct BiquadCoeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoeffs {
    /// Peaking EQ filter.
    pub fn peaking(sample_rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.001));
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

/// Per-channel biquad state (direct form I).
#[derive(Debug, Clone, Copy, Default)]
struct BiquadState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BiquadState {
    fn process(&mut self, c: &BiquadCoeffs, x: f64) -> f64 {
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Fixed 10-band EQ (31 Hz … 16 kHz), applied per channel.
pub struct EqChain {
    bands: Vec<(f64, f64, f64)>, // (freq, q, gain_db)
    coeffs: Vec<BiquadCoeffs>,
    state: Vec<Vec<BiquadState>>, // [channel][band]
    preamp: f32,
    enabled: bool,
    channels: usize,
    sample_rate: u32,
}

/// Standard 10-band center frequencies.
pub const EQ_FREQS: [f64; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

impl EqChain {
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let bands: Vec<(f64, f64, f64)> = EQ_FREQS.iter().map(|f| (*f, 1.41, 0.0)).collect();
        let mut eq = Self {
            bands,
            coeffs: Vec::new(),
            state: Vec::new(),
            preamp: 0.0,
            enabled: false,
            channels: channels.max(1),
            sample_rate,
        };
        eq.state = vec![vec![BiquadState::default(); eq.bands.len()]; eq.channels];
        eq.recompute();
        eq
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_preamp_db(&mut self, db: f32) {
        self.preamp = db;
    }

    pub fn set_band_gain(&mut self, band: usize, gain_db: f64) {
        if band < self.bands.len() {
            self.bands[band].2 = gain_db;
            self.recompute();
        }
    }

    pub fn set_gains(&mut self, gains: &[f32]) {
        for (i, g) in gains.iter().take(self.bands.len()).enumerate() {
            self.bands[i].2 = *g as f64;
        }
        self.recompute();
    }

    fn recompute(&mut self) {
        self.coeffs = self
            .bands
            .iter()
            .map(|(f, q, g)| BiquadCoeffs::peaking(self.sample_rate as f64, *f, *q, *g))
            .collect();
        self.state = vec![vec![BiquadState::default(); self.bands.len()]; self.channels];
    }

    /// Process interleaved samples in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.enabled {
            return;
        }
        let pre = 10f32.powf(self.preamp / 20.0);
        for frame in samples.chunks_mut(self.channels) {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample as f64 * pre as f64;
                for (band, coeffs) in self.coeffs.iter().enumerate() {
                    x = self.state[ch][band].process(coeffs, x);
                }
                *sample = x as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_respects_ceiling() {
        let mut limiter = SoftLimiter::new(-1.0);
        let mut samples = vec![2.0, -2.0, 0.5, -0.5, 10.0];
        limiter.process(&mut samples);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        for s in samples {
            assert!(s.abs() <= ceiling + 1e-6, "{s} exceeds {ceiling}");
        }
    }

    #[test]
    fn eq_flat_is_near_identity() {
        let mut eq = EqChain::new(2, 48000);
        eq.set_enabled(true);
        let mut samples = vec![0.25, 0.25, -0.25, -0.25];
        let original = samples.clone();
        eq.process(&mut samples);
        for (a, b) in original.iter().zip(&samples) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn eq_boost_changes_signal() {
        let mut eq = EqChain::new(1, 48000);
        eq.set_enabled(true);
        eq.set_band_gain(5, 6.0);
        // 1 kHz tone at 48k.
        let mut samples: Vec<f32> = (0..4800)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.1)
            .collect();
        let before = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        eq.process(&mut samples);
        let after = samples[1000..]
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        assert!(
            after > before,
            "boost did not increase level: {before} -> {after}"
        );
    }
}
