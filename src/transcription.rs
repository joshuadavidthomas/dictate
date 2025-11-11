use anyhow::Result;
use anyhow::anyhow;

use std::path::Path;
use transcribe_rs::{TranscriptionEngine as TranscribeTrait, engines::whisper::WhisperEngine};

pub struct TranscriptionEngine {
    whisper_engine: WhisperEngine,
    model_loaded: bool,
    model_path: Option<String>,
}

impl TranscriptionEngine {
    pub fn new() -> Self {
        Self {
            whisper_engine: WhisperEngine::new(),
            model_loaded: false,
            model_path: None,
        }
    }

    pub fn load_model(&mut self, model_path: &str) -> Result<()> {
        println!("Loading transcription model from: {}", model_path);

        let path = Path::new(model_path);

        // Check if model file exists
        if !path.exists() {
            return Err(anyhow!("Model file not found: {}", model_path));
        }

        // Try to load the Whisper model
        match self.whisper_engine.load_model(path) {
            Ok(_) => {
                self.model_loaded = true;
                self.model_path = Some(model_path.to_string());
                println!("Model loaded successfully");
                Ok(())
            }
            Err(e) => {
                // Check if file might be corrupted by examining size
                let metadata = std::fs::metadata(path).ok();
                let file_size = metadata.map(|m| m.len()).unwrap_or(0);

                if file_size < 1_000_000 {
                    Err(anyhow!(
                        "Failed to load Whisper model (file may be corrupt, size: {} bytes). Try re-downloading with: dictate models download <model>",
                        file_size
                    ))
                } else {
                    Err(anyhow!(
                        "Failed to load Whisper model: {}. The model file may be incompatible or corrupted. Try re-downloading.",
                        e
                    ))
                }
            }
        }
    }

    pub fn transcribe_file<P: AsRef<Path>>(&mut self, audio_path: P) -> Result<String> {
        if !self.model_loaded {
            return Err(anyhow!("No model loaded"));
        }

        println!("Transcribing audio file: {}", audio_path.as_ref().display());

        // Check if we're using placeholder mode
        if let Some(model_path) = &self.model_path {
            if model_path.starts_with("placeholder:") {
                println!("Using placeholder transcription (no real model loaded)");
                std::thread::sleep(std::time::Duration::from_millis(1000));
                let text = "This is a placeholder transcription from the audio file. Real Whisper transcription will work when model files are available.".to_string();
                println!("Transcription completed: {}", text);
                return Ok(text);
            }
        }

        // Transcribe the audio file with no additional parameters
        match self
            .whisper_engine
            .transcribe_file(audio_path.as_ref(), None)
        {
            Ok(result) => {
                let text = result.text;
                println!("Transcription completed: {}", text);
                Ok(text)
            }
            Err(e) => {
                println!("Transcription failed: {}", e);
                Err(anyhow!("Transcription failed: {}", e))
            }
        }
    }

    pub fn is_model_loaded(&self) -> bool {
        self.model_loaded
    }

    pub fn get_model_path(&self) -> Option<&str> {
        self.model_path.as_deref()
    }

    pub fn unload_model(&mut self) {
        if self.model_loaded {
            println!("Unloading transcription model");
            self.whisper_engine.unload_model();
            self.model_loaded = false;
            self.model_path = None;
        }
    }
}

impl Default for TranscriptionEngine {
    fn default() -> Self {
        Self::new()
    }
}
