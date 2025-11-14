use tokio::sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime};
use serde::{Serialize, Deserialize};
use sqlx::SqlitePool;
use crate::audio::AudioRecorder;
use crate::conf::Settings;
use crate::models::ModelManager;
use crate::transcription::TranscriptionEngine;
use crate::broadcast::BroadcastServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
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
    pub settings: Arc<Mutex<Settings>>,
    pub config_mtime: Arc<Mutex<Option<SystemTime>>>,
    pub db_pool: Arc<Mutex<Option<SqlitePool>>>,
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
    pub start_time: Instant,
}

impl AppState {
    pub fn new() -> Self {
        // Load settings from config file
        let settings = Settings::load();
        let output_mode = settings.output_mode;
        
        // Get initial config file mtime
        let config_mtime = crate::conf::config_mtime().ok();
        
        Self {
            recording_state: Mutex::new(RecordingState::Idle),
            recorder: Arc::new(Mutex::new(None)),
            engine: Arc::new(Mutex::new(None)),
            model_manager: Arc::new(Mutex::new(None)),
            current_recording: Mutex::new(None),
            broadcast: BroadcastServer::new(),
            start_time: Instant::now(),
            output_mode: Mutex::new(output_mode),
            settings: Arc::new(Mutex::new(settings)),
            config_mtime: Arc::new(Mutex::new(config_mtime)),
            db_pool: Arc::new(Mutex::new(None)),
        }
    }
    
    /// Get monotonic timestamp in milliseconds since app start
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}
