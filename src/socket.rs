use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

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
pub struct Message {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub message_type: MessageType,
    pub params: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    Transcribe,
    Status,
    Stop,
    Subscribe,
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

impl Message {
    pub fn new(message_type: MessageType, params: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_type,
            params,
        }
    }

    pub fn transcribe(params: serde_json::Value) -> Self {
        Self::new(MessageType::Transcribe, params)
    }

    pub fn status(params: serde_json::Value) -> Self {
        Self::new(MessageType::Status, params)
    }

    pub fn stop(params: serde_json::Value) -> Self {
        Self::new(MessageType::Stop, params)
    }
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

    pub fn status(id: Uuid, data: serde_json::Value) -> Self {
        Self::new(id, ResponseType::Status, data)
    }

    pub fn event(event_name: &str, data: serde_json::Value) -> Self {
        let mut event_data = serde_json::json!({
            "event": event_name,
        });
        
        // Merge data fields into event_data
        if let (Some(obj), Some(data_obj)) = (event_data.as_object_mut(), data.as_object()) {
            for (k, v) in data_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
        
        Self {
            id: Uuid::nil(), // Events don't have an id (broadcast)
            response_type: ResponseType::Event,
            data: event_data,
        }
    }
}
