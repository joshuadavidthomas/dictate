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

/// Messages sent from clients to the server
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    /// Request to transcribe audio
    Transcribe {
        id: Uuid,
        #[serde(default = "default_max_duration")]
        max_duration: u64,
        #[serde(default = "default_silence_duration")]
        silence_duration: u64,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
    },
    /// Start recording (manual control)
    Start {
        id: Uuid,
        #[serde(default = "default_max_duration")]
        max_duration: u64,
        #[serde(default)]
        silence_duration: Option<u64>,
        #[serde(default = "default_sample_rate")]
        sample_rate: u32,
    },
    /// Stop current recording
    Stop { id: Uuid },
    /// Request server status
    Status { id: Uuid },
    /// Subscribe to server events
    Subscribe { id: Uuid },
}

fn default_max_duration() -> u64 {
    30
}
fn default_silence_duration() -> u64 {
    2
}
fn default_sample_rate() -> u32 {
    16000
}

impl ClientMessage {
    /// Create a new Transcribe request
    pub fn new_transcribe(max_duration: u64, silence_duration: u64, sample_rate: u32) -> Self {
        ClientMessage::Transcribe {
            id: Uuid::new_v4(),
            max_duration,
            silence_duration,
            sample_rate,
        }
    }

    /// Create a new Start request
    pub fn new_start(max_duration: u64, silence_duration: Option<u64>, sample_rate: u32) -> Self {
        ClientMessage::Start {
            id: Uuid::new_v4(),
            max_duration,
            silence_duration,
            sample_rate,
        }
    }

    /// Create a new Stop request
    pub fn new_stop() -> Self {
        ClientMessage::Stop { id: Uuid::new_v4() }
    }

    /// Create a new Status request
    pub fn new_status() -> Self {
        ClientMessage::Status { id: Uuid::new_v4() }
    }

    /// Create a new Subscribe request
    pub fn new_subscribe() -> Self {
        ClientMessage::Subscribe { id: Uuid::new_v4() }
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
    ) -> Self {
        ServerMessage::Status {
            id,
            service_running,
            model_loaded,
            model_path,
            audio_device,
            uptime_seconds,
            last_activity_seconds_ago,
        }
    }

    /// Create a Subscribed response
    pub fn new_subscribed(id: Uuid) -> Self {
        ServerMessage::Subscribed { id }
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
}


