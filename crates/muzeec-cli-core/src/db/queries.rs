//! Query helpers layered on top of [`Database`].

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{AlbumSummary, Database, Playlist, Track, now_unix};
use crate::error::{Error, Result};
use crate::search::SearchQuery;

pub const TRACK_COLUMNS: &str = "id, path, filename, title, artist, albumartist, album, genre, \
composer, comment, track_no, disc_no, year, duration_ms, sample_rate, channels, bitrate, codec, \
bpm, lufs, true_peak, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, size, mtime, \
art_hash, added_at, last_seen_at, error";

pub fn row_to_track(r: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get(0)?,
        path: r.get(1)?,
        filename: r.get(2)?,
        title: r.get(3)?,
        artist: r.get(4)?,
        albumartist: r.get(5)?,
        album: r.get(6)?,
        genre: r.get(7)?,
        composer: r.get(8)?,
        comment: r.get(9)?,
        track_no: r.get(10)?,
        disc_no: r.get(11)?,
        year: r.get(12)?,
        duration_ms: r.get::<_, i64>(13)?.max(0) as u64,
        sample_rate: r.get::<_, i64>(14)?.max(0) as u32,
        channels: r.get::<_, i64>(15)?.max(0) as u16,
        bitrate: r.get(16)?,
        codec: r.get(17)?,
        bpm: r.get(18)?,
        lufs: r.get(19)?,
        true_peak: r.get(20)?,
        rg_track_gain: r.get(21)?,
        rg_track_peak: r.get(22)?,
        rg_album_gain: r.get(23)?,
        rg_album_peak: r.get(24)?,
        size: r.get::<_, i64>(25)?.max(0) as u64,
        mtime: r.get(26)?,
        art_hash: r.get(27)?,
        added_at: r.get(28)?,
        last_seen_at: r.get(29)?,
        error: r.get(30)?,
    })
}

/// Data needed to upsert a scanned track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackUpsert {
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
    pub rg_track_gain: Option<f32>,
    pub rg_track_peak: Option<f32>,
    pub rg_album_gain: Option<f32>,
    pub rg_album_peak: Option<f32>,
    pub size: u64,
    pub mtime: i64,
    pub art_hash: Option<String>,
    pub error: Option<String>,
}

