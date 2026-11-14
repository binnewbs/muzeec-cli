//! SQLite library database, migrations, and queries.

pub mod queries;
pub mod schema;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub use queries::*;

/// A track row as stored in the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub title: String,
    pub artist: String,
    pub albumartist: String,
    pub album: String,
    pub genre: String,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: Option<u32>,
    pub codec: String,
    pub bpm: Option<f32>,
    pub lufs: Option<f32>,
    pub true_peak: Option<f32>,
    pub rg_track_gain: Option<f32>,
    pub rg_track_peak: Option<f32>,
    pub rg_album_gain: Option<f32>,
    pub rg_album_peak: Option<f32>,
    pub size: u64,
    pub mtime: i64,
    pub art_hash: Option<String>,
    pub added_at: i64,
    pub last_seen_at: i64,
    pub error: Option<String>,
}

impl Track {
    pub fn is_lossless(&self) -> bool {
        matches!(
            self.codec.to_ascii_lowercase().as_str(),
            "flac" | "alac" | "wav" | "aiff" | "ape" | "wavpack"
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumSummary {
    pub name: String,
    pub albumartist: String,
    pub year: Option<u32>,
    pub art_hash: Option<String>,
    pub track_count: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub track_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
    pub genres: u64,
    pub playlists: u64,
    pub total_duration_ms: u64,
}

/// Timestamp helper used across the DB layer.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Handle to an open library database.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (creating if needed) the database at `path`, applying migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        Self::init(conn)
    }

    /// Open an in-memory database (tests).
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let target = schema::MIGRATIONS.len() as i64;
        if current >= target {
            return Ok(());
        }
        for (idx, sql) in schema::MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (idx + 1) as i64)?;
            tx.commit()?;
            tracing::debug!(version = idx + 1, "applied migration");
        }
        Ok(())
    }

    /// Verify the FTS5 module is available.
    pub fn fts5_available(&self) -> bool {
        self.conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='tracks_fts'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// Drop and rebuild the derived albums table from tracks.
    pub fn refresh_albums(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(
            "DELETE FROM albums;
             INSERT INTO albums(name, albumartist, year, art_hash, track_count, duration_ms)
             SELECT album, albumartist, MAX(year), MAX(art_hash), COUNT(*), SUM(duration_ms)
             FROM tracks WHERE album != '' GROUP BY album, albumartist;",
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn stats(&self) -> Result<LibraryStats> {
        let (tracks, duration): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms),0) FROM tracks WHERE error IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let albums: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
        let artists: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT artist) FROM tracks WHERE artist != ''",
            [],
            |r| r.get(0),
        )?;
        let genres: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT genre) FROM tracks WHERE genre != ''",
            [],
            |r| r.get(0),
        )?;
        let playlists: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM playlists", [], |r| r.get(0))?;
        Ok(LibraryStats {
            tracks: tracks.max(0) as u64,
            albums: albums.max(0) as u64,
            artists: artists.max(0) as u64,
            genres: genres.max(0) as u64,
            playlists: playlists.max(0) as u64,
            total_duration_ms: duration.max(0) as u64,
        })
    }

    /// Look up a track by exact path.
    pub fn track_by_path(&self, path: &str) -> Result<Option<Track>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {} FROM tracks WHERE path = ?1",
            queries::TRACK_COLUMNS
        ))?;
        let mut rows = stmt.query_map(params![path], queries::row_to_track)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn track_by_id(&self, id: i64) -> Result<Option<Track>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {} FROM tracks WHERE id = ?1",
            queries::TRACK_COLUMNS
        ))?;
        let mut rows = stmt.query_map(params![id], queries::row_to_track)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Resolve a fuzzy query (path, title, etc.) to track IDs.
    pub fn resolve_tracks(&self, query: &str, limit: usize) -> Result<Vec<i64>> {
        queries::resolve_tracks(self, query, limit)
    }
}

/// Convenience: escape a value for FTS5 MATCH by wrapping in double quotes.
pub fn fts_quote(s: &str) -> String {
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

/// Convert an arbitrary decode error into our error type.
pub fn map_sql_err(e: rusqlite::Error) -> Error {
    Error::Db(e)
}
