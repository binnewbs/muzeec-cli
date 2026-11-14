//! Album-art extraction and a hash-addressed, size-capped cache.

use std::path::{Path, PathBuf};

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use lofty::probe::Probe;

use crate::error::{Error, Result};

/// Sidecar cover filenames, in priority order.
const SIDECAR_NAMES: &[&str] = &["cover", "folder", "front", "album", "artwork", "albumart"];
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];

/// Disk-backed artwork cache keyed by blake3 hash.
#[derive(Debug, Clone)]
pub struct ArtCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl ArtCache {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, max_bytes })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path to a cached original image by hash.
    pub fn path_for(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.img"))
    }

    /// Path to a resized thumbnail by hash and size.
    pub fn thumb_path(&self, hash: &str, size: u32) -> PathBuf {
        self.dir.join(format!("{hash}-{size}.jpg"))
    }

    /// Store image bytes, returning the content hash. Deduplicates.
    pub fn store_bytes(&self, data: &[u8]) -> Result<String> {
        let hash = blake3::hash(data).to_hex().to_string();
        let path = self.path_for(&hash);
        if !path.exists() {
            std::fs::write(&path, data)?;
        }
        Ok(hash)
    }

    /// Extract embedded art from an audio file, if present.
    pub fn extract_embedded(&self, audio: &Path) -> Result<Option<String>> {
        let Some((data, _mime)) = read_embedded_picture(audio)? else {
            return Ok(None);
        };
        Ok(Some(self.store_bytes(&data)?))
    }

    /// Find a sidecar cover image next to the file.
    pub fn sidecar(&self, audio: &Path) -> Option<String> {
        let dir = audio.parent()?;
        let cover = find_sidecar(dir)?;
        let data = std::fs::read(&cover).ok()?;
        self.store_bytes(&data).ok()
    }

    /// Resolve artwork for a track: cached hash, else embedded, else sidecar.
    pub fn resolve(&self, audio: &Path, known_hash: Option<&str>) -> Option<String> {
        if let Some(h) = known_hash
            && self.path_for(h).exists()
        {
            return Some(h.to_string());
        }
        if let Ok(Some(h)) = self.extract_embedded(audio) {
            return Some(h);
        }
        self.sidecar(audio)
    }

    /// Produce (or reuse) a resized JPEG thumbnail.
    pub fn thumbnail(&self, hash: &str, size: u32) -> Result<PathBuf> {
        let out = self.thumb_path(hash, size);
        if out.exists() {
            return Ok(out);
        }
        let src = self.path_for(hash);
        // Cached originals use a `.img` extension, so sniff the format from the
        // bytes instead of relying on the filename.
        let img = image::ImageReader::open(&src)?
            .with_guessed_format()?
            .decode()?;
        let thumb = img.thumbnail(size, size);
        thumb.save_with_format(&out, image::ImageFormat::Jpeg)?;
        Ok(out)
    }

    /// Evict least-recently-used files until the cache is under its cap.
    pub fn cleanup_lru(&self) -> Result<u64> {
        let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            let atime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            entries.push((atime, meta.len(), entry.path()));
        }
        if total <= self.max_bytes {
            return Ok(0);
        }
        entries.sort_by_key(|(t, _, _)| *t);
        let mut freed = 0u64;
        for (_, len, path) in entries {
            if total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
                freed += len;
            }
        }
        Ok(freed)
    }
}

/// Locate a sidecar cover file in `dir`.
pub fn find_sidecar(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    for name in SIDECAR_NAMES {
        for entry in &entries {
            let stem = entry
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let ext = entry
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if stem == *name && IMAGE_EXTS.contains(&ext.as_str()) {
                return Some(entry.clone());
            }
        }
    }
    None
}

/// Read the preferred embedded picture (front cover first).
pub fn read_embedded_picture(audio: &Path) -> Result<Option<(Vec<u8>, String)>> {
    let file = Probe::open(audio)
        .map_err(|e| Error::Metadata(e.to_string()))?
        .options(ParseOptions::new())
        .read()
        .map_err(|e| Error::Metadata(e.to_string()))?;
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return Ok(None);
    };
    let pictures = tag.pictures();
    if pictures.is_empty() {
        return Ok(None);
    }
    let pic = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .unwrap_or(&pictures[0]);
    let mime = pic
        .mime_type()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "image/jpeg".to_string());
    Ok(Some((pic.data().to_vec(), mime)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_dedupes_by_hash() {
        let dir = std::env::temp_dir().join(format!("muzeec-art-{}", std::process::id()));
        let cache = ArtCache::new(dir.clone(), 10_000).unwrap();
        let h1 = cache.store_bytes(b"hello image").unwrap();
        let h2 = cache.store_bytes(b"hello image").unwrap();
        assert_eq!(h1, h2);
        assert!(cache.path_for(&h1).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cleanup_evicts() {
        let dir = std::env::temp_dir().join(format!("muzeec-art-lru-{}", std::process::id()));
        let cache = ArtCache::new(dir.clone(), 10).unwrap();
        cache.store_bytes(b"0123456789abcdef").unwrap();
        cache.store_bytes(b"zyxwvutsrqponmlk").unwrap();
        let freed = cache.cleanup_lru().unwrap();
        assert!(freed > 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
