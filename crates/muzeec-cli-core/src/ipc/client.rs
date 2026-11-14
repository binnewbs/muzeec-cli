//! IPC client used by the CLI and TUI.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::proto::{Event, PROTOCOL_VERSION, Request, ServerMessage};
use crate::error::{Error, Result};

/// A synchronous IPC client connection.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
    path: PathBuf,
}

impl Client {
    /// Connect to the daemon socket at `path`.
    pub fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .map_err(|e| Error::DaemonUnreachable(format!("{}: {e}", path.display())))?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Perform the version handshake; errors on mismatch.
    pub fn handshake(&mut self) -> Result<()> {
        let v = self.call("hello", Value::Null)?;
        let server_version = v.get("version").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if server_version != PROTOCOL_VERSION {
            return Err(Error::Ipc(format!(
                "protocol mismatch: client {PROTOCOL_VERSION}, daemon {server_version}"
            )));
        }
        Ok(())
    }

    /// Send a command and wait for its matching response. Events received
    /// while waiting are returned via the optional `events` sink.
    pub fn call(&mut self, cmd: &str, params: Value) -> Result<Value> {
        self.call_with_events(cmd, params, &mut |_| {})
    }

    pub fn call_with_events(
        &mut self,
        cmd: &str,
        params: Value,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request::new(id, cmd, params);
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self.reader.read_line(&mut buf)?;
            if n == 0 {
                return Err(Error::Ipc("daemon closed the connection".into()));
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: ServerMessage = serde_json::from_str(trimmed)
                .map_err(|e| Error::Ipc(format!("bad server message: {e}")))?;
            match msg {
                ServerMessage::Response(r) if r.id == id => {
                    if r.ok {
                        return Ok(r.data.unwrap_or(Value::Null));
                    }
                    return Err(Error::Other(
                        r.error.unwrap_or_else(|| "unknown daemon error".into()),
                    ));
                }
                ServerMessage::Response(_) => continue,
                ServerMessage::Event(e) => on_event(e),
            }
        }
    }

    /// Typed convenience wrapper.
    pub fn call_typed<T: DeserializeOwned>(&mut self, cmd: &str, params: Value) -> Result<T> {
        let v = self.call(cmd, params)?;
        serde_json::from_value(v).map_err(Error::from)
    }

    /// Read the next event (call after `subscribe`).
    pub fn next_event(&mut self) -> Result<Event> {
        loop {
            let mut buf = String::new();
            let n = self.reader.read_line(&mut buf)?;
            if n == 0 {
                return Err(Error::Ipc("daemon closed the connection".into()));
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<ServerMessage>(trimmed) {
                Ok(ServerMessage::Event(e)) => return Ok(e),
                Ok(ServerMessage::Response(_)) => continue,
                Err(e) => return Err(Error::Ipc(format!("bad server message: {e}"))),
            }
        }
    }

    /// Subscribe to the daemon event stream.
    pub fn subscribe(&mut self) -> Result<()> {
        self.call("subscribe", Value::Null)?;
        Ok(())
    }
}

/// Try to connect to a daemon socket, returning `None` if unreachable.
pub fn try_connect(path: &Path) -> Option<Client> {
    Client::connect(path).ok()
}

/// Response type re-exported for convenience.
pub use super::proto::{Request as IpcRequest, Response as IpcResponse};
