//! Tag extraction with `lofty`, producing [`TrackUpsert`] rows.

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::prelude::{Accessor, AudioFile, ItemKey};
use lofty::probe::Probe;

use crate::db::queries::TrackUpsert;
use crate::error::{Error, Result};

/// Audio extensions we attempt to scan.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "mp4", "aac", "alac", "wav", "aiff", "aif", "ape",
    "wv", "mka", "mkv",
];

pub fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Filename-based fallback: split `Artist - Album - 01 - Title`.
fn fallback_from_filename(
    path: &Path,
) -> (Option<String>, Option<String>, Option<String>, Option<u32>) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parts: Vec<&str> = stem.split(" - ").collect();
    if parts.len() >= 4 {
        let track_no = parts[2].trim().parse::<u32>().ok();
        (
            Some(parts[0].trim().to_string()),
            Some(parts[1].trim().to_string()),
            Some(parts[3].trim().to_string()),
            track_no,
        )
    } else if parts.len() == 2 {
        (
            Some(parts[0].trim().to_string()),
            None,
            Some(parts[1].trim().to_string()),
            None,
        )
    } else {
        (None, None, Some(stem.to_string()), None)
    }
}

/// Read tags and filesystem metadata for a track. Returns a `TrackUpsert` with
/// `error` set for corrupt files rather than returning `Err`.
pub fn read_tags(path: &Path) -> Result<TrackUpsert> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_str = abs.to_string_lossy().to_string();
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_string();

    let mut upsert = TrackUpsert {
        path: path_str,
        filename: filename.clone(),
        size: meta.len(),
        mtime,
        ..Default::default()
    };

    let parsed = Probe::open(path)
        .map_err(|e| Error::Metadata(e.to_string()))
        .and_then(|p| {
            p.options(ParseOptions::new())
                .read()
                .map_err(|e| Error::Metadata(e.to_string()))
        });

    let file = match parsed {
        Ok(f) => f,
        Err(e) => {
            let (artist, album, title, track_no) = fallback_from_filename(path);
            upsert.title = title.unwrap_or_else(|| filename.clone());
            upsert.artist = artist.unwrap_or_default();
            upsert.albumartist = upsert.artist.clone();
            upsert.album = album.unwrap_or_default();
            upsert.track_no = track_no;
            upsert.codec = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_uppercase();
            upsert.error = Some(e.to_string());
            return Ok(upsert);
        }
    };

    upsert.codec = format!("{:?}", file.file_type()).to_lowercase();
    let props = file.properties();
    upsert.duration_ms = props.duration().as_millis() as u64;
    upsert.sample_rate = props.sample_rate().unwrap_or(0);
    upsert.channels = props.channels().unwrap_or(0) as u16;
    upsert.bitrate = props.audio_bitrate().or_else(|| props.overall_bitrate());

    let tag = file.primary_tag().or_else(|| file.first_tag());
    if let Some(tag) = tag {
        let get =
            |s: Option<std::borrow::Cow<'_, str>>| s.map(|c| c.into_owned()).unwrap_or_default();
        upsert.title = get(tag.title());
        upsert.artist = get(tag.artist());
        upsert.albumartist = tag
            .get_string(ItemKey::AlbumArtist)
            .unwrap_or("")
            .to_string();
        upsert.album = get(tag.album());
        upsert.genre = get(tag.genre());
        upsert.composer = tag.get_string(ItemKey::Composer).map(str::to_owned);
        upsert.comment = tag.get_string(ItemKey::Comment).map(str::to_owned);
        upsert.track_no = tag.track();
        upsert.disc_no = tag.disk();
        upsert.year = tag
            .get_string(ItemKey::Year)
            .and_then(|s| s.get(..4).unwrap_or(s).parse().ok());
        upsert.bpm = tag.get_string(ItemKey::Bpm).and_then(|s| s.parse().ok());
        upsert.rg_track_gain = tag
            .get_string(ItemKey::ReplayGainTrackGain)
            .and_then(parse_gain_db);
        upsert.rg_track_peak = tag
            .get_string(ItemKey::ReplayGainTrackPeak)
            .and_then(|s| s.parse().ok());
        upsert.rg_album_gain = tag
            .get_string(ItemKey::ReplayGainAlbumGain)
            .and_then(parse_gain_db);
        upsert.rg_album_peak = tag
            .get_string(ItemKey::ReplayGainAlbumPeak)
            .and_then(|s| s.parse().ok());
    }

    if upsert.albumartist.is_empty() {
        upsert.albumartist = upsert.artist.clone();
    }
    if upsert.title.is_empty() {
        upsert.title = filename
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or_else(|| filename.clone());
    }
    if upsert.artist.is_empty() {
        let (artist, album, title, track_no) = fallback_from_filename(path);
        if let Some(a) = artist {
            upsert.artist = a;
            if upsert.albumartist.is_empty() {
                upsert.albumartist = upsert.artist.clone();
            }
        }
        if upsert.album.is_empty()
            && let Some(al) = album
        {
            upsert.album = al;
        }
        if upsert.track_no.is_none() {
            upsert.track_no = track_no;
        }
        if upsert.title == filename
            && let Some(t) = title
        {
            upsert.title = t;
        }
    }

    Ok(upsert)
}

/// Parse a ReplayGain gain string like `-7.23 dB` into a float.
fn parse_gain_db(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gain_strings() {
        assert_eq!(parse_gain_db("-7.23 dB"), Some(-7.23));
        assert_eq!(parse_gain_db("3.0"), Some(3.0));
        assert_eq!(parse_gain_db("-7.23 dB "), Some(-7.23));
    }

    #[test]
    fn identifies_audio() {
        assert!(is_audio_path(Path::new("a.flac")));
        assert!(is_audio_path(Path::new("a.MP3")));
        assert!(!is_audio_path(Path::new("a.txt")));
    }
}
