//! Socket client for subscribing to OSD events

use anyhow::{anyhow, Result};
use crate::protocol::{Message, Response, Event};
use std::time::Instant;

/// Socket message unwrapped from wire protocol
#[derive(Debug, Clone)]
pub enum SocketMessage {
    Event(Event),
    Response(Response),
}

/// Socket client with reconnection logic
pub struct OsdSocket {
    transport: crate::transport::SyncTransport,
}

impl OsdSocket {
    pub fn new(path: String) -> Self {
        Self {
            transport: crate::transport::SyncTransport::new(path),
        }
    }

    /// Connect and subscribe to events
    pub fn connect(&mut self) -> Result<()> {
        // 1. Connect to socket
        self.transport.connect()?;

        // 2. Send subscribe request using typed Request
        let subscribe_request = crate::protocol::Request::new_subscribe();
        self.transport.send_request(&subscribe_request)?;

        // 3. Read acknowledgment
        let ack = self.transport.read_line()?
            .ok_or_else(|| anyhow!("No acknowledgment received"))?;

        // Parse acknowledgment to verify subscription
        let message = crate::transport::codec::decode_message(&ack)
            .map_err(|e| anyhow!("Failed to decode message: {}", e))?;

        // Verify it's a Subscribed response
        match message {
            Message::Response(Response::Subscribed { .. }) => Ok(()),
            _ => Err(anyhow!("Failed to subscribe: unexpected response")),
        }
    }

    /// Send a transcribe request to the service
    pub fn send_transcribe(&mut self, max_duration: u64, silence_duration: u64, sample_rate: u32) -> Result<()> {
        let request = crate::protocol::Request::new_transcribe(max_duration, silence_duration, sample_rate);
        self.transport.send_request(&request)?;
        Ok(())
    }

    /// Read next message from stream, unwrapping the protocol Message wrapper
    pub fn read_message(&mut self) -> Result<Option<SocketMessage>> {
        match self.transport.read_line()? {
            Some(line) => {
                let message = crate::transport::codec::decode_message(&line)
                    .map_err(|e| anyhow!("Failed to decode message: {}", e))?;
                
                // Unwrap protocol::Message at the socket boundary
                let unwrapped = match message {
                    Message::Event(e) => SocketMessage::Event(e),
                    Message::Response(r) => SocketMessage::Response(r),
                };
                Ok(Some(unwrapped))
            }
            None => Ok(None),
        }
    }

    /// Check if we should attempt reconnection
    pub fn should_reconnect(&self, now: Instant) -> bool {
        self.transport.should_reconnect(now)
    }

    /// Schedule next reconnection attempt
    pub fn schedule_reconnect(&mut self) {
        self.transport.schedule_reconnect();
    }
}
