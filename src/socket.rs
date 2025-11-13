use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// Import protocol types with aliases to avoid conflicts during migration
use crate::protocol::{Event as ProtocolEvent, Response as ProtocolResponse};

#[derive(Error, Debug)]
pub enum SocketError {
    #[error("Socket connection error: {0}")]
    Connection(String),
    #[error("Socket I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ResponseType {
    Result,
    Error,
    Status,
    Event,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Response {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub response_type: ResponseType,
    pub data: serde_json::Value,
}

impl Response {
    pub fn new(id: Uuid, response_type: ResponseType, data: serde_json::Value) -> Self {
        Self {
            id,
            response_type,
            data,
        }
    }

    pub fn result(id: Uuid, data: serde_json::Value) -> Self {
        Self::new(id, ResponseType::Result, data)
    }

    pub fn error(id: Uuid, error: String) -> Self {
        Self::new(
            id,
            ResponseType::Error,
            serde_json::json!({ "error": error }),
        )
    }

    /// Create a Response from the new Event type (compatibility helper)
    pub fn from_event(event: ProtocolEvent) -> Self {
        let data = serde_json::to_value(event)
            .expect("Event serialization should never fail");
        
        Self {
            id: Uuid::nil(), // Events don't have an id (broadcast)
            response_type: ResponseType::Event,
            data,
        }
    }

    /// Create a Response from the new Response type (compatibility helper)
    pub fn from_protocol_response(response: ProtocolResponse) -> Self {
        let id = response.id();
        let (response_type, data) = match response {
            ProtocolResponse::Result { text, duration, model, .. } => {
                (ResponseType::Result, serde_json::json!({
                    "text": text,
                    "duration": duration,
                    "model": model,
                }))
            }
            ProtocolResponse::Error { error, .. } => {
                (ResponseType::Error, serde_json::json!({ "error": error }))
            }
            ProtocolResponse::Status {
                service_running,
                model_loaded,
                model_path,
                audio_device,
                uptime_seconds,
                last_activity_seconds_ago,
                ..
            } => {
                (ResponseType::Status, serde_json::json!({
                    "service_running": service_running,
                    "model_loaded": model_loaded,
                    "model_path": model_path,
                    "audio_device": audio_device,
                    "uptime_seconds": uptime_seconds,
                    "last_activity_seconds_ago": last_activity_seconds_ago,
                }))
            }
            ProtocolResponse::Subscribed { .. } => {
                (ResponseType::Result, serde_json::json!({ "subscribed": true }))
            }
        };
        
        Self {
            id,
            response_type,
            data,
        }
    }
}
