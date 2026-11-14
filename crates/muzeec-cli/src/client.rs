//! Connecting to (and auto-spawning) the daemon.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use muzeec_cli_core::ipc::Client;
use muzeec_cli_core::paths::Paths;
use serde_json::Value;

/// A connected daemon client.
pub struct DaemonClient {
    client: Client,
}

impl DaemonClient {
    /// Connect, spawning the daemon if necessary (unless disabled).
    pub fn connect(no_daemon: bool) -> Result<Self, String> {
        let paths = Paths::resolve();
        let socket = paths.socket_path();
        if let Ok(mut client) = Client::connect(&socket) {
            let _ = client.handshake();
            return Ok(Self { client });
        }
        if no_daemon {
            return Err(format!(
                "daemon unreachable at {} (auto-spawn disabled)",
                socket.display()
            ));
        }
        spawn_daemon()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if let Ok(mut client) = Client::connect(&socket) {
                if let Ok(()) = client.handshake() {
                    return Ok(Self { client });
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "daemon did not start within 8s (socket {})",
            socket.display()
        ))
    }

    pub fn call(&mut self, cmd: &str, params: Value) -> Result<Value, String> {
        self.client.call(cmd, params).map_err(|e| e.to_string())
    }

    pub fn into_inner(self) -> Client {
        self.client
    }
}

fn daemon_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("muzeec-cli-daemon");
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from("muzeec-cli-daemon")
}

fn spawn_daemon() -> Result<(), String> {
    let bin = daemon_binary();
    std::process::Command::new(&bin)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot spawn {}: {e}", bin.display()))
}
