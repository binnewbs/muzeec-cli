//! Incremental library scanner using `rayon` for tag parsing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use walkdir::WalkDir;

use super::art::ArtCache;
use super::tags::{is_audio_path, read_tags};
use crate::db::{Database, now_unix};
use crate::error::Result;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub dirs: Vec<PathBuf>,
    pub follow_symlinks: bool,
    pub workers: usize,
}

impl ScanOptions {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            follow_symlinks: false,
            workers: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Started {
        total: usize,
    },
    Progress {
        scanned: usize,
        total: usize,
        path: String,
    },
    Finished(ScanReport),
}

#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub scanned: usize,
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
    pub errors: usize,
    pub duration_ms: u128,
}

/// A scanner bound to a database and artwork cache.
pub struct Scanner<'a> {
    db: &'a Database,
    art: &'a ArtCache,
}

impl<'a> Scanner<'a> {
    pub fn new(db: &'a Database, art: &'a ArtCache) -> Self {
        Self { db, art }
    }

    /// Walk the configured dirs and incrementally update the library.
    pub fn scan(
        &self,
        opts: &ScanOptions,
        progress: &(dyn Fn(ScanEvent) + Sync),
    ) -> Result<ScanReport> {
        let start = Instant::now();
        let mut report = ScanReport::default();
        let scan_start = now_unix();

        // 1. Collect candidate files.
        let mut files: Vec<(PathBuf, u64, i64, bool)> = Vec::new();
        for root in &opts.dirs {
            let walker = WalkDir::new(root)
                .follow_links(opts.follow_symlinks)
                .into_iter()
                .filter_entry(|e| {
                    e.depth() == 0
                        || !e
                            .file_name()
                            .to_str()
                            .map(|n| n.starts_with('.'))
                            .unwrap_or(false)
                });
            for entry in walker.flatten() {
                if !entry.file_type().is_file() || !is_audio_path(entry.path()) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    report.errors += 1;
                    continue;
                };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                files.push((entry.into_path(), meta.len(), mtime, true));
            }
        }

        progress(ScanEvent::Started { total: files.len() });

        // 2. Decide what actually needs parsing (incremental).
        let mut to_scan: Vec<(PathBuf, bool)> = Vec::new();
        for (path, size, mtime, _) in files {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            let key_str = key.to_string_lossy().to_string();
            match self.db.track_by_path(&key_str)? {
                Some(t) if t.size == size && t.mtime == mtime => {
                    self.db.touch_track(t.id, scan_start)?;
                    report.skipped += 1;
                }
                Some(_) => to_scan.push((path, false)),
                None => to_scan.push((path, true)),
            }
        }

        let total = to_scan.len();
        let scanned = AtomicUsize::new(0);

        // 3. Parse tags in parallel, resolve art.
        let parsed: Vec<_> = to_scan
            .par_iter()
            .map(|(path, is_new)| {
                let mut upsert = read_tags(path).unwrap_or_else(|e| {
                    let mut u = crate::db::queries::TrackUpsert {
                        path: path.to_string_lossy().to_string(),
                        filename: path
                            .file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or("")
                            .to_string(),
                        ..Default::default()
                    };
                    u.error = Some(e.to_string());
                    u
                });
                if upsert.error.is_none() {
                    upsert.art_hash = self.art.resolve(path, None);
                }
                let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                progress(ScanEvent::Progress {
                    scanned: done,
                    total,
                    path: path.to_string_lossy().to_string(),
                });
                (*is_new, upsert)
            })
            .collect();

        // 4. Persist sequentially in batches.
        for chunk in parsed.chunks(500) {
            for (is_new, upsert) in chunk {
                match self.db.upsert_track(upsert) {
                    Ok(_) => {
                        if upsert.error.is_some() {
                            report.errors += 1;
                        }
                        if *is_new {
                            report.added += 1;
                        } else {
                            report.updated += 1;
                        }
                        report.scanned += 1;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, path = %upsert.path, "failed to persist track");
                        report.errors += 1;
                    }
                }
            }
        }

        // 5. Prune tracks that disappeared under scanned roots.
        for root in &opts.dirs {
            let prefix = root.to_string_lossy();
            report.removed += self.db.prune_stale(&prefix, scan_start)?;
        }

        self.db.refresh_albums()?;
        let _ = self.art.cleanup_lru();

        report.duration_ms = start.elapsed().as_millis();
        progress(ScanEvent::Finished(report.clone()));
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_temp_tree() {
        let root = std::env::temp_dir().join(format!("muzeec-scan-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        // Write junk that is not valid audio: it should be recorded with error.
        std::fs::write(root.join("song.mp3"), b"not audio").unwrap();
        std::fs::write(root.join("readme.txt"), b"ignore me").unwrap();
        // Sidecar cover.
        std::fs::write(root.join("cover.jpg"), b"\xff\xd8\xff\xd9").unwrap();

        let db = Database::open_memory().unwrap();
        let art = ArtCache::new(root.join(".art"), 1024).unwrap();
        let scanner = Scanner::new(&db, &art);
        let opts = ScanOptions::new(vec![root.clone()]);
        let report = scanner.scan(&opts, &|_| {}).unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.added, 1);

        // Second scan should skip the unchanged file.
        let report2 = scanner.scan(&opts, &|_| {}).unwrap();
        assert_eq!(report2.skipped, 1);
        assert_eq!(report2.scanned, 0);

        std::fs::remove_dir_all(&root).ok();
    }
}
