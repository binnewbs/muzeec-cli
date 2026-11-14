//! muzeec-cli core library: all logic, no UI.

pub mod config;
pub mod db;
pub mod engine;
pub mod error;
pub mod ipc;
pub mod library;
pub mod mix;
pub mod paths;
pub mod playlist;
pub mod queue;
pub mod replaygain;
pub mod search;

pub use error::{Error, Result};
pub use paths::Paths;
