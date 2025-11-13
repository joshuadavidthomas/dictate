//! Async transport implementation using tokio
//!
//! This module provides an async socket client for communicating with the dictate service.

use crate::protocol::{ClientMessage, ServerMessage};
use crate::socket::SocketError;
use crate::transport::codec;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Async transport for socket communication (stateless client)
pub struct AsyncTransport {
    socket_path: String,
}

/// Async transport for server-side connection handling (stateful)
pub struct AsyncConnection {
    pub reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    pub writer: tokio::net::unix::OwnedWriteHalf,
}

impl AsyncTransport {
    /// Create a new async transport with the given socket path
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    /// Connect to the socket and return a stateful connection
    pub async fn connect(&self) -> Result<AsyncConnection, SocketError> {
        let stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
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
        })?;

        let (reader, writer) = stream.into_split();
        Ok(AsyncConnection {
            reader: BufReader::new(reader),
            writer,
        })
    }

    /// Send a client message and receive a server response (one-shot request-response)
    pub async fn send_request(&self, message: &ClientMessage) -> Result<ServerMessage, SocketError> {
        let mut conn = self.connect().await?;

        // Send message
        conn.write_message(message).await?;

        // Read response with timeout (2 minutes for long transcriptions)
        let response = tokio::time::timeout(
            Duration::from_secs(120),
            conn.read_server_message(),
        )
        .await
        .map_err(|_| SocketError::Connection("Request timed out after 2 minutes".to_string()))??
        .ok_or_else(|| SocketError::Connection("No response from server".to_string()))?;

        Ok(response)
    }
}

impl AsyncConnection {
    /// Read a client message from the connection (server-side)
    pub async fn read_client_message(&mut self) -> Result<Option<ClientMessage>, SocketError> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await {
            Ok(0) => Ok(None), // EOF - connection closed
            Ok(_) => {
                let message = codec::decode_client_message(&line)?;
                Ok(Some(message))
            }
            Err(e) => Err(SocketError::Io(e)),
        }
    }

    /// Read a server message from the connection (client-side)
    pub async fn read_server_message(&mut self) -> Result<Option<ServerMessage>, SocketError> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await {
            Ok(0) => Ok(None), // EOF - connection closed
            Ok(_) => {
                let message = codec::decode_server_message(&line)?;
                Ok(Some(message))
            }
            Err(e) => Err(SocketError::Io(e)),
        }
    }

    /// Write a client message to the connection (client-side)
    pub async fn write_message(&mut self, message: &ClientMessage) -> Result<(), SocketError> {
        let encoded = codec::encode_client_message(message)?;
        self.writer.write_all(encoded.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Write a server message to the connection (server-side)
    pub async fn write_server_message(&mut self, message: &ServerMessage) -> Result<(), SocketError> {
        let encoded = codec::encode_server_message(message)?;
        self.writer.write_all(encoded.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }
}
