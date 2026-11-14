use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::Result;

/// XDG-derived filesystem locations, with env overrides for tests.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self::resolve()
    }
}

impl Paths {
    /// Resolve standard locations, honouring `MUZEEC_CLI_*` test overrides first.
    pub fn resolve() -> Self {
        if let Some(p) = Self::from_env_overrides() {
            return p;
        }
        let dirs =
            ProjectDirs::from("", "", "muzeec-cli").expect("cannot determine home directory");
        Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
            state_dir: dirs
                .state_dir()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| dirs.data_dir().to_path_buf()),
            runtime_dir: runtime_base().join("muzeec-cli"),
        }
    }

    fn from_env_overrides() -> Option<Self> {
        let root = std::env::var_os("MUZEEC_CLI_ROOT")?;
        let root = PathBuf::from(root);
        Some(Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            runtime_dir: root.join("run"),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn keys_file(&self) -> PathBuf {
        self.config_dir.join("keys.toml")
    }

    pub fn db_file(&self) -> PathBuf {
        self.data_dir.join("library.db")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn art_dir(&self) -> PathBuf {
        self.cache_dir.join("art")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.runtime_dir.join("sock")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.runtime_dir.join("daemon.lock")
    }

    /// Create all directories the daemon needs.
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.state_dir,
            &self.runtime_dir,
            &self.art_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        // The runtime dir holds a control socket; keep it private.
        restrict_permissions(&self.runtime_dir)?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn runtime_base() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    // Fall back to a per-uid dir under the system temp dir.
    let uid = unsafe { libc_getuid() };
    PathBuf::from("/tmp").join(format!("muzeec-cli-{uid}"))
}

#[cfg(unix)]
unsafe fn libc_getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(not(unix))]
unsafe fn libc_getuid() -> u32 {
    0
}

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    if input == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(input)
}
