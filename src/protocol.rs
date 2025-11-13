use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Application state enumeration
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl State {
    /// Convert to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            State::Idle => "Idle",
            State::Recording => "Recording",
            State::Transcribing => "Transcribing",
            State::Error => "Error",
        }
    }

    /// Get UI-friendly label for display
    pub fn ui_label(&self) -> &'static str {
        match self {
            State::Idle => "Ready",
            State::Recording => "Recording",
            State::Transcribing => "Transcribing",
            State::Error => "Error",
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

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
        state: State,
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
        state: State,
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
    pub fn new_state(state: State, idle_hot: bool, ts: u64) -> Self {
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
    pub fn new_status(state: State, level: f32, idle_hot: bool, ts: u64) -> Self {
        Event::Status {
            state,
            level,
            idle_hot,
            ts,
            ver: 1,
        }
    }
}

/// Wire protocol message - represents anything that can be sent over the socket
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Message {
    /// Response to a client request
    Response(Response),
    /// Event broadcast to subscribers
    Event(Event),
}

impl Message {
    /// Create a Message from a Response
    pub fn from_response(response: Response) -> Self {
        Message::Response(response)
    }

    /// Create a Message from an Event
    pub fn from_event(event: Event) -> Self {
        Message::Event(event)
    }

    /// Check if this is a response
    pub fn is_response(&self) -> bool {
        matches!(self, Message::Response(_))
    }

    /// Check if this is an event
    pub fn is_event(&self) -> bool {
        matches!(self, Message::Event(_))
    }

    /// Get the response if this is a Response variant
    pub fn as_response(&self) -> Option<&Response> {
        match self {
            Message::Response(r) => Some(r),
            _ => None,
        }
    }

    /// Get the event if this is an Event variant
    pub fn as_event(&self) -> Option<&Event> {
        match self {
            Message::Event(e) => Some(e),
            _ => None,
        }
    }

    /// Convert into response if this is a Response variant
    pub fn into_response(self) -> Option<Response> {
        match self {
            Message::Response(r) => Some(r),
            _ => None,
        }
    }

    /// Convert into event if this is an Event variant
    pub fn into_event(self) -> Option<Event> {
        match self {
            Message::Event(e) => Some(e),
            _ => None,
        }
    }
}
