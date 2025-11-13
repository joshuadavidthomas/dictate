//! Async transport implementation using tokio
//!
//! This module provides an async socket client for communicating with the dictate service.

use crate::protocol::Request;
use crate::socket::{Response, SocketError};
use crate::transport::codec;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Async transport for socket communication
pub struct AsyncTransport {
    socket_path: String,
}

impl AsyncTransport {
    /// Create a new async transport with the given socket path
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    /// Connect to the socket and return a stream
    async fn connect(&self) -> Result<UnixStream, SocketError> {
        UnixStream::connect(&self.socket_path).await.map_err(|e| {
            match e.kind() {
                std::io::ErrorKind::ConnectionRefused => SocketError::Connection(
                    "Service is not running. Use 'dictate service' to start the service."
                        .to_string(),
                ),
                std::io::ErrorKind::NotFound => SocketError::Connection(format!(
                    "Service socket not found at {}. Use 'dictate service' to start the service.",
                    self.socket_path
                )),
                _ => SocketError::Connection(format!(
                    "Failed to connect to service at {}: {}",
                    self.socket_path, e
                )),
            }
        })
    }

    /// Send a request and receive a response
    pub async fn send_request(&self, request: &Request) -> Result<Response, SocketError> {
        let mut stream = self.connect().await?;

        // Encode and send request
        let message = codec::encode_request(request)?;
        stream.write_all(message.as_bytes()).await?;
        stream.flush().await?;

        // Read response with timeout (2 minutes for long transcriptions)
        let mut buffer = vec![0u8; 4096];
        let read_result = tokio::time::timeout(
            Duration::from_secs(120),
            stream.read(&mut buffer),
        )
        .await;

        let n = match read_result {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(SocketError::Io(e)),
            Err(_) => {
                return Err(SocketError::Connection(
                    "Request timed out after 2 minutes".to_string(),
                ))
            }
        };

        if n == 0 {
            return Err(SocketError::Connection(
                "No response from server".to_string(),
            ));
        }

        // Decode response
        let response_str = String::from_utf8_lossy(&buffer[..n]);
        codec::decode_response(&response_str)
    }

    /// Get the socket path
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_creation() {
        let transport = AsyncTransport::new("/tmp/test.sock".to_string());
        assert_eq!(transport.socket_path(), "/tmp/test.sock");
    }
}
