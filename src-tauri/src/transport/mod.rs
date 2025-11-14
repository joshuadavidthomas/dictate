//! Transport layer for socket communication

use thiserror::Error;

mod async_transport;
mod codec;
mod sync_transport;

pub use async_transport::{AsyncConnection, AsyncTransport};
pub use codec::decode_server_message;
pub use codec::encode_server_message;
pub use sync_transport::SyncTransport;

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
