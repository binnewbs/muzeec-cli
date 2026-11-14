//! Filesystem watcher that debounces change events for the daemon.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::{Error, Result};

/// Debounced filesystem watcher over the music directories.
pub struct LibraryWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<Vec<PathBuf>>,
}

impl LibraryWatcher {
    /// Create a watcher noticing audio-file creates/modifies/removes,
    /// coalescing bursts into a single batch every `debounce`.
    pub fn new(dirs: &[PathBuf], debounce: Duration) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res
                    && matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    )
                {
                    let _ = tx.send(event.paths);
                }
            },
            Config::default().with_poll_interval(debounce),
        )
        .map_err(|e| Error::Other(format!("watcher init: {e}")))?;

        for dir in dirs {
            if dir.exists() {
                watcher
                    .watch(dir, RecursiveMode::Recursive)
                    .map_err(|e| Error::Other(format!("watch {}: {e}", dir.display())))?;
            }
        }

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Block up to `timeout` for a debounced batch of changed paths.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Vec<PathBuf>> {
        let first = self.rx.recv_timeout(timeout).ok()?;
        let mut paths = first;
        // Drain any events that arrived in the meantime.
        while let Ok(more) = self.rx.try_recv() {
            paths.extend(more);
        }
        paths.sort();
        paths.dedup();
        Some(paths)
    }
}
