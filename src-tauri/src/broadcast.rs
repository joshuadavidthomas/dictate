//! Channel-based broadcast for iced OSD
//!
//! Sends status updates to iced layer-shell overlay via tokio broadcast channel

use crate::protocol::{ServerMessage, State};
use crate::transport::encode_server_message;
use tokio::sync::broadcast;

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

    pub async fn broadcast_status(&self, state: State, spectrum: Option<Vec<f32>>, ts: u64) {
        eprintln!("[broadcast] Broadcasting status: {:?} (ts={})", state, ts);
        let msg = ServerMessage::new_status_event(state, spectrum, false, ts);
        if let Ok(json) = encode_server_message(&msg) {
            match self.tx.send(json) {
                Ok(n) => eprintln!("[broadcast] Sent to {} subscribers", n),
                Err(e) => eprintln!("[broadcast] Send failed (no subscribers): {}", e),
            }
        }
    }

    pub async fn broadcast_result(&self, text: String) {
        eprintln!("[broadcast] Broadcasting result: {}", text);
        let msg = ServerMessage::Result {
            id: uuid::Uuid::new_v4(),
            text,
            duration: 0.0,
            model: "parakeet-v3".into(),
        };
        if let Ok(json) = encode_server_message(&msg) {
            match self.tx.send(json) {
                Ok(n) => eprintln!("[broadcast] Sent result to {} subscribers", n),
                Err(e) => eprintln!("[broadcast] Send failed (no subscribers): {}", e),
            }
        }
    }
}
