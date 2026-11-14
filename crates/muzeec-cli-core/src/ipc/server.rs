//! IPC server: accepts Unix-socket clients, dispatches commands, streams events.

use crossbeam_channel::Receiver;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::proto::{Event, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Response, parse_request};
use crate::error::{Error, Result};

/// Command handler implemented by the daemon.
pub trait Handler: Send + Sync {
    /// Execute a command and return its JSON payload.
    fn handle(&self, cmd: &str, params: Value) -> Result<Value>;

    /// Register a new event subscriber.
    fn subscribe(&self) -> Receiver<Event>;
}

/// A bound IPC server.
pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
}

impl IpcServer {
    /// Bind the socket, replacing a stale socket file if present.
    pub fn bind(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            // Detect a live daemon; if reachable, refuse to clobber it.
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(Error::Ipc(format!(
                        "another daemon is already listening on {}",
                        path.display()
                    )));
                }
                Err(_) => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let listener = UnixListener::bind(path)?;
        restrict_socket(path)?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accept connections forever, spawning a thread per client.
    pub fn serve(&self, handler: Arc<dyn Handler>) -> Result<()> {
        for conn in self.listener.incoming() {
            match conn {
                Ok(stream) => {
                    let handler = handler.clone();
                    std::thread::Builder::new()
                        .name("ipc-client".into())
                        .spawn(move || {
                            if let Err(e) = handle_client(stream, handler) {
                                tracing::debug!(error = %e, "client disconnected");
                            }
                        })?;
                }
                Err(e) => tracing::warn!(error = %e, "accept failed"),
            }
        }
        Ok(())
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn handle_client(stream: UnixStream, handler: Arc<dyn Handler>) -> Result<()> {
    if !peer_allowed(&stream) {
        tracing::warn!("rejecting IPC client with mismatched uid");
        return Ok(());
    }
    let writer = Arc::new(std::sync::Mutex::new(stream.try_clone()?));
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        if line.len() > MAX_MESSAGE_BYTES {
            write_message(&writer, &Response::error(0, "message too large"))?;
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Request = match parse_request(trimmed) {
            Ok(r) => r,
            Err(e) => {
                write_message(&writer, &Response::error(0, e))?;
                continue;
            }
        };

        match req.cmd.as_str() {
            "hello" => {
                let resp = Response::ok(
                    req.id,
                    json!({"version": PROTOCOL_VERSION, "name": "muzeec-cli-daemon"}),
                );
                write_message(&writer, &resp)?;
            }
            "subscribe" => {
                let rx = handler.subscribe();
                let resp = Response::ok(req.id, json!({"subscribed": true}));
                write_message(&writer, &resp)?;
                let w = writer.clone();
                std::thread::Builder::new()
                    .name("ipc-events".into())
                    .spawn(move || {
                        while let Ok(ev) = rx.recv() {
                            if write_message(&w, &ev).is_err() {
                                break;
                            }
                        }
                    })?;
            }
            _ => {
                let resp = match handler.handle(&req.cmd, req.params) {
                    Ok(data) => Response::ok(req.id, data),
                    Err(e) => Response::error(req.id, e.to_string()),
                };
                write_message(&writer, &resp)?;
            }
        }
    }
    Ok(())
}

fn write_message<T: serde::Serialize>(
    writer: &Arc<std::sync::Mutex<UnixStream>>,
    msg: &T,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    let mut guard = writer.lock().unwrap_or_else(|e| e.into_inner());
    guard.write_all(line.as_bytes())?;
    guard.flush()
}

fn restrict_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

/// Check that the connecting peer has the same uid as this process.
#[cfg(target_os = "linux")]
fn peer_allowed(stream: &UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    #[repr(C)]
    struct UCred {
        pid: i32,
        uid: u32,
        gid: u32,
    }
    unsafe extern "C" {
        fn getsockopt(
            fd: i32,
            level: i32,
            optname: i32,
            optval: *mut core::ffi::c_void,
            optlen: *mut u32,
        ) -> i32;
    }
    const SOL_SOCKET: i32 = 1;
    const SO_PEERCRED: i32 = 17;
    let mut cred = UCred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<UCred>() as u32;
    let rc = unsafe {
        getsockopt(
            stream.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            &mut cred as *mut UCred as *mut core::ffi::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return false;
    }
    cred.uid == unsafe { getuid() }
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn getuid() -> u32;
}

#[cfg(not(target_os = "linux"))]
fn peer_allowed(_stream: &UnixStream) -> bool {
    true
}

/// Serialize a response for tests.
pub fn encode_response(resp: &Response) -> String {
    serde_json::to_string(resp).unwrap_or_default()
}
