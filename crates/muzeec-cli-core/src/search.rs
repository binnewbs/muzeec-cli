//! Search query parsing and SQL generation.
//!
//! Supports free text plus filters: `artist:`, `album:`, `genre:`, `year:`,
//! `is:lossless`, `is:instrumental`, `dur:>5m`, `rated:>=4`.

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

use crate::db::TRACK_COLUMNS;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub albumartist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub lossless: bool,
    pub instrumental: bool,
    pub duration: Option<String>,
    pub rating: Option<String>,
}

impl SearchQuery {
    /// Parse a raw query string into structured filters + free text.
    pub fn parse(input: &str) -> Self {
        let mut q = SearchQuery::default();
        let mut text = Vec::new();
        for token in split_tokens(input) {
            if let Some((key, val)) = token.split_once(':') {
                let val = val.trim();
                match key.to_ascii_lowercase().as_str() {
                    "artist" => q.artist = Some(val.to_string()),
                    "album" => q.album = Some(val.to_string()),
                    "albumartist" => q.albumartist = Some(val.to_string()),
                    "genre" => q.genre = Some(val.to_string()),
                    "year" => q.year = Some(val.to_string()),
                    "dur" | "duration" => q.duration = Some(val.to_string()),
                    "rated" | "rating" => q.rating = Some(val.to_string()),
                    "is" => match val.to_ascii_lowercase().as_str() {
                        "lossless" => q.lossless = true,
                        "instrumental" => q.instrumental = true,
                        other => text.push(format!("is:{other}")),
                    },
                    _ => text.push(token),
                }
            } else {
                text.push(token);
            }
        }
        q.text = text.join(" ").trim().to_string();
        q
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.artist.is_none()
            && self.album.is_none()
            && self.albumartist.is_none()
            && self.genre.is_none()
            && self.year.is_none()
            && !self.lossless
            && !self.instrumental
            && self.duration.is_none()
            && self.rating.is_none()
    }

    fn fts_expr(&self) -> Option<String> {
        let terms: Vec<String> = self
            .text
            .split_whitespace()
            .filter_map(|t| {
                let cleaned: String = t
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if cleaned.is_empty() {
                    None
                } else {
                    Some(format!("{cleaned}*"))
                }
            })
            .collect();
        if terms.is_empty() {
            None
        } else {
            Some(terms.join(" "))
        }
    }

