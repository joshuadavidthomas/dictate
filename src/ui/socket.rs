//! Socket client for subscribing to OSD events

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::time::Instant;

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
        let ack_json: Value = serde_json::from_str(&ack)?;
        if ack_json["type"] != "result" || ack_json["data"]["subscribed"] != true {
            return Err(anyhow!("Failed to subscribe: {:?}", ack_json));
        }

        Ok(())
    }

    /// Send a transcribe request to the service
    pub fn send_transcribe(&mut self, max_duration: u64, silence_duration: u64, sample_rate: u32) -> Result<()> {
        let request = crate::protocol::Request::new_transcribe(max_duration, silence_duration, sample_rate);
        self.transport.send_request(&request)?;
        Ok(())
    }

    /// Read next message from stream
    pub fn read_message(&mut self) -> Result<Option<OsdMessage>> {
        match self.transport.read_line()? {
            Some(line) => {
                let msg: Value = serde_json::from_str(&line)?;
                Ok(Some(parse_message(msg)?))
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

    /// Get the socket path
    pub fn path(&self) -> &str {
        self.transport.socket_path()
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
