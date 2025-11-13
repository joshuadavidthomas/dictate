use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Requests sent from clients to the server
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Request {
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
    /// Request server status
    Status { id: Uuid },
    /// Request to stop the server
    Stop { id: Uuid },
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

impl Request {
    /// Create a new Transcribe request
    pub fn new_transcribe(max_duration: u64, silence_duration: u64, sample_rate: u32) -> Self {
        Request::Transcribe {
            id: Uuid::new_v4(),
            max_duration,
            silence_duration,
            sample_rate,
        }
    }

    /// Create a new Status request
    pub fn new_status() -> Self {
        Request::Status { id: Uuid::new_v4() }
    }

    /// Create a new Stop request
    pub fn new_stop() -> Self {
        Request::Stop { id: Uuid::new_v4() }
    }

    /// Create a new Subscribe request
    pub fn new_subscribe() -> Self {
        Request::Subscribe { id: Uuid::new_v4() }
    }
}

/// Responses sent from server to client (in reply to requests)
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Response {
    /// Successful transcription result
    Result {
        id: Uuid,
        text: String,
        duration: f32,
        model: String,
    },
    /// Error response
    Error { id: Uuid, error: String },
    /// Status information
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
}

impl Response {
    /// Get the response ID
    pub fn id(&self) -> Uuid {
        match self {
            Response::Result { id, .. } => *id,
            Response::Error { id, .. } => *id,
            Response::Status { id, .. } => *id,
            Response::Subscribed { id } => *id,
        }
    }

    /// Create a Result response
    pub fn new_result(id: Uuid, text: String, duration: f32, model: String) -> Self {
        Response::Result {
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
        Response::Status {
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
        Response::Subscribed { id }
    }
}

/// Events broadcast from server to subscribers
/// These are wrapped in a Response with type="event" and the event data in the data field
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    /// State change event
    State {
        state: String,
        idle_hot: bool,
        ts: u64,
        #[serde(default = "default_version")]
        ver: u32,
    },
    /// Audio level event
    Level {
        v: f32,
        ts: u64,
        #[serde(default = "default_version")]
        ver: u32,
    },
    /// Spectrum data event
    Spectrum {
        bands: Vec<f32>,
        ts: u64,
        #[serde(default = "default_version")]
        ver: u32,
    },
    /// Combined status event
    Status {
        state: String,
        level: f32,
        idle_hot: bool,
        ts: u64,
        #[serde(default = "default_version")]
        ver: u32,
    },
}

fn default_version() -> u32 {
    1
}

impl Event {
    /// Create a State event
    pub fn new_state(state: String, idle_hot: bool, ts: u64) -> Self {
        Event::State {
            state,
            idle_hot,
            ts,
            ver: 1,
        }
    }

    /// Create a Spectrum event
    pub fn new_spectrum(bands: Vec<f32>, ts: u64) -> Self {
        Event::Spectrum { bands, ts, ver: 1 }
    }

    /// Create a Status event
    pub fn new_status(state: String, level: f32, idle_hot: bool, ts: u64) -> Self {
        Event::Status {
            state,
            level,
            idle_hot,
            ts,
            ver: 1,
        }
    }
}
