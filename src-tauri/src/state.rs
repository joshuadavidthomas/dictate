use crate::conf::Settings;
use crate::models::ModelManager;
use crate::transcription::TranscriptionEngine;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingState {
    Idle,
    Recording,
    Transcribing,
}

pub struct ActiveRecording {
    pub audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
    pub stop_signal: Arc<AtomicBool>,
    pub stream: Option<cpal::Stream>,
    pub start_time: Instant,
}

/// Manages the current recording session state
#[derive(Clone)]
pub struct RecordingSession {
    state: Arc<Mutex<RecordingState>>,
    current_recording: Arc<Mutex<Option<ActiveRecording>>>,
}

impl RecordingSession {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            current_recording: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn get_state(&self) -> RecordingState {
        *self.state.lock().await
    }

    pub async fn set_state(&self, new_state: RecordingState) {
        *self.state.lock().await = new_state;
    }

    pub fn current_recording(&self) -> &Arc<Mutex<Option<ActiveRecording>>> {
        &self.current_recording
    }

    /// Get elapsed recording time in milliseconds
    pub async fn elapsed_ms(&self) -> u64 {
        let recording = self.current_recording.lock().await;
        if let Some(rec) = recording.as_ref() {
            rec.start_time.elapsed().as_millis() as u64
        } else {
            0
        }
    }
}

/// Manages transcription engine and model state
#[derive(Clone)]
pub struct TranscriptionState {
    engine: Arc<Mutex<Option<TranscriptionEngine>>>,
    model_manager: Arc<Mutex<Option<ModelManager>>>,
}

impl TranscriptionState {
    pub fn new() -> Self {
        Self {
            engine: Arc::new(Mutex::new(None)),
            model_manager: Arc::new(Mutex::new(None)),
        }
    }

    pub fn engine(&self) -> &Arc<Mutex<Option<TranscriptionEngine>>> {
        &self.engine
    }

    pub fn model_manager(&self) -> &Arc<Mutex<Option<ModelManager>>> {
        &self.model_manager
    }
}
