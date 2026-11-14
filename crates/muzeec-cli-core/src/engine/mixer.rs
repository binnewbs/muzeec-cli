//! Mixing math: volume curve and equal-power crossfade.

/// Perceptual volume curve: maps 0..=100 to 0.0..=1.0 with a cubic taper.
pub fn volume_curve(volume: u8) -> f32 {
    let v = (volume.min(100) as f32) / 100.0;
    v * v * v
}

/// Equal-power crossfade gains at position `t` in 0..=1.
/// Returns `(gain_a, gain_b)` such that `a^2 + b^2 == 1`.
pub fn equal_power(t: f32) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let angle = t * std::f32::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// Mix two equally-sized interleaved buffers with equal-power gains.
/// `out.len()` must be the smaller of the two.
pub fn mix_crossfade(a: &[f32], b: &[f32], out: &mut [f32], t: f32) {
    let (ga, gb) = equal_power(t);
    let n = out.len().min(a.len()).min(b.len());
    for i in 0..n {
        out[i] = a[i] * ga + b[i] * gb;
    }
}

/// Apply a linear gain to an interleaved buffer.
pub fn apply_gain(buf: &mut [f32], gain: f32) {
    for s in buf.iter_mut() {
        *s *= gain;
    }
}

/// Short fade envelope for start/stop/seek splices (5 ms by default).
pub struct Fade {
    remaining: usize,
    total: usize,
    rising: bool,
}

impl Fade {
    pub fn new(frames: usize, rising: bool) -> Self {
        Self {
            remaining: frames,
            total: frames.max(1),
            rising,
        }
    }

    pub fn is_done(&self) -> bool {
        self.remaining == 0
    }

    /// Next gain multiplier in 0..=1.
    pub fn next_gain(&mut self) -> f32 {
        if self.remaining == 0 {
            return if self.rising { 1.0 } else { 0.0 };
        }
        let progress = 1.0 - (self.remaining as f32 / self.total as f32);
        self.remaining -= 1;
        if self.rising {
            progress
        } else {
            1.0 - progress
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_monotonic() {
        assert_eq!(volume_curve(0), 0.0);
        assert_eq!(volume_curve(100), 1.0);
        assert!(volume_curve(50) < volume_curve(75));
    }

    #[test]
    fn equal_power_sums_to_one() {
        for i in 0..=10 {
            let (a, b) = equal_power(i as f32 / 10.0);
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn crossfade_endpoints() {
        let a = [1.0f32; 4];
        let b = [0.5f32; 4];
        let mut out = [0.0f32; 4];
        mix_crossfade(&a, &b, &mut out, 0.0);
        assert!((out[0] - 1.0).abs() < 1e-5);
        mix_crossfade(&a, &b, &mut out, 1.0);
        assert!((out[0] - 0.5).abs() < 1e-5);
    }
}
