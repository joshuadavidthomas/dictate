use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionHistory {
    pub id: i64,
    pub text: String,
    pub created_at: i64, // Unix timestamp in seconds
    pub duration_ms: Option<i64>,
    pub model_name: Option<String>,
    pub audio_path: Option<String>,
    pub output_mode: Option<String>,
    pub audio_size_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTranscription {
    pub text: String,
    pub duration_ms: Option<i64>,
    pub model_name: Option<String>,
    pub audio_path: Option<String>,
    pub output_mode: Option<String>,
    pub audio_size_bytes: Option<i64>,
}

impl NewTranscription {
    pub fn new(text: String) -> Self {
        Self {
            text,
            duration_ms: None,
            model_name: None,
            audio_path: None,
            output_mode: None,
            audio_size_bytes: None,
        }
    }
    
    pub fn with_duration(mut self, duration_ms: i64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
    
    pub fn with_model(mut self, model_name: String) -> Self {
        self.model_name = Some(model_name);
        self
    }
    
    pub fn with_audio_path(mut self, audio_path: String) -> Self {
        self.audio_path = Some(audio_path);
        self
    }
    
    pub fn with_output_mode(mut self, output_mode: String) -> Self {
        self.output_mode = Some(output_mode);
        self
    }
    
    pub fn with_audio_size(mut self, audio_size_bytes: i64) -> Self {
        self.audio_size_bytes = Some(audio_size_bytes);
        self
    }
}
