//! IPC wire protocol: newline-delimited JSON over a Unix stream socket.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version, bumped on incompatible wire changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// A request from a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, cmd: impl Into<String>, params: Value) -> Self {
        Self {
            id,
            cmd: cmd.into(),
            params,
        }
    }
}

/// A response to a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, data: Value) -> Self {
        Self {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// An asynchronous event pushed to subscribed clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

impl Event {
    pub fn new(event: impl Into<String>, data: Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }
}

/// Anything the server may send to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    Response(Response),
    Event(Event),
}

/// Parse a single line from a client into a request.
pub fn parse_request(line: &str) -> std::result::Result<Request, String> {
    serde_json::from_str(line).map_err(|e| format!("invalid request: {e}"))
}

/// Maximum accepted line length (guard against oversized messages).
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
