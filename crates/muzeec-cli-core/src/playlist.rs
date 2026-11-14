//! M3U/M3U8 import and export with relative-path resolution.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const M3U_EXTS: &[&str] = &["m3u", "m3u8"];

/// A single resolved playlist entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M3uEntry {
    pub path: PathBuf,
    pub title: Option<String>,
    pub duration_secs: Option<i64>,
}

/// Result of parsing/importing an M3U file.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub entries: Vec<M3uEntry>,
    pub missing: Vec<PathBuf>,
    pub duplicates: usize,
    pub base_dir: PathBuf,
}

impl ImportReport {
    pub fn paths(&self) -> Vec<&Path> {
        self.entries.iter().map(|e| e.path.as_path()).collect()
    }
}

/// Parse M3U/M3U8 text. Relative paths resolve against `base_dir`; entries that
/// do not exist are reported in `missing` rather than failing the import.
pub fn parse_m3u(text: &str, base_dir: &Path) -> ImportReport {
    let mut report = ImportReport {
        base_dir: base_dir.to_path_buf(),
        ..Default::default()
    };
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut pending_title: Option<String> = None;
    let mut pending_duration: Option<i64> = None;

    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            // #EXTINF:123,Title
            let (dur, title) = info.split_once(',').unwrap_or((info, ""));
            pending_duration = dur.trim().parse().ok();
            let title = title.trim();
            pending_title = if title.is_empty() {
                None
            } else {
                Some(title.to_string())
            };
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let path = resolve_entry_path(line, base_dir);
        let title = pending_title.take();
        let duration = pending_duration.take();
        if !seen.insert(path.clone()) {
            report.duplicates += 1;
            continue;
        }
        if !path.exists() {
            report.missing.push(path.clone());
        }
        report.entries.push(M3uEntry {
            path,
            title,
            duration_secs: duration,
        });
    }
    report
}

fn resolve_entry_path(raw: &str, base_dir: &Path) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { base_dir.join(p) }
}

/// Read and parse an M3U file from disk.
pub fn import_file(path: &Path) -> Result<ImportReport> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !M3U_EXTS.contains(&ext.as_str()) {
        return Err(Error::Invalid(format!(
            "not an M3U playlist: {}",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path)?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    Ok(parse_m3u(&text, base))
}

/// An export track: absolute path plus optional metadata.
#[derive(Debug, Clone)]
pub struct ExportTrack {
    pub path: PathBuf,
    pub title: String,
    pub duration_secs: i64,
}

/// Render an M3U8 playlist. Paths are written relative to `relative_to` when
/// provided and feasible, otherwise absolute.
pub fn export_m3u(tracks: &[ExportTrack], relative_to: Option<&Path>) -> String {
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let path = match relative_to {
            Some(base) => pathdiff(t.path.as_path(), base),
            None => t.path.clone(),
        };
        out.push_str(&format!(
            "#EXTINF:{},{}\n{}\n",
            t.duration_secs,
            t.title,
            path.display()
        ));
    }
    out
}

/// Best-effort relative path; falls back to the absolute path.
fn pathdiff(target: &Path, base: &Path) -> PathBuf {
    let (Ok(t), Ok(b)) = (target.canonicalize(), base.canonicalize()) else {
        return target.to_path_buf();
    };
    let t_parts: Vec<_> = t.components().collect();
    let b_parts: Vec<_> = b.components().collect();
    let common = t_parts
        .iter()
        .zip(b_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return target.to_path_buf();
    }
    let mut rel = PathBuf::new();
    for _ in common..b_parts.len() {
        rel.push("..");
    }
    for comp in &t_parts[common..] {
        rel.push(comp.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        target.to_path_buf()
    } else {
        rel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extinf_and_paths() {
        let dir = std::env::temp_dir().join("muzeec-m3u-test");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.flac");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        let text = "#EXTM3U\n#EXTINF:123,First\n./a.mp3\n#EXTINF:45,Second\nb.flac\n";
        let r = parse_m3u(text, &dir);
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.entries[0].title.as_deref(), Some("First"));
        assert_eq!(r.entries[0].duration_secs, Some(123));
        assert_eq!(r.entries[1].path, b);
        assert!(r.missing.is_empty());
    }

    #[test]
    fn reports_missing_and_dupes() {
        let dir = std::env::temp_dir().join("muzeec-m3u-test2");
        std::fs::create_dir_all(&dir).unwrap();
        let text = "nope.mp3\nnope.mp3\n";
        let r = parse_m3u(text, &dir);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.missing.len(), 1);
        assert_eq!(r.duplicates, 1);
    }

    #[test]
    fn export_roundtrip() {
        let tracks = vec![ExportTrack {
            path: PathBuf::from("/music/song.flac"),
            title: "Song".into(),
            duration_secs: 200,
        }];
        let text = export_m3u(&tracks, None);
        assert!(text.contains("#EXTM3U"));
        assert!(text.contains("#EXTINF:200,Song"));
        assert!(text.contains("/music/song.flac"));
    }
}
