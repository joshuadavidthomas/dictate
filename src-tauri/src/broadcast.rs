//! Channel-based broadcast for iced OSD
//!
//! Sends status updates to iced layer-shell overlay via tokio broadcast channel

use crate::state::RecordingSnapshot;
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
    Error {
        id: Uuid,
        error: String,
    },
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
}

#[derive(Clone)]
pub struct BroadcastServer {
    tx: broadcast::Sender<String>,
}

impl BroadcastServer {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(100);
        eprintln!("[broadcast] Created broadcast channel");
        Self { tx }
    }

    /// Subscribe to broadcast events
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        eprintln!("[broadcast] New subscriber connected");
        self.tx.subscribe()
    }

    /// Send a message to all subscribers
    pub async fn send(&self, msg: &Message) {
        if let Ok(mut json) = serde_json::to_string(msg) {
            json.push('\n');
            match self.tx.send(json) {
                Ok(n) => eprintln!("[broadcast] Sent to {} subscribers", n),
                Err(e) => eprintln!("[broadcast] Send failed (no subscribers): {}", e),
            }
        }
    }

    /// Spawn a bridge task that forwards broadcast messages to Tauri events
    /// 
    /// This enables the Svelte frontend (and other Tauri consumers) to receive
    /// the same events that the Iced OSD and other broadcast subscribers receive.
    pub fn spawn_tauri_bridge(&self, app: tauri::AppHandle) {
        let mut rx = self.subscribe();
        
        tokio::spawn(async move {
            eprintln!("[broadcast] Tauri event bridge started");
            
            while let Ok(json) = rx.recv().await {
                if let Ok(msg) = serde_json::from_str::<Message>(&json) {
                    match msg {
                        Message::StatusEvent { state, .. } => {
                            match state {
                                RecordingSnapshot::Recording => {
                                    app.emit("recording-started", 
                                        serde_json::json!({ "state": "recording" })
                                    ).ok();
                                }
                                RecordingSnapshot::Transcribing => {
                                    app.emit("recording-stopped",
                                        serde_json::json!({ "state": "transcribing" })
                                    ).ok();
                                }
                                RecordingSnapshot::Idle => {
                                    app.emit("transcription-complete",
                                        serde_json::json!({ "state": "idle" })
                                    ).ok();
                                }
                                _ => {}
                            }
                        }
                        Message::Result { text, .. } => {
                            app.emit("transcription-result",
                                serde_json::json!({ "text": text })
                            ).ok();
                        }
                        _ => {} // ConfigUpdate, Error not needed by frontend currently
                    }
                }
            }
            
            eprintln!("[broadcast] Tauri event bridge ended");
        });
    }
}
