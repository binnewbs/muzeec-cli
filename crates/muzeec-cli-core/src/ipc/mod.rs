//! Inter-process protocol types, server, and client.

pub mod client;
pub mod proto;
pub mod server;

pub use client::Client;
pub use proto::{Event, PROTOCOL_VERSION, Request, Response, ServerMessage};
pub use server::{Handler, IpcServer};
