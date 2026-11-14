//! Similarity-based "mix" (auto-DJ) scoring.
//!
//! Score is a weighted sum of feature similarities. Candidate selection uses
//! weighted sampling from the top window so playlists are not deterministic.

use rand::Rng;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::db::Track;

/// Configurable scoring weights (sum ~= 1.0).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MixWeights {
    pub artist: f32,
    pub albumartist: f32,
    pub genre: f32,
    pub bpm: f32,
    pub energy: f32,
    pub rating: f32,
    pub recency: f32,
}

impl Default for MixWeights {
    fn default() -> Self {
        Self {
            artist: 0.20,
            albumartist: 0.10,
            genre: 0.20,
            bpm: 0.15,
            energy: 0.15,
            rating: 0.10,
            recency: 0.10,
        }
    }
}

/// Seed features derived from the currently playing/reference track.
#[derive(Debug, Clone, Default)]
pub struct Seed {
    pub track_id: Option<i64>,
    pub artist: String,
    pub albumartist: String,
    pub genre: String,
    pub year: Option<u32>,
    pub bpm: Option<f32>,
    pub lufs: Option<f32>,
}

impl Seed {
    pub fn from_track(t: &Track) -> Self {
        Self {
            track_id: Some(t.id),
            artist: t.artist.clone(),
            albumartist: t.albumartist.clone(),
            genre: t.genre.clone(),
            year: t.year,
            bpm: t.bpm,
            lufs: t.lufs,
        }
    }
}

/// A candidate with social signals.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub track: Track,
    pub rating: u8,
    pub play_count: u32,
}

/// Score a candidate against the seed. Higher is more similar.
pub fn score(seed: &Seed, cand: &Track, rating: u8, play_count: u32, w: &MixWeights) -> f32 {
    let mut s = 0.0;
    if !seed.artist.is_empty() && seed.artist.eq_ignore_ascii_case(&cand.artist) {
        s += w.artist;
    }
    if !seed.albumartist.is_empty() && seed.albumartist.eq_ignore_ascii_case(&cand.albumartist) {
        s += w.albumartist;
    }
    s += w.genre * genre_similarity(&seed.genre, &cand.genre);
    s += w.bpm * bpm_similarity(seed.bpm, cand.bpm);
    s += w.energy * energy_similarity(seed.lufs, cand.lufs);
    s += w.rating * (rating as f32 / 5.0);
    // Recency: lightly-played tracks get a small boost.
    s += w.recency * (1.0 / (1.0 + play_count as f32));
    s
}

fn genre_similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a.eq_ignore_ascii_case(b) {
        return 1.0;
    }
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if a.split_whitespace().any(|w| b.contains(w)) || b.split_whitespace().any(|w| a.contains(w)) {
        0.5
    } else {
        0.0
    }
}

fn bpm_similarity(a: Option<f32>, b: Option<f32>) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) if a > 0.0 && b > 0.0 => {
            let diff = (a - b).abs();
            (1.0 - diff / 40.0).clamp(0.0, 1.0)
        }
        _ => 0.0,
    }
}

fn energy_similarity(a: Option<f32>, b: Option<f32>) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) => (1.0 - (a - b).abs() / 20.0).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Build a mix of `n` track ids from candidates, avoiding repeats within the
/// recent window and the seed track itself.
pub fn build_mix<R: Rng>(
    seed: &Seed,
    candidates: &[Candidate],
    n: usize,
    avoid: &[i64],
    rng: &mut R,
) -> Vec<i64> {
    let w = MixWeights::default();
    let mut scored: Vec<(f32, i64)> = candidates
        .iter()
        .filter(|c| Some(c.track.id) != seed.track_id)
        .filter(|c| !avoid.contains(&c.track.id))
        .map(|c| {
            (
                score(seed, &c.track, c.rating, c.play_count, &w),
                c.track.id,
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    let top: Vec<(f32, i64)> = scored.into_iter().take(50).collect();
    let mut chosen = Vec::with_capacity(n);
    let mut pool = top;
    while chosen.len() < n && !pool.is_empty() {
        // Weighted sampling with low temperature (score^3 emphasises top).
        let weights: Vec<f32> = pool.iter().map(|(s, _)| (s.max(0.01)).powf(3.0)).collect();
        let idx = weighted_index(&weights, rng);
        let (_, id) = pool.remove(idx);
        if !avoid.contains(&id) && !chosen.contains(&id) {
            chosen.push(id);
        }
    }
    // If the candidate pool was exhausted, shuffle the leftovers deterministically.
    if chosen.len() < n {
        let mut rest: Vec<i64> = candidates
            .iter()
            .map(|c| c.track.id)
            .filter(|id| Some(*id) != seed.track_id && !chosen.contains(id))
            .collect();
        rest.shuffle(rng);
        chosen.extend(rest.into_iter().take(n - chosen.len()));
    }
    chosen
}

fn weighted_index<R: Rng>(weights: &[f32], rng: &mut R) -> usize {
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return 0;
    }
    let mut point = rng.random::<f32>() * total;
    for (i, w) in weights.iter().enumerate() {
        point -= w;
        if point <= 0.0 {
            return i;
        }
    }
    weights.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn tr(id: i64, artist: &str, genre: &str) -> Track {
        Track {
            id,
            path: String::new(),
            filename: String::new(),
            title: String::new(),
            artist: artist.into(),
            albumartist: artist.into(),
            album: String::new(),
            genre: genre.into(),
            composer: None,
            comment: None,
            track_no: None,
            disc_no: None,
            year: None,
            duration_ms: 0,
            sample_rate: 0,
            channels: 0,
            bitrate: None,
            codec: String::new(),
            bpm: None,
            lufs: None,
            true_peak: None,
            rg_track_gain: None,
            rg_track_peak: None,
            rg_album_gain: None,
            rg_album_peak: None,
            size: 0,
            mtime: 0,
            art_hash: None,
            added_at: 0,
            last_seen_at: 0,
            error: None,
        }
    }

    #[test]
    fn same_artist_scores_higher() {
        let seed = Seed {
            artist: "Miles".into(),
            genre: "Jazz".into(),
            ..Default::default()
        };
        let w = MixWeights::default();
        let a = score(&seed, &tr(1, "Miles", "Jazz"), 0, 0, &w);
        let b = score(&seed, &tr(2, "Other", "Rock"), 0, 0, &w);
        assert!(a > b);
    }

    #[test]
    fn mix_avoids_repeats() {
        let seed = Seed {
            artist: "A".into(),
            genre: "Rock".into(),
            ..Default::default()
        };
        let cands: Vec<Candidate> = (0..10)
            .map(|i| Candidate {
                track: tr(i, "A", "Rock"),
                rating: 3,
                play_count: 0,
            })
            .collect();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let mix = build_mix(&seed, &cands, 5, &[], &mut rng);
        assert_eq!(mix.len(), 5);
        let mut uniq = mix.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 5);
    }
}
