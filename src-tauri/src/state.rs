use tokio::sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use serde::{Serialize, Deserialize};
use crate::audio::AudioRecorder;
use crate::models::ModelManager;
use crate::transcription::TranscriptionEngine;
use crate::broadcast::BroadcastServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Print,    // Print to stdout only
    Copy,     // Copy to clipboard
    Insert,   // Type at cursor position
}

pub struct AppState {
    pub recording_state: Mutex<RecordingState>,
    pub recorder: Arc<Mutex<Option<AudioRecorder>>>,
    pub engine: Arc<Mutex<Option<TranscriptionEngine>>>,
    pub model_manager: Arc<Mutex<Option<ModelManager>>>,
    pub current_recording: Mutex<Option<ActiveRecording>>,
    pub broadcast: BroadcastServer,
    pub start_time: Instant,
    pub output_mode: Mutex<OutputMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Transcribing,
}

pub struct ActiveRecording {
    pub audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
    pub stop_signal: Arc<AtomicBool>,
    pub stream: Option<cpal::Stream>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            recording_state: Mutex::new(RecordingState::Idle),
            recorder: Arc::new(Mutex::new(None)),
            engine: Arc::new(Mutex::new(None)),
            model_manager: Arc::new(Mutex::new(None)),
            current_recording: Mutex::new(None),
            broadcast: BroadcastServer::new(),
            start_time: Instant::now(),
            output_mode: Mutex::new(OutputMode::Print), // Default: safe mode
        }
    }
    
    /// Get monotonic timestamp in milliseconds since app start
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}