    /// Build SQL and bound parameters for this query.
    pub fn build(&self, limit: usize) -> (String, Vec<Value>) {
        let cols = TRACK_COLUMNS
            .split(", ")
            .map(|c| format!("t.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut params: Vec<Value> = Vec::new();
        let mut conds: Vec<String> = vec!["t.error IS NULL".to_string()];
        let mut joins = String::new();
        let mut pi = 0usize;
        let mut ph = |params: &mut Vec<Value>, v: Value| {
            params.push(v);
            pi += 1;
            format!("?{pi}")
        };

        let fts = self.fts_expr();
        let mut order = if fts.is_some() {
            String::new()
        } else {
            "ORDER BY t.albumartist COLLATE NOCASE, t.album COLLATE NOCASE, t.disc_no, t.track_no"
                .to_string()
        };

        if let Some(expr) = &fts {
            let p = ph(&mut params, Value::Text(expr.clone()));
            joins.push_str(" JOIN tracks_fts ON tracks_fts.rowid = t.id");
            conds.push(format!("tracks_fts MATCH {p}"));
            // BM25 with field boosts: title, artist, album, albumartist, genre, filename.
            order = "ORDER BY bm25(tracks_fts, 10.0, 5.0, 3.0, 3.0, 2.0, 1.0)".to_string();
        }
        if let Some(v) = &self.artist {
            let p = ph(&mut params, Value::Text(format!("%{v}%")));
            conds.push(format!("t.artist LIKE {p}"));
        }
        if let Some(v) = &self.album {
            let p = ph(&mut params, Value::Text(format!("%{v}%")));
            conds.push(format!("t.album LIKE {p}"));
        }
        if let Some(v) = &self.albumartist {
            let p = ph(&mut params, Value::Text(format!("%{v}%")));
            conds.push(format!("t.albumartist LIKE {p}"));
        }
        if let Some(v) = &self.genre {
            let p = ph(&mut params, Value::Text(format!("%{v}%")));
            conds.push(format!("t.genre LIKE {p}"));
        }
        if let Some(v) = &self.year
            && let Some(clause) = parse_numeric_filter(v, "t.year")
        {
            let p = ph(&mut params, Value::Integer(clause.1));
            conds.push(format!("{} {p}", clause.0));
        }
        if self.lossless {
            conds.push("LOWER(t.codec) IN ('flac','alac','wav','aiff','ape','wavpack')".into());
        }
        if self.instrumental {
            conds.push(
                "(LOWER(t.genre) LIKE '%instrumental%' OR LOWER(t.title) LIKE '%instrumental%')"
                    .into(),
            );
        }
        if let Some(v) = &self.duration
            && let Some(clause) = parse_duration_filter(v, "t.duration_ms")
        {
            let p = ph(&mut params, Value::Integer(clause.1));
            conds.push(format!("{} {p}", clause.0));
        }
        if let Some(v) = &self.rating
            && let Some(clause) = parse_numeric_filter(v, "COALESCE(r.stars,0)")
        {
            joins.push_str(" LEFT JOIN ratings r ON r.track_id = t.id");
            let p = ph(&mut params, Value::Integer(clause.1));
            conds.push(format!("{} {p}", clause.0));
        }

        let sql = format!(
            "SELECT {cols} FROM tracks t{joins} WHERE {} {order} LIMIT {limit}",
            conds.join(" AND ")
        );
        (sql, params)
    }

    pub fn to_sql(&self, limit: usize) -> String {
        self.build(limit).0
    }

    pub fn sql_params(&self) -> Vec<Value> {
        self.build(0).1
    }
}

/// Split an input on whitespace but keep quoted segments intact.
fn split_tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in input.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Parse a comparison like `>=4`, `<100`, `2000` into (op, value).
fn parse_numeric_filter(input: &str, _col: &str) -> Option<(String, i64)> {
    let input = input.trim();
    let (op, rest) = if let Some(r) = input.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = input.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = input.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = input.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = input.strip_prefix('=') {
        ("=", r)
    } else {
        ("=", input)
    };
    let val: i64 = rest.trim().parse().ok()?;
    Some((op.to_string(), val))
}

/// Parse `>5m`, `<3:30`, `300s`, `90` into (op, milliseconds).
fn parse_duration_filter(input: &str, _col: &str) -> Option<(String, i64)> {
    let input = input.trim();
    let (op, rest) = if let Some(r) = input.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = input.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = input.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = input.strip_prefix('<') {
        ("<", r)
    } else {
        ("=", input)
    };
    let ms = parse_duration_ms(rest.trim())?;
    Some((op.to_string(), ms))
}

/// Parse `5m`, `5:30`, `1h2m`, `300s`, or bare seconds into ms.
pub fn parse_duration_ms(input: &str) -> Option<i64> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(ms) = input.strip_suffix("ms")
        && let Ok(v) = ms.trim().parse::<f64>()
    {
        return Some(v as i64);
    }
    if input.contains(':') {
        let parts: Vec<&str> = input.split(':').collect();
        if parts.len() == 2 {
            let m: i64 = parts[0].parse().ok()?;
            let s: i64 = parts[1].parse().ok()?;
            return Some((m * 60 + s) * 1000);
        }
        if parts.len() == 3 {
            let h: i64 = parts[0].parse().ok()?;
            let m: i64 = parts[1].parse().ok()?;
            let s: i64 = parts[2].parse().ok()?;
            return Some(((h * 3600) + (m * 60) + s) * 1000);
        }
        return None;
    }
    let mut total: f64 = 0.0;
    let mut num = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
        } else if ch.is_ascii_alphabetic() {
            let n: f64 = num.parse().ok()?;
            num.clear();
            total += match ch.to_ascii_lowercase() {
                'h' => n * 3600.0,
                'm' => n * 60.0,
                's' => n,
                _ => return None,
            };
        } else {
            return None;
        }
    }
    if !num.is_empty() {
        let n: f64 = num.parse().ok()?;
        total += n;
    }
    Some((total * 1000.0) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_free_text() {
        let q = SearchQuery::parse("miles davis");
        assert_eq!(q.text, "miles davis");
    }

    #[test]
    fn parses_filters() {
        let q = SearchQuery::parse(
            "kind artist:miles genre:jazz year:>1950 is:lossless dur:>5m rated:>=4",
        );
        assert_eq!(q.text, "kind");
        assert_eq!(q.artist.as_deref(), Some("miles"));
        assert_eq!(q.genre.as_deref(), Some("jazz"));
        assert!(q.lossless);
        assert_eq!(q.duration.as_deref(), Some(">5m"));
        assert_eq!(q.rating.as_deref(), Some(">=4"));
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration_ms("5m"), Some(300_000));
        assert_eq!(parse_duration_ms("3:30"), Some(210_000));
        assert_eq!(parse_duration_ms("1h2m30s"), Some(3_750_000));
        assert_eq!(parse_duration_ms("90"), Some(90_000));
        assert_eq!(parse_duration_ms("500ms"), Some(500));
        assert_eq!(parse_duration_ms("1.5s"), Some(1_500));
    }

    #[test]
    fn builds_sql_with_params() {
        let q = SearchQuery::parse("kind artist:miles is:lossless");
        let (sql, params) = q.build(10);
        assert!(sql.contains("tracks_fts MATCH"));
        assert!(sql.contains("LOWER(t.codec)"));
        assert_eq!(params.len(), 2);
    }
}
