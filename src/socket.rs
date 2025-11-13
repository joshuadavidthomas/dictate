use thiserror::Error;

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
