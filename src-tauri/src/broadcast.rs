//! Channel-based broadcast for iced OSD
//!
//! Sends status updates to iced layer-shell overlay via tokio broadcast channel

use crate::recording::RecordingSnapshot;
use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::sync::broadcast;
use uuid::Uuid;

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
    pub async fn transcription_result(&self, text: String, duration_secs: f32, model: String) {
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
        self.send_message(Message::ConfigUpdate { osd_position })
            .await;
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
        self.send_message(Message::ModelDownloadProgress {
            id,
            engine,
            downloaded_bytes,
            total_bytes,
            phase: phase.into(),
        })
        .await;
    }

    async fn send_message(&self, msg: Message) {
        match self.tx.send(msg) {
            Ok(n) => eprintln!("[broadcast] Sent to {} subscribers", n),
            Err(e) => eprintln!("[broadcast] Send failed (no subscribers): {}", e),
        }
    }

    /// Drain all currently available messages from a receiver into a vector
    pub fn drain_messages(rx: &mut broadcast::Receiver<Message>) -> Vec<Message> {
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
            let event = match &msg {
                Message::StatusEvent { state, .. } => match state {
                    RecordingSnapshot::Recording => Some((
                        "recording-started",
                        serde_json::json!({ "state": "recording" }),
                    )),
                    RecordingSnapshot::Transcribing => Some((
                        "recording-stopped",
                        serde_json::json!({ "state": "transcribing" }),
                    )),
                    RecordingSnapshot::Idle => Some((
                        "transcription-complete",
                        serde_json::json!({ "state": "idle" }),
                    )),
                    RecordingSnapshot::Error => None,
                },
                Message::Result { text, .. } => {
                    Some(("transcription-result", serde_json::json!({ "text": text })))
                }
                Message::ModelDownloadProgress {
                    id,
                    engine,
                    downloaded_bytes,
                    total_bytes,
                    phase,
                } => Some((
                    "model-download-progress",
                    serde_json::json!({
                        "id": id,
                        "engine": engine,
                        "downloaded_bytes": downloaded_bytes,
                        "total_bytes": total_bytes,
                        "phase": phase
                    }),
                )),
                // ConfigUpdate and Error not needed by frontend
                _ => None,
            };

            if let Some((name, payload)) = event
                && let Err(e) = app_handle.emit(name, payload)
            {
                eprintln!("[events] Failed to emit {}: {}", name, e);
            }
        });
    }
}
