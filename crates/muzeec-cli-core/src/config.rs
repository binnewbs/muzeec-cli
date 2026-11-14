use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths::{Paths, expand_tilde};

pub const DEFAULT_MUSIC_DIR: &str = "~/Music";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LibraryConfig {
    pub music_dirs: Vec<String>,
    pub follow_symlinks: bool,
    pub watch: bool,
    pub scan_workers: usize,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            music_dirs: vec![DEFAULT_MUSIC_DIR.to_string()],
            follow_symlinks: false,
            watch: true,
            scan_workers: 0,
        }
    }
}

impl LibraryConfig {
    /// Resolved absolute music directories.
    pub fn resolved_dirs(&self) -> Vec<PathBuf> {
        self.music_dirs.iter().map(|d| expand_tilde(d)).collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainMode {
    Off,
    #[default]
    Track,
    Album,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PlaybackConfig {
    pub device: String,
    /// Engine sample rate; `"auto"` follows the device default. Stored as a
    /// flexible value because the config may contain either an integer or the
    /// string `"auto"`.
    #[serde(
        deserialize_with = "de_sample_rate",
        serialize_with = "ser_sample_rate"
    )]
    pub engine_sample_rate: Option<u32>,
    pub buffer_ms: u32,
    pub gapless: bool,
    pub crossfade_secs: f32,
    pub crossfade_on_album_change_only: bool,
    pub replaygain: ReplayGainMode,
    pub volume: u8,
    pub resume_on_start: bool,
    pub idle_stop: bool,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            device: "default".into(),
            engine_sample_rate: Some(48_000),
            buffer_ms: 20,
            gapless: true,
            crossfade_secs: 0.0,
            crossfade_on_album_change_only: false,
            replaygain: ReplayGainMode::Track,
            volume: 70,
            resume_on_start: true,
            idle_stop: false,
        }
    }
}

fn de_sample_rate<'de, D>(d: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(u32),
        Text(String),
    }
    match Option::<Raw>::deserialize(d)? {
        None => Ok(None),
        Some(Raw::Num(n)) => Ok(Some(n)),
        Some(Raw::Text(s)) if s == "auto" => Ok(None),
        Some(Raw::Text(s)) => s.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

fn ser_sample_rate<S>(v: &Option<u32>, s: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match v {
        Some(n) => s.serialize_u32(*n),
        None => s.serialize_str("auto"),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ArtMode {
    #[default]
    Auto,
    Kitty,
    Sixel,
    Iterm2,
    Halfblocks,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VisualizerMode {
    #[default]
    Bars,
    Osc,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub theme: String,
    pub album_art: ArtMode,
    pub visualizer: VisualizerMode,
    pub fps: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".into(),
            album_art: ArtMode::Auto,
            visualizer: VisualizerMode::Bars,
            fps: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct IntegrationsConfig {
    pub mpris: bool,
    pub notifications: bool,
    pub notify_art: bool,
}

impl Default for IntegrationsConfig {
    fn default() -> Self {
        Self {
            mpris: true,
            notifications: true,
            notify_art: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct EqConfig {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands: Vec<f32>,
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preamp_db: 0.0,
            bands: vec![0.0; 10],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct KeysConfig {
    /// Map of key name to action name.
    pub bindings: BTreeMap<String, String>,
}

/// Root configuration document.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub library: LibraryConfig,
    pub playback: PlaybackConfig,
    pub ui: UiConfig,
    pub integrations: IntegrationsConfig,
    pub eq: EqConfig,
    pub keys: KeysConfig,
}

impl Config {
    /// Load config from the standard location, or defaults if absent.
    pub fn load(paths: &Paths) -> Result<Self> {
        Self::load_from(&paths.config_file())
    }

    /// Load config from an explicit path; missing file yields defaults.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                Self::parse(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Reject nonsensical values with a clear message instead of panicking later.
    pub fn validate(&self) -> Result<()> {
        if !(10..=100).contains(&self.playback.buffer_ms) {
            return Err(Error::Config(format!(
                "playback.buffer_ms must be 10..=100, got {}",
                self.playback.buffer_ms
            )));
        }
        if !(0.0..=12.0).contains(&self.playback.crossfade_secs) {
            return Err(Error::Config(format!(
                "playback.crossfade_secs must be 0..=12, got {}",
                self.playback.crossfade_secs
            )));
        }
        if self.playback.volume > 100 {
            return Err(Error::Config(format!(
                "playback.volume must be 0..=100, got {}",
                self.playback.volume
            )));
        }
        if self.eq.bands.len() != 10 {
            return Err(Error::Config(format!(
                "eq.bands must have 10 entries, got {}",
                self.eq.bands.len()
            )));
        }
        Ok(())
    }

    /// Default config text written by `config edit` when no file exists yet.
    pub fn default_toml() -> String {
        toml::to_string_pretty(&Config::default()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn parses_partial_config() {
        let cfg = Config::parse("[playback]\nvolume = 42\n").unwrap();
        assert_eq!(cfg.playback.volume, 42);
        assert_eq!(cfg.playback.buffer_ms, 20);
    }

    #[test]
    fn parses_auto_sample_rate() {
        let cfg = Config::parse("[playback]\nengine_sample_rate = \"auto\"\n").unwrap();
        assert_eq!(cfg.playback.engine_sample_rate, None);
    }

    #[test]
    fn rejects_bad_buffer() {
        assert!(Config::parse("[playback]\nbuffer_ms = 5\n").is_err());
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(Config::parse("[playback]\nbogus = 1\n").is_err());
    }

    #[test]
    fn tilde_expansion() {
        unsafe { std::env::set_var("HOME", "/home/test") };
        assert_eq!(expand_tilde("~/Music"), PathBuf::from("/home/test/Music"));
    }
}
