//! Channel-based broadcast for iced OSD
//!
//! Sends status updates to iced layer-shell overlay via tokio broadcast channel

use crate::state::RecordingSnapshot;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Type-safe events emitted to Tauri frontend
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TauriEvent {
    RecordingStarted { state: String },
    RecordingStopped { state: String },
    TranscriptionComplete { state: String },
    TranscriptionResult { text: String },
    ModelDownloadProgress {
        id: crate::models::ModelId,
        engine: crate::models::ModelEngine,
        downloaded_bytes: u64,
        total_bytes: u64,
        phase: String,
    },
}
impl TauriEvent {
    /// Get the event name for Tauri's emit API
    pub fn name(&self) -> &'static str {
        match self {
            Self::RecordingStarted { .. } => "recording-started",
            Self::RecordingStopped { .. } => "recording-stopped",
            Self::TranscriptionComplete { .. } => "transcription-complete",
            Self::TranscriptionResult { .. } => "transcription-result",
            Self::ModelDownloadProgress { .. } => "model-download-progress",
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
                RecordingSnapshot::Recording => Some(TauriEvent::RecordingStarted {
                    state: "recording".into(),
                }),
                RecordingSnapshot::Transcribing => Some(TauriEvent::RecordingStopped {
                    state: "transcribing".into(),
                }),
                RecordingSnapshot::Idle => Some(TauriEvent::TranscriptionComplete {
                    state: "idle".into(),
                }),
                RecordingSnapshot::Error => None,
            },
            Message::Result { text, .. } => {
                Some(TauriEvent::TranscriptionResult { text: text.clone() })
            }
            Message::ModelDownloadProgress {
                id,
                engine,
                downloaded_bytes,
                total_bytes,
                phase,
            } => Some(TauriEvent::ModelDownloadProgress {
                id: *id,
                engine: *engine,
                downloaded_bytes: *downloaded_bytes,
                total_bytes: *total_bytes,
                phase: phase.clone(),
            }),
            // ConfigUpdate and Error not needed by frontend
            _ => None,
        }
    }
}

/// Messages broadcast over channels to subscribers (OSD, etc.)
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Message {
    /// Transcription result
    Result {
        id: Uuid,
        text: String,
        duration: f32,
        model: String,
    },
    /// Error message
    Error { id: Uuid, error: String },
    /// Status update event
    #[serde(rename = "status_event")]
    StatusEvent {
        state: RecordingSnapshot,
        spectrum: Option<Vec<f32>>,
        idle_hot: bool,
        ts: u64,
    },
    /// Configuration update
    #[serde(rename = "config_update")]
    ConfigUpdate {
        osd_position: crate::conf::OsdPosition,
    },
    /// Model download progress
    #[serde(rename = "model_download_progress")]
    ModelDownloadProgress {
        id: crate::models::ModelId,
        engine: crate::models::ModelEngine,
        downloaded_bytes: u64,
        total_bytes: u64,
        phase: String,
    },
}

#[derive(Clone)]
pub struct BroadcastServer {
    tx: broadcast::Sender<Message>,
}

impl BroadcastServer {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(100);
        eprintln!("[broadcast] Created broadcast channel");
        Self { tx }
    }

    /// Subscribe to broadcast events
    pub fn subscribe(&self) -> broadcast::Receiver<Message> {
        eprintln!("[broadcast] New subscriber connected");
        self.tx.subscribe()
    }

    /// Broadcast a recording-related status update
    pub async fn recording_status(
        &self,
        state: RecordingSnapshot,
        spectrum: Option<Vec<f32>>,
        idle_hot: bool,
        ts: u64,
    ) {
        self.send_message(Message::StatusEvent {
            state,
            spectrum,
            idle_hot,
            ts,
        })
        .await;
    }

    /// Broadcast a completed transcription result
    pub async fn transcription_result(
        &self,
        text: String,
        duration_secs: f32,
        model: String,
    ) {
        self.send_message(Message::Result {
            id: Uuid::new_v4(),
            text,
            duration: duration_secs,
            model,
        })
        .await;
    }

    /// Broadcast an OSD configuration update (position change, etc.)
    pub async fn osd_position_updated(&self, osd_position: crate::conf::OsdPosition) {
        self.send_message(Message::ConfigUpdate { osd_position }).await;
    }

    /// Broadcast model download progress
    pub async fn model_download_progress(
        &self,
        id: crate::models::ModelId,
        engine: crate::models::ModelEngine,
        downloaded_bytes: u64,
        total_bytes: u64,
        phase: impl Into<String>,
    ) {
        self
            .send_message(Message::ModelDownloadProgress {
                id,
                engine,
                downloaded_bytes,
                total_bytes,
                phase: phase.into(),
            })
            .await;
    }
 
    /// Broadcast an error message
    pub async fn error(&self, id: Uuid, error: impl Into<String>) {
        self.send_message(Message::Error {
            id,
            error: error.into(),
        })
        .await;
    }
 
    /// Internal helper: send a message to all subscribers

    async fn send_message(&self, msg: Message) {
        match self.tx.send(msg) {
            Ok(n) => eprintln!("[broadcast] Sent to {} subscribers", n),
            Err(e) => eprintln!("[broadcast] Send failed (no subscribers): {}", e),
        }
    }

    /// Drain all currently available messages from a receiver into a vector
    pub fn drain_messages(
        rx: &mut broadcast::Receiver<Message>,
    ) -> Vec<Message> {
        use tokio::sync::broadcast::error::TryRecvError;

        let mut messages = Vec::new();

        loop {
            match rx.try_recv() {
                Ok(msg) => messages.push(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Closed) => {
                    eprintln!("[broadcast] Receiver closed");
                    break;
                }
                Err(TryRecvError::Lagged(n)) => {
                    eprintln!("[broadcast] Lagged by {} messages", n);
                    // Continue draining newer messages
                }
            }
        }

        messages
    }

    /// Spawn a background consumer that handles messages as they arrive
    pub fn spawn_consumer<F>(&self, mut handler: F)
    where
        F: FnMut(Message) + Send + 'static,
    {
        let mut rx = self.subscribe();

        tauri::async_runtime::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(msg) => handler(msg),
                    Err(e) => {
                        eprintln!("[broadcast] Consumer recv error: {}", e);
                        break;
                    }
                }
            }
        });
    }

    /// Spawn a bridge task that forwards broadcast messages to Tauri events
    ///
    /// This enables the Svelte frontend (and other Tauri consumers) to receive
    /// the same events that the Iced OSD and other broadcast subscribers receive.
    pub fn spawn_tauri_bridge(&self, app: tauri::AppHandle) {
        let app_handle = app.clone();

        self.spawn_consumer(move |msg| {
            if let Some(event) = TauriEvent::from_message(&msg) {
                event.emit(&app_handle);
            }
        });
    }
}
