use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl State {
    pub fn as_str(&self) -> &'static str {
        match self {
            State::Idle => "Ready",
            State::Recording => "Recording",
            State::Transcribing => "Transcribing",
            State::Error => "Error",
        }
    }
}

/// Messages sent from server to client
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMessage {
    /// Successful transcription result
    Result {
        id: Uuid,
        text: String,
        duration: f32,
        model: String,
    },
    /// Error response
    Error { id: Uuid, error: String },
    /// Status information (in response to status request)
    Status {
        id: Uuid,
        service_running: bool,
        model_loaded: bool,
        model_path: String,
        audio_device: String,
        uptime_seconds: u64,
        last_activity_seconds_ago: u64,
        state: State,
    },
    /// Subscription confirmation
    Subscribed { id: Uuid },
    /// Status event broadcast (sent periodically to subscribers)
    #[serde(rename = "status_event")]
    StatusEvent {
        state: State,
        spectrum: Option<Vec<f32>>,
        idle_hot: bool,
        ts: u64,
        #[serde(default = "default_version")]
        ver: u32,
    },
    /// Configuration update broadcast
    #[serde(rename = "config_update")]
    ConfigUpdate {
        osd_position: crate::conf::OsdPosition,
    },
}

fn default_version() -> u32 {
    1
}

impl ServerMessage {
    /// Create a Result response
    pub fn new_result(id: Uuid, text: String, duration: f32, model: String) -> Self {
        ServerMessage::Result {
            id,
            text,
            duration,
            model,
        }
    }

    /// Create a Status response
    pub fn new_status(
        id: Uuid,
        service_running: bool,
        model_loaded: bool,
        model_path: String,
        audio_device: String,
        uptime_seconds: u64,
        last_activity_seconds_ago: u64,
        state: State,
    ) -> Self {
        ServerMessage::Status {
            id,
            service_running,
            model_loaded,
            model_path,
            audio_device,
            uptime_seconds,
            last_activity_seconds_ago,
            state,
        }
    }

    /// Create a StatusEvent broadcast
    pub fn new_status_event(
        state: State,
        spectrum: Option<Vec<f32>>,
        idle_hot: bool,
        ts: u64,
    ) -> Self {
        ServerMessage::StatusEvent {
            state,
            spectrum,
            idle_hot,
            ts,
            ver: 1,
        }
    }

    /// Create a ConfigUpdate broadcast
    pub fn new_config_update(osd_position: crate::conf::OsdPosition) -> Self {
        ServerMessage::ConfigUpdate { osd_position }
    }
}
