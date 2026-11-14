//! Ordered schema migrations. Index `i` upgrades the DB from version `i` to
//! `i + 1` (`PRAGMA user_version`).

pub const MIGRATIONS: &[&str] = &[
    // v1: core schema + FTS5
    r#"
CREATE TABLE IF NOT EXISTS tracks (
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  filename TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL DEFAULT '',
  artist TEXT NOT NULL DEFAULT '',
  albumartist TEXT NOT NULL DEFAULT '',
  album TEXT NOT NULL DEFAULT '',
  genre TEXT NOT NULL DEFAULT '',
  composer TEXT,
  comment TEXT,
  track_no INTEGER,
  disc_no INTEGER,
  year INTEGER,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  sample_rate INTEGER NOT NULL DEFAULT 0,
  channels INTEGER NOT NULL DEFAULT 0,
  bitrate INTEGER,
  codec TEXT NOT NULL DEFAULT '',
  bpm REAL,
  lufs REAL,
  true_peak REAL,
  rg_track_gain REAL,
  rg_track_peak REAL,
  rg_album_gain REAL,
  rg_album_peak REAL,
  size INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  art_hash TEXT,
  added_at INTEGER NOT NULL DEFAULT 0,
  last_seen_at INTEGER NOT NULL DEFAULT 0,
  error TEXT
);
CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album);
CREATE INDEX IF NOT EXISTS idx_tracks_albumartist ON tracks(albumartist);
CREATE INDEX IF NOT EXISTS idx_tracks_genre ON tracks(genre);

CREATE TABLE IF NOT EXISTS albums (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  albumartist TEXT NOT NULL DEFAULT '',
  year INTEGER,
  art_hash TEXT,
  track_count INTEGER NOT NULL DEFAULT 0,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  UNIQUE(name, albumartist)
);

CREATE TABLE IF NOT EXISTS playlists (
  id INTEGER PRIMARY KEY,
  name TEXT UNIQUE NOT NULL,
  created_at INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS playlist_items (
  playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  PRIMARY KEY (playlist_id, position)
);

CREATE TABLE IF NOT EXISTS queue_state (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  payload TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS history (
  id INTEGER PRIMARY KEY,
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  played_at INTEGER NOT NULL DEFAULT 0,
  ms_played INTEGER NOT NULL DEFAULT 0,
  completed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_history_track ON history(track_id);

CREATE TABLE IF NOT EXISTS ratings (
  track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
  stars INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
  title, artist, album, albumartist, genre, filename,
  content='tracks', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2',
  prefix='2 3 4'
);

CREATE TRIGGER IF NOT EXISTS tracks_ai AFTER INSERT ON tracks BEGIN
  INSERT INTO tracks_fts(rowid, title, artist, album, albumartist, genre, filename)
  VALUES (new.id, new.title, new.artist, new.album, new.albumartist, new.genre, new.filename);
END;

CREATE TRIGGER IF NOT EXISTS tracks_ad AFTER DELETE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, albumartist, genre, filename)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.albumartist, old.genre, old.filename);
END;

CREATE TRIGGER IF NOT EXISTS tracks_au AFTER UPDATE ON tracks BEGIN
  INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, albumartist, genre, filename)
  VALUES ('delete', old.id, old.title, old.artist, old.album, old.albumartist, old.genre, old.filename);
  INSERT INTO tracks_fts(rowid, title, artist, album, albumartist, genre, filename)
  VALUES (new.id, new.title, new.artist, new.album, new.albumartist, new.genre, new.filename);
END;
"#,
];
