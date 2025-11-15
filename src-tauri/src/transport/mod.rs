//! Transport layer for socket communication

use thiserror::Error;

mod codec;

pub use codec::decode_server_message;
pub use codec::encode_server_message;

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
