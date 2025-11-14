//! Transport layer for socket communication
//!
//! This module provides a unified abstraction for socket-based communication
//! with separate implementations for async (tokio) and sync (std) I/O.
//!
//! The transport layer is organized into:
//! - `codec`: NDJSON encoding/decoding for messages
//! - `async_transport`: Tokio-based async client transport
//! - `sync_transport`: Std-based sync client transport

use thiserror::Error;

mod async_transport;
mod codec;
mod sync_transport;

pub use async_transport::{AsyncConnection, AsyncTransport};
pub use codec::decode_server_message;
pub use codec::encode_server_message;
pub use sync_transport::SyncTransport;

/// Default Unix socket path for the dictate service
pub const DEFAULT_SOCKET_PATH: &str = "/run/user/$UID/dictate/dictate.sock";

/// Socket error types
#[derive(Error, Debug)]
pub enum SocketError {
    #[error("Socket connection error: {0}")]
    Connection(String),
    #[error("Socket I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
