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
        state: crate::protocol::State,
        level: f32,
        idle_hot: bool,
        ts: u64,
    },
    State {
        state: crate::protocol::State,
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
    TranscriptionResult {
        text: String,
        duration: f32,
        model: String,
    },
    Error {
        error: String,
    },
}

/// Socket client with reconnection logic
pub struct OsdSocket {
    stream: Option<BufReader<UnixStream>>,
    write_stream: Option<UnixStream>,
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
            write_stream: None,
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

        // Clone the underlying stream for writing
        let write_stream = reader.get_ref().try_clone()?;

        self.stream = Some(reader);
        self.write_stream = Some(write_stream);
        self.reconnect_state.attempt = 0;
        Ok(())
    }

    /// Send a transcribe request to the service
    pub fn send_transcribe(&mut self, max_duration: u64, silence_duration: u64, sample_rate: u32) -> Result<()> {
        let Some(stream) = &mut self.write_stream else {
            return Err(anyhow!("Not connected to socket"));
        };

        let request = crate::protocol::Request::new_transcribe(max_duration, silence_duration, sample_rate);
        let request_json = serde_json::to_string(&request)?;
        writeln!(stream, "{}", request_json)?;
        stream.flush()?;

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

/// Parse message from server (events or responses)
fn parse_message(msg: Value) -> Result<OsdMessage> {
    let msg_type = msg.get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing type field"))?;

    match msg_type {
        "event" => {
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
        "result" => {
            // Handle Response::Result for transcription
            let data = msg.get("data")
                .ok_or_else(|| anyhow!("Missing data field"))?;
            
            if let (Some(text), Some(duration), Some(model)) = (
                data.get("text").and_then(|v| v.as_str()),
                data.get("duration").and_then(|v| v.as_f64()),
                data.get("model").and_then(|v| v.as_str()),
            ) {
                Ok(OsdMessage::TranscriptionResult {
                    text: text.to_string(),
                    duration: duration as f32,
                    model: model.to_string(),
                })
            } else {
                // Might be a different kind of result (e.g., subscription ack)
                Err(anyhow!("Unexpected result format: {:?}", data))
            }
        }
        "error" => {
            let data = msg.get("data")
                .ok_or_else(|| anyhow!("Missing data field"))?;
            let error = data.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            Ok(OsdMessage::Error { error })
        }
        _ => Err(anyhow!("Unknown message type: {}", msg_type))
    }
}
