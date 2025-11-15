use crate::broadcast::Message;
use crate::state::RecordingSnapshot;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Type-safe events emitted to Tauri frontend
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TauriEvent {
    RecordingStarted { state: String },
    RecordingStopped { state: String },
    TranscriptionComplete { state: String },
    TranscriptionResult { text: String },
}

impl TauriEvent {
    /// Get the event name for Tauri's emit API
    pub fn name(&self) -> &'static str {
        match self {
            Self::RecordingStarted { .. } => "recording-started",
            Self::RecordingStopped { .. } => "recording-stopped",
            Self::TranscriptionComplete { .. } => "transcription-complete",
            Self::TranscriptionResult { .. } => "transcription-result",
        }
    }
    
    /// Emit this event to the Tauri frontend
    pub fn emit(&self, app: &AppHandle) {
        if let Err(e) = app.emit(self.name(), self) {
            eprintln!("[events] Failed to emit {}: {}", self.name(), e);
        }
    }
}

impl TauriEvent {
    /// Convert broadcast messages to Tauri events (if applicable)
    pub fn from_message(msg: &Message) -> Option<Self> {
        match msg {
            Message::StatusEvent { state, .. } => match state {
                RecordingSnapshot::Recording => {
                    Some(TauriEvent::RecordingStarted {
                        state: "recording".into(),
                    })
                }
                RecordingSnapshot::Transcribing => {
                    Some(TauriEvent::RecordingStopped {
                        state: "transcribing".into(),
                    })
                }
                RecordingSnapshot::Idle => {
                    Some(TauriEvent::TranscriptionComplete {
                        state: "idle".into(),
                    })
                }
                RecordingSnapshot::Error => None,
            },
            Message::Result { text, .. } => Some(TauriEvent::TranscriptionResult {
                text: text.clone(),
            }),
            // ConfigUpdate and Error not needed by frontend
            _ => None,
        }
    }
}