impl Database {
    /// Insert or update a track, returning its id. Unchanged files (same
    /// mtime+size) are skipped by the scanner before reaching here.
    pub fn upsert_track(&self, t: &TrackUpsert) -> Result<i64> {
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO tracks (path, filename, title, artist, albumartist, album, genre,
                composer, comment, track_no, disc_no, year, duration_ms, sample_rate, channels,
                bitrate, codec, bpm, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak,
                size, mtime, art_hash, added_at, last_seen_at, error)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,
                ?21,?22,?23,?24,?25,?26,?27,?28)
             ON CONFLICT(path) DO UPDATE SET
                filename=excluded.filename, title=excluded.title, artist=excluded.artist,
                albumartist=excluded.albumartist, album=excluded.album, genre=excluded.genre,
                composer=excluded.composer, comment=excluded.comment, track_no=excluded.track_no,
                disc_no=excluded.disc_no, year=excluded.year, duration_ms=excluded.duration_ms,
                sample_rate=excluded.sample_rate, channels=excluded.channels,
                bitrate=excluded.bitrate, codec=excluded.codec, bpm=excluded.bpm,
                rg_track_gain=excluded.rg_track_gain, rg_track_peak=excluded.rg_track_peak,
                rg_album_gain=excluded.rg_album_gain, rg_album_peak=excluded.rg_album_peak,
                size=excluded.size, mtime=excluded.mtime, art_hash=excluded.art_hash,
                last_seen_at=excluded.last_seen_at, error=excluded.error",
            params![
                t.path,
                t.filename,
                t.title,
                t.artist,
                t.albumartist,
                t.album,
                t.genre,
                t.composer,
                t.comment,
                t.track_no,
                t.disc_no,
                t.year,
                t.duration_ms as i64,
                t.sample_rate,
                t.channels,
                t.bitrate,
                t.codec,
                t.bpm,
                t.rg_track_gain,
                t.rg_track_peak,
                t.rg_album_gain,
                t.rg_album_peak,
                t.size as i64,
                t.mtime,
                t.art_hash,
                now,
                now,
                t.error
            ],
        )?;
        self.conn
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                params![t.path],
                |r| r.get(0),
            )
            .map_err(Error::from)
    }

    /// Record that a file was seen unchanged during a scan.
    pub fn touch_track(&self, id: i64, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET last_seen_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Remove tracks whose path is under `prefix` and were not seen since `scan_start`.
    pub fn prune_stale(&self, prefix: &str, scan_start: i64) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM tracks WHERE path LIKE ?1 || '%' AND last_seen_at < ?2",
            params![prefix, scan_start],
        )?;
        Ok(n)
    }

    pub fn all_tracks(&self, limit: usize, offset: usize) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {TRACK_COLUMNS} FROM tracks WHERE error IS NULL
             ORDER BY albumartist, album, disc_no, track_no, title LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], row_to_track)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    pub fn artists(&self) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT artist, COUNT(*) FROM tracks WHERE artist != '' AND error IS NULL
             GROUP BY artist ORDER BY artist COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    pub fn genres(&self) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT genre, COUNT(*) FROM tracks WHERE genre != '' AND error IS NULL
             GROUP BY genre ORDER BY genre COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    pub fn albums(&self) -> Result<Vec<AlbumSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, albumartist, year, art_hash, track_count, duration_ms FROM albums
             ORDER BY albumartist COLLATE NOCASE, year, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AlbumSummary {
                name: r.get(0)?,
                albumartist: r.get(1)?,
                year: r.get(2)?,
                art_hash: r.get(3)?,
                track_count: r.get::<_, i64>(4)? as u32,
                duration_ms: r.get::<_, i64>(5)?.max(0) as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    pub fn album_tracks(&self, album: &str, albumartist: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {TRACK_COLUMNS} FROM tracks WHERE album = ?1 AND albumartist = ?2
             AND error IS NULL ORDER BY disc_no, track_no, title"
        ))?;
        let rows = stmt.query_map(params![album, albumartist], row_to_track)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    /// Full-text search using the parsed query. Returns ranked tracks.
    pub fn search(&self, q: &SearchQuery, limit: usize) -> Result<Vec<Track>> {
        let sql = q.to_sql(limit);
        let params = q.sql_params();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), row_to_track)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    // ---- Playlists -------------------------------------------------------

    pub fn playlist_create(&self, name: &str) -> Result<i64> {
        let now = now_unix();
        self.conn.execute(
            "INSERT INTO playlists(name, created_at, updated_at) VALUES (?1, ?2, ?2)
             ON CONFLICT(name) DO UPDATE SET updated_at = excluded.updated_at",
            params![name, now],
        )?;
        self.conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .map_err(Error::from)
    }

    pub fn playlist_delete(&self, name: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM playlists WHERE name = ?1", params![name])?;
        Ok(n > 0)
    }

    pub fn playlist_rename(&self, old: &str, new: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE playlists SET name = ?1, updated_at = ?2 WHERE name = ?3",
            params![new, now_unix(), old],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("playlist '{old}'")));
        }
        Ok(())
    }

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.created_at, p.updated_at,
                    (SELECT COUNT(*) FROM playlist_items i WHERE i.playlist_id = p.id)
             FROM playlists p ORDER BY p.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Playlist {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                track_count: r.get::<_, i64>(4)? as u32,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    fn playlist_id(&self, name: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("playlist '{name}'")))
    }

    pub fn playlist_add(&self, name: &str, track_ids: &[i64]) -> Result<()> {
        let pid = self.playlist_id(name)?;
        let tx = self.conn.unchecked_transaction()?;
        let next: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position)+1, 0) FROM playlist_items WHERE playlist_id = ?1",
            params![pid],
            |r| r.get(0),
        )?;
        for (i, tid) in track_ids.iter().enumerate() {
            tx.execute(
                "INSERT OR REPLACE INTO playlist_items(playlist_id, position, track_id)
                 VALUES (?1, ?2, ?3)",
                params![pid, next + i as i64, tid],
            )?;
        }
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now_unix(), pid],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn playlist_remove_position(&self, name: &str, position: usize) -> Result<()> {
        let pid = self.playlist_id(name)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1 AND position = ?2",
            params![pid, position as i64],
        )?;
        // Renumber positions to stay contiguous.
        tx.execute(
            "UPDATE playlist_items SET position = (
                SELECT COUNT(*) FROM playlist_items i2
                WHERE i2.playlist_id = playlist_items.playlist_id
                  AND i2.position < playlist_items.position)
             WHERE playlist_id = ?1",
            params![pid],
        )?;
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now_unix(), pid],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn playlist_move(&self, name: &str, from: usize, to: usize) -> Result<()> {
        let pid = self.playlist_id(name)?;
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
            )?;
            let rows = stmt.query_map(params![pid], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if from >= ids.len() || to >= ids.len() {
            return Err(Error::Invalid(format!(
                "position out of range (len {})",
                ids.len()
            )));
        }
        let mut ids = ids;
        let item = ids.remove(from);
        ids.insert(to, item);
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM playlist_items WHERE playlist_id = ?1",
            params![pid],
        )?;
        for (i, tid) in ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1,?2,?3)",
                params![pid, i as i64, tid],
            )?;
        }
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now_unix(), pid],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn playlist_tracks(&self, name: &str) -> Result<Vec<Track>> {
        let pid = self.playlist_id(name)?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM playlist_items i JOIN tracks t ON t.id = i.track_id
             WHERE i.playlist_id = ?1 ORDER BY i.position",
            TRACK_COLUMNS
                .split(", ")
                .map(|c| format!("t.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let rows = stmt.query_map(params![pid], row_to_track)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Error::from)
    }

    // ---- Ratings & history ----------------------------------------------

    pub fn set_rating(&self, track_id: i64, stars: u8) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ratings(track_id, stars, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(track_id) DO UPDATE SET stars = excluded.stars, updated_at = excluded.updated_at",
            params![track_id, stars.min(5), now_unix()],
        )?;
        Ok(())
    }

    pub fn rating(&self, track_id: i64) -> Result<u8> {
        Ok(self
            .conn
            .query_row(
                "SELECT stars FROM ratings WHERE track_id = ?1",
                params![track_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u8)
    }

    pub fn record_history(&self, track_id: i64, ms_played: u64, completed: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO history(track_id, played_at, ms_played, completed) VALUES (?1,?2,?3,?4)",
            params![track_id, now_unix(), ms_played as i64, completed as i64],
        )?;
        Ok(())
    }

    // ---- Queue persistence ----------------------------------------------

    pub fn save_queue_state(&self, payload: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO queue_state(id, payload, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
            params![payload, now_unix()],
        )?;
        Ok(())
    }

    pub fn load_queue_state(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT payload FROM queue_state WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }
}

/// Resolve a free-form query to track ids: numeric ids, exact paths, then FTS.
pub fn resolve_tracks(db: &Database, query: &str, limit: usize) -> Result<Vec<i64>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(id) = query.parse::<i64>()
        && db.track_by_id(id)?.is_some()
    {
        return Ok(vec![id]);
    }
    let path = std::path::Path::new(query);
    if path.exists()
        && let Some(abs) = path
            .canonicalize()
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned))
        && let Some(t) = db.track_by_path(&abs)?
    {
        return Ok(vec![t.id]);
    }
    let sq = SearchQuery::parse(query);
    let tracks = db.search(&sq, limit)?;
    Ok(tracks.into_iter().map(|t| t.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Database {
        Database::open_memory().unwrap()
    }

    fn track(path: &str, title: &str) -> TrackUpsert {
        TrackUpsert {
            path: path.into(),
            filename: path.rsplit('/').next().unwrap().into(),
            title: title.into(),
            artist: "Artist".into(),
            albumartist: "Artist".into(),
            album: "Album".into(),
            genre: "Rock".into(),
            duration_ms: 1000,
            sample_rate: 44100,
            channels: 2,
            codec: "flac".into(),
            size: 10,
            mtime: 1,
            ..Default::default()
        }
    }

    #[test]
    fn fts5_available() {
        assert!(mem().fts5_available());
    }

    #[test]
    fn upsert_and_fetch() {
        let db = mem();
        let id = db.upsert_track(&track("/a/b.flac", "Hello")).unwrap();
        let t = db.track_by_id(id).unwrap().unwrap();
        assert_eq!(t.title, "Hello");
        assert!(t.is_lossless());
    }

    #[test]
    fn upsert_is_idempotent() {
        let db = mem();
        let id1 = db.upsert_track(&track("/a/b.flac", "Hello")).unwrap();
        let id2 = db.upsert_track(&track("/a/b.flac", "Hello2")).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(db.track_by_id(id1).unwrap().unwrap().title, "Hello2");
    }

    #[test]
    fn search_matches_title() {
        let db = mem();
        db.upsert_track(&track("/a/b.flac", "So What")).unwrap();
        db.upsert_track(&track("/a/c.flac", "Blue in Green"))
            .unwrap();
        let q = SearchQuery::parse("what");
        let hits = db.search(&q, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "So What");
    }

    #[test]
    fn playlist_roundtrip() {
        let db = mem();
        let id = db.upsert_track(&track("/a/b.flac", "T1")).unwrap();
        db.playlist_create("roadtrip").unwrap();
        db.playlist_add("roadtrip", &[id]).unwrap();
        let tracks = db.playlist_tracks("roadtrip").unwrap();
        assert_eq!(tracks.len(), 1);
        db.playlist_remove_position("roadtrip", 0).unwrap();
        assert!(db.playlist_tracks("roadtrip").unwrap().is_empty());
    }

    #[test]
    fn ratings_roundtrip() {
        let db = mem();
        let id = db.upsert_track(&track("/a/b.flac", "T")).unwrap();
        assert_eq!(db.rating(id).unwrap(), 0);
        db.set_rating(id, 4).unwrap();
        assert_eq!(db.rating(id).unwrap(), 4);
    }
}
