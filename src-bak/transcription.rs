use anyhow::Result;
use anyhow::anyhow;

use std::path::Path;
use transcribe_rs::{
    TranscriptionEngine as TranscribeTrait,
    engines::whisper::WhisperEngine,
    engines::parakeet::{ParakeetEngine, ParakeetModelParams},
};

enum TranscriptionBackend {
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
}

pub struct TranscriptionEngine {
    backend: Option<TranscriptionBackend>,
    model_loaded: bool,
    model_path: Option<String>,
}

impl TranscriptionEngine {
    pub fn new() -> Self {
        Self {
            backend: None,
            model_loaded: false,
            model_path: None,
        }
    }

    pub fn load_model(&mut self, model_path: &str) -> Result<()> {
        println!("Loading transcription model from: {}", model_path);

        let path = Path::new(model_path);

        // Check if model file/directory exists
        if !path.exists() {
            return Err(anyhow!("Model path not found: {}", model_path));
        }

        // Detect engine type based on path
        let is_directory = path.is_dir();
        
        if is_directory {
            // Load as Parakeet model (directory-based)
            // Use int8 quantized models for faster inference
            let mut parakeet_engine = ParakeetEngine::new();
            match parakeet_engine.load_model_with_params(&path.to_path_buf(), ParakeetModelParams::int8()) {
                Ok(_) => {
                    self.backend = Some(TranscriptionBackend::Parakeet(parakeet_engine));
                    self.model_loaded = true;
                    self.model_path = Some(model_path.to_string());
                    println!("Parakeet model loaded successfully");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("DEBUG: Raw Parakeet error: {:?}", e);
                    Err(anyhow!("Failed to load Parakeet model: {}", e))
                }
            }
        } else {
            // Load as Whisper model (file-based)
            let mut whisper_engine = WhisperEngine::new();
            match whisper_engine.load_model(path) {
                Ok(_) => {
                    self.backend = Some(TranscriptionBackend::Whisper(whisper_engine));
                    self.model_loaded = true;
                    self.model_path = Some(model_path.to_string());
                    println!("Whisper model loaded successfully");
                    Ok(())
                }
                Err(e) => {
                    // Check if file might be corrupted by examining size
                    let metadata = std::fs::metadata(path).ok();
                    let file_size = metadata.map(|m| m.len()).unwrap_or(0);

                    if file_size < 1_000_000 {
                        Err(anyhow!(
                            "Failed to load Whisper model (file may be corrupt, size: {} bytes): {}",
                            file_size, e
                        ))
                    } else {
                        Err(anyhow!("Failed to load Whisper model: {}", e))
                    }
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
                let text = "This is a placeholder transcription from the audio file. Real transcription will work when model files are available.".to_string();
                println!("Transcription completed: {}", text);
                return Ok(text);
            }
        }

        // Dispatch to the appropriate backend
        match &mut self.backend {
            Some(TranscriptionBackend::Whisper(engine)) => {
                match engine.transcribe_file(audio_path.as_ref(), None) {
                    Ok(result) => {
                        let text = result.text;
                        println!("Transcription completed: {}", text);
                        Ok(text)
                    }
                    Err(e) => {
                        println!("Transcription failed: {}", e);
                        Err(anyhow!("Whisper transcription failed: {}", e))
                    }
                }
            }
            Some(TranscriptionBackend::Parakeet(engine)) => {
                match engine.transcribe_file(audio_path.as_ref(), None) {
                    Ok(result) => {
                        let text = result.text;
                        println!("Transcription completed: {}", text);
                        Ok(text)
                    }
                    Err(e) => {
                        println!("Transcription failed: {}", e);
                        Err(anyhow!("Parakeet transcription failed: {}", e))
                    }
                }
            }
            None => {
                Err(anyhow!("No transcription backend initialized"))
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
            if let Some(backend) = &mut self.backend {
                match backend {
                    TranscriptionBackend::Whisper(engine) => engine.unload_model(),
                    TranscriptionBackend::Parakeet(engine) => engine.unload_model(),
                }
            }
            self.backend = None;
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
