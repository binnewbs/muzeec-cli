//! Ordered play queue with history, loop modes, and shuffle.

use std::collections::VecDeque;

use rand::SeedableRng;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoopMode {
    #[default]
    Off,
    One,
    All,
}

impl LoopMode {
    pub fn cycle(self) -> Self {
        match self {
            LoopMode::Off => LoopMode::All,
            LoopMode::All => LoopMode::One,
            LoopMode::One => LoopMode::Off,
        }
    }

    /// MPRIS `LoopStatus` string.
    pub fn mpris(self) -> &'static str {
        match self {
            LoopMode::Off => "None",
            LoopMode::One => "Track",
            LoopMode::All => "Playlist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "none" => Some(LoopMode::Off),
            "one" | "track" => Some(LoopMode::One),
            "all" | "playlist" => Some(LoopMode::All),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: u64,
    pub track_id: i64,
}

/// Serializable queue snapshot for state persistence.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct QueueState {
    pub items: Vec<QueueItem>,
    pub current: Option<usize>,
    pub history: Vec<i64>,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
    pub next_id: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Queue {
    items: VecDeque<QueueItem>,
    current: Option<usize>,
    history: Vec<i64>,
    loop_mode: LoopMode,
    shuffle: bool,
    next_id: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ..Default::default()
        }
    }

    pub fn from_state(state: QueueState) -> Self {
        Self {
            items: state.items.into_iter().collect(),
            current: state.current,
            history: state.history,
            loop_mode: state.loop_mode,
            shuffle: state.shuffle,
            next_id: state.next_id.max(1),
        }
    }

    pub fn to_state(&self) -> QueueState {
        QueueState {
            items: self.items.iter().cloned().collect(),
            current: self.current,
            history: self.history.clone(),
            loop_mode: self.loop_mode,
            shuffle: self.shuffle,
            next_id: self.next_id,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> impl Iterator<Item = &QueueItem> {
        self.items.iter()
    }

    pub fn track_ids(&self) -> Vec<i64> {
        self.items.iter().map(|i| i.track_id).collect()
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current_track(&self) -> Option<i64> {
        self.current
            .and_then(|i| self.items.get(i))
            .map(|i| i.track_id)
    }

    pub fn loop_mode(&self) -> LoopMode {
        self.loop_mode
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.shuffle = on;
    }

    pub fn history(&self) -> &[i64] {
        &self.history
    }

    /// Append a track to the end of the queue.
    pub fn append(&mut self, track_id: i64) -> u64 {
        let item = self.make_item(track_id);
        let id = item.id;
        self.items.push_back(item);
        id
    }

    /// Insert a track immediately after the current one ("play next").
    pub fn play_next(&mut self, track_id: i64) -> u64 {
        let item = self.make_item(track_id);
        let id = item.id;
        let pos = self
            .current
            .map(|c| c + 1)
            .unwrap_or(0)
            .min(self.items.len());
        self.items.insert(pos, item);
        id
    }

    fn make_item(&mut self, track_id: i64) -> QueueItem {
        let id = self.next_id;
        self.next_id += 1;
        QueueItem { id, track_id }
    }

    /// Remove by queue-item id.
    pub fn remove(&mut self, item_id: u64) -> Result<bool> {
        let Some(pos) = self.items.iter().position(|i| i.id == item_id) else {
            return Ok(false);
        };
        self.items.remove(pos);
        if let Some(cur) = self.current {
            if pos < cur {
                self.current = Some(cur - 1);
            } else if pos == cur {
                // Removing the current item: keep index pointing at the next item.
                self.current = if self.items.is_empty() {
                    None
                } else {
                    Some(cur.min(self.items.len() - 1))
                };
            }
        }
        Ok(true)
    }

    /// Move an item from one position to another.
    pub fn move_item(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.items.len() || to >= self.items.len() {
            return Err(Error::Invalid(format!(
                "position out of range (len {})",
                self.items.len()
            )));
        }
        let item = self.items.remove(from).unwrap();
        self.items.insert(to, item);
        if let Some(cur) = self.current {
            self.current = Some(if cur == from {
                to
            } else if from < cur && to >= cur {
                cur - 1
            } else if from > cur && to <= cur {
                cur + 1
            } else {
                cur
            });
        }
        Ok(())
    }

    /// Remove duplicate track entries, keeping the first occurrence.
    pub fn dedupe(&mut self) -> usize {
        let mut seen = std::collections::HashSet::new();
        let current_id = self.current.and_then(|c| self.items.get(c)).map(|i| i.id);
        let before = self.items.len();
        self.items.retain(|i| seen.insert(i.track_id));
        if let Some(cid) = current_id {
            self.current = self.items.iter().position(|i| i.id == cid);
        }
        before - self.items.len()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
    }

    /// Select the item at `pos` and record the previous current track in history.
    pub fn jump(&mut self, pos: usize) -> Result<i64> {
        if pos >= self.items.len() {
            return Err(Error::Invalid(format!(
                "position out of range (len {})",
                self.items.len()
            )));
        }
        self.push_history();
        self.current = Some(pos);
        Ok(self.items[pos].track_id)
    }

    fn push_history(&mut self) {
        if let Some(t) = self.current_track() {
            self.history.push(t);
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
    }

    /// Advance to the next track according to loop mode.
    /// Returns `None` when the queue is exhausted (loop off at end).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<i64> {
        self.push_history();
        let len = self.items.len();
        if len == 0 {
            return None;
        }
        match self.loop_mode {
            LoopMode::One => {
                // Repeat current; if none, start at 0.
                let pos = self.current.unwrap_or(0);
                self.current = Some(pos);
                Some(self.items[pos].track_id)
            }
            LoopMode::All => {
                let pos = match self.current {
                    Some(c) => (c + 1) % len,
                    None => 0,
                };
                self.current = Some(pos);
                Some(self.items[pos].track_id)
            }
            LoopMode::Off => {
                let pos = match self.current {
                    Some(c) if c + 1 < len => c + 1,
                    None if !self.items.is_empty() => 0,
                    _ => {
                        // Undo the history push: nothing actually played.
                        self.history.pop();
                        self.current = None;
                        return None;
                    }
                };
                self.current = Some(pos);
                Some(self.items[pos].track_id)
            }
        }
    }

    /// Go back one track using the history stack.
    pub fn prev(&mut self) -> Option<i64> {
        if let Some(prev) = self.history.pop() {
            if let Some(pos) = self.items.iter().position(|i| i.track_id == prev) {
                self.current = Some(pos);
                return Some(prev);
            }
            // Track no longer in queue; reload it directly.
            return Some(prev);
        }
        // No history: step backwards within the queue.
        let pos = self.current?;
        if pos > 0 {
            self.current = Some(pos - 1);
            Some(self.items[pos - 1].track_id)
        } else if self.loop_mode != LoopMode::Off && !self.items.is_empty() {
            self.current = Some(self.items.len() - 1);
            Some(self.items[self.items.len() - 1].track_id)
        } else {
            None
        }
    }

    /// Preview the next track without mutating the queue. Used to pre-load the
    /// following decoder for gapless playback.
    pub fn peek_next(&self) -> Option<i64> {
        if self.items.is_empty() {
            return None;
        }
        let len = self.items.len();
        match self.loop_mode {
            LoopMode::One => self.current_track(),
            LoopMode::All => {
                let pos = match self.current {
                    Some(c) => (c + 1) % len,
                    None => 0,
                };
                Some(self.items[pos].track_id)
            }
            LoopMode::Off => match self.current {
                Some(c) if c + 1 < len => Some(self.items[c + 1].track_id),
                None => Some(self.items[0].track_id),
                _ => None,
            },
        }
    }

    /// Point the current index at the next occurrence of `track_id` after the
    /// current position (used when the engine auto-advances gaplessly).
    pub fn select_track_id(&mut self, track_id: i64) -> bool {
        let start = self.current.map(|c| c + 1).unwrap_or(0);
        let found = self
            .items
            .iter()
            .enumerate()
            .skip(start)
            .chain(self.items.iter().enumerate().take(start))
            .find(|(_, item)| item.track_id == track_id)
            .map(|(i, _)| i);
        if let Some(i) = found {
            if let Some(prev) = self.current_track()
                && prev != track_id
            {
                self.history.push(prev);
                if self.history.len() > 200 {
                    self.history.remove(0);
                }
            }
            self.current = Some(i);
            true
        } else {
            false
        }
    }

    /// Items following the current one (the "up next" view).
    pub fn up_next(&self, limit: usize) -> Vec<&QueueItem> {
        let start = self.current.map(|c| c + 1).unwrap_or(0);
        self.items.iter().skip(start).take(limit).collect()
    }

    /// Shuffle the upcoming items with a deterministic seed, keeping the
    /// current track in place at the front.
    pub fn shuffle_with_seed(&mut self, seed: u64) {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut rest: Vec<QueueItem> = self.items.iter().skip(1).cloned().collect();
        rest.shuffle(&mut rng);
        let first = self.items.front().cloned();
        self.items.clear();
        if let Some(f) = first {
            self.items.push_back(f);
        }
        self.items.extend(rest);
        if self.current.is_some() {
            self.current = Some(0);
        }
    }
}

impl Queue {
    /// Replace the whole queue, preserving or resetting the current track.
    pub fn replace(&mut self, track_ids: Vec<i64>, play_first: bool) {
        self.items = track_ids
            .into_iter()
            .map(|tid| {
                let id = self.next_id;
                self.next_id += 1;
                QueueItem { id, track_id: tid }
            })
            .collect();
        self.current = if play_first && !self.items.is_empty() {
            Some(0)
        } else {
            None
        };
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> Queue {
        let mut q = Queue::new();
        for t in [10, 20, 30] {
            q.append(t);
        }
        q
    }

    #[test]
    fn append_and_next() {
        let mut q = q();
        assert_eq!(q.len(), 3);
        assert_eq!(q.next(), Some(10));
        assert_eq!(q.next(), Some(20));
        assert_eq!(q.next(), Some(30));
        assert_eq!(q.next(), None);
    }

    #[test]
    fn loop_all_wraps() {
        let mut q = q();
        q.set_loop_mode(LoopMode::All);
        assert_eq!(q.next(), Some(10));
        assert_eq!(q.next(), Some(20));
        assert_eq!(q.next(), Some(30));
        assert_eq!(q.next(), Some(10));
    }

    #[test]
    fn loop_one_repeats() {
        let mut q = q();
        q.jump(1).unwrap();
        q.set_loop_mode(LoopMode::One);
        assert_eq!(q.next(), Some(20));
        assert_eq!(q.next(), Some(20));
    }

    #[test]
    fn prev_uses_history() {
        let mut q = q();
        q.next();
        q.next();
        assert_eq!(q.current_track(), Some(20));
        assert_eq!(q.prev(), Some(10));
    }

    #[test]
    fn play_next_inserts_after_current() {
        let mut q = q();
        q.next();
        q.play_next(99);
        assert_eq!(q.track_ids(), vec![10, 99, 20, 30]);
    }

    #[test]
    fn dedupe_keeps_first() {
        let mut q = Queue::new();
        for t in [1, 2, 1, 3, 2] {
            q.append(t);
        }
        assert_eq!(q.dedupe(), 2);
        assert_eq!(q.track_ids(), vec![1, 2, 3]);
    }

    #[test]
    fn move_item_updates_current() {
        let mut q = q();
        q.jump(0).unwrap();
        q.move_item(0, 2).unwrap();
        assert_eq!(q.track_ids(), vec![20, 30, 10]);
        assert_eq!(q.current_track(), Some(10));
    }

    #[test]
    fn state_roundtrip() {
        let mut q = q();
        q.next();
        q.set_loop_mode(LoopMode::All);
        let state = q.to_state();
        let q2 = Queue::from_state(state);
        assert_eq!(q2.track_ids(), q.track_ids());
        assert_eq!(q2.current_track(), q.current_track());
        assert_eq!(q2.loop_mode(), LoopMode::All);
    }
}
