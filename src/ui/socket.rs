//! Socket client for subscribing to OSD events

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

/// OSD message types from server
#[derive(Debug, Clone)]
pub enum OsdMessage {
    Status {
        state: String,
        level: f32,
        idle_hot: bool,
        ts: u64,
    },
    State {
        state: String,
        idle_hot: bool,
        ts: u64,
    },
    Level {
        v: f32,
        ts: u64,
    },
    Spectrum {
        bands: Vec<f32>,
        ts: u64,
    },
}

/// Socket client with reconnection logic
pub struct OsdSocket {
    stream: Option<BufReader<UnixStream>>,
    pub path: String,
    reconnect_state: ReconnectState,
}

struct ReconnectState {
    attempt: u32,
    next_retry: Instant,
}

impl OsdSocket {
    pub fn new(path: String) -> Self {
        Self {
            stream: None,
            path,
            reconnect_state: ReconnectState {
                attempt: 0,
                next_retry: Instant::now(),
            },
        }
    }

    /// Connect and subscribe to events
    pub fn connect(&mut self) -> Result<()> {
        // 1. Connect to socket
        let mut stream = UnixStream::connect(&self.path)
            .map_err(|e| anyhow!("Failed to connect to {}: {}", self.path, e))?;

        // 2. Send subscribe request using typed Request
        let subscribe_request = crate::protocol::Request::new_subscribe();
        let subscribe_json = serde_json::to_string(&subscribe_request)?;
        writeln!(stream, "{}", subscribe_json)?;
        stream.flush()?;

        // 3. Read acknowledgment
        let mut reader = BufReader::new(stream);
        let mut ack = String::new();
        reader.read_line(&mut ack)?;

        // Parse acknowledgment to verify subscription
        let ack_json: Value = serde_json::from_str(&ack)?;
        if ack_json["type"] != "result" || ack_json["data"]["subscribed"] != true {
            return Err(anyhow!("Failed to subscribe: {:?}", ack_json));
        }

        // Set socket to non-blocking mode
        reader.get_ref().set_nonblocking(true)?;

        self.stream = Some(reader);
        self.reconnect_state.attempt = 0;
        Ok(())
    }

    /// Read next message from stream
    pub fn read_message(&mut self) -> Result<Option<OsdMessage>> {
        let Some(reader) = &mut self.stream else {
            return Ok(None);
        };

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // EOF - connection closed
                self.stream = None;
                Err(anyhow!("Connection closed"))
            }
            Ok(_) => {
                let msg: Value = serde_json::from_str(&line)?;
                Ok(Some(parse_message(msg)?))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available right now (non-blocking mode)
                Ok(None)
            }
            Err(e) => {
                self.stream = None;
                Err(e.into())
            }
        }
    }

    /// Check if we should attempt reconnection
    pub fn should_reconnect(&self, now: Instant) -> bool {
        self.stream.is_none() && now >= self.reconnect_state.next_retry
    }

    /// Schedule next reconnection attempt
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

/// Parse event message from server
/// Now uses typed Event enum instead of manual JSON parsing
fn parse_message(msg: Value) -> Result<OsdMessage> {
    // Extract the data field which contains the Event enum
    let event_data = msg.get("data")
        .ok_or_else(|| anyhow!("Missing data field"))?;
    
    // Deserialize directly to Event enum
    let event: crate::protocol::Event = serde_json::from_value(event_data.clone())?;
    
    // Convert Event to OsdMessage
    match event {
        crate::protocol::Event::Status { state, level, idle_hot, ts, .. } => {
            Ok(OsdMessage::Status {
                state,
                level,
                idle_hot,
                ts,
            })
        }
        crate::protocol::Event::State { state, idle_hot, ts, .. } => {
            Ok(OsdMessage::State {
                state,
                idle_hot,
                ts,
            })
        }
        crate::protocol::Event::Level { v, ts, .. } => {
            Ok(OsdMessage::Level { v, ts })
        }
        crate::protocol::Event::Spectrum { bands, ts, .. } => {
            Ok(OsdMessage::Spectrum { bands, ts })
        }
    }
}
