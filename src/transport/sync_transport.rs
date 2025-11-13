//! Sync transport implementation using std
//!
//! This module provides a synchronous socket client for UI/OSD communication.
//! It uses non-blocking I/O for reading and includes reconnection logic.

use crate::protocol::Request;
use crate::transport::codec;
use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

/// Sync transport for socket communication with reconnection support
pub struct SyncTransport {
    reader: Option<BufReader<UnixStream>>,
    writer: Option<UnixStream>,
    socket_path: String,
    reconnect_state: ReconnectState,
}

struct ReconnectState {
    attempt: u32,
    next_retry: Instant,
}

impl SyncTransport {
    /// Create a new sync transport with the given socket path
    pub fn new(socket_path: String) -> Self {
        Self {
            reader: None,
            writer: None,
            socket_path,
            reconnect_state: ReconnectState {
                attempt: 0,
                next_retry: Instant::now(),
            },
        }
    }

    /// Connect to the socket
    pub fn connect(&mut self) -> Result<()> {
        // Connect to socket
        let stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| anyhow!("Failed to connect to {}: {}", self.socket_path, e))?;

        // Create reader
        let reader = BufReader::new(stream);

        // Set socket to non-blocking mode
        reader.get_ref().set_nonblocking(true)?;

        // Clone the underlying stream for writing
        let writer = reader.get_ref().try_clone()?;

        self.reader = Some(reader);
        self.writer = Some(writer);
        self.reconnect_state.attempt = 0;

        Ok(())
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.reader.is_some() && self.writer.is_some()
    }

    /// Disconnect and clean up
    pub fn disconnect(&mut self) {
        self.reader = None;
        self.writer = None;
    }

    /// Send a request
    pub fn send_request(&mut self, request: &Request) -> Result<()> {
        let Some(writer) = &mut self.writer else {
            return Err(anyhow!("Not connected to socket"));
        };

        let message = codec::encode_request(request)
            .map_err(|e| anyhow!("Failed to encode request: {}", e))?;

        writer.write_all(message.as_bytes())?;
        writer.flush()?;

        Ok(())
    }

    /// Read a line from the socket (non-blocking)
    pub fn read_line(&mut self) -> Result<Option<String>> {
        let Some(reader) = &mut self.reader else {
            return Ok(None);
        };

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // EOF - connection closed
                self.disconnect();
                Err(anyhow!("Connection closed"))
            }
            Ok(_) => Ok(Some(line)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available right now (non-blocking mode)
                Ok(None)
            }
            Err(e) => {
                self.disconnect();
                Err(e.into())
            }
        }
    }

    /// Check if we should attempt reconnection
    pub fn should_reconnect(&self, now: Instant) -> bool {
        !self.is_connected() && now >= self.reconnect_state.next_retry
    }

    /// Schedule next reconnection attempt with exponential backoff
    pub fn schedule_reconnect(&mut self) {
        self.reconnect_state.attempt += 1;
        let delay = match self.reconnect_state.attempt {
            0..=1 => Duration::from_secs(1),
            2..=3 => Duration::from_secs(2),
            _ => Duration::from_secs(5),
        };
        self.reconnect_state.next_retry = Instant::now() + delay;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_creation() {
        let transport = SyncTransport::new("/tmp/test.sock".to_string());
        assert!(!transport.is_connected());
    }

    #[test]
    fn test_reconnect_backoff() {
        let mut transport = SyncTransport::new("/tmp/test.sock".to_string());

        let now = Instant::now();
        transport.schedule_reconnect();
        assert!(transport.should_reconnect(now + Duration::from_millis(1100)));

        transport.schedule_reconnect();
        assert!(transport.reconnect_state.attempt == 2);
    }
}
