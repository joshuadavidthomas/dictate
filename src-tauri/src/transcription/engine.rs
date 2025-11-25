//! Runtime inference engine cache and loading logic.

use super::models::{self, ModelId};
use crate::conf::SettingsState;
use anyhow::{Result, anyhow};
use std::path::Path;
use transcribe_rs::{
    TranscriptionEngine as TranscribeTrait,
    engines::parakeet::{ParakeetEngine, ParakeetModelParams},
    engines::whisper::WhisperEngine,
};

/// Loaded transcription engine (Whisper or Parakeet) ready for inference.
pub enum LoadedEngine {
    Whisper { engine: WhisperEngine },
    Parakeet { engine: ParakeetEngine },
}

impl LoadedEngine {
    /// Transcribes an audio file to text.
    pub fn transcribe(&mut self, audio_path: &Path) -> Result<String> {
        println!("Transcribing audio file: {}", audio_path.display());

        match self {
            LoadedEngine::Whisper { engine } => match engine.transcribe_file(audio_path, None) {
                Ok(result) => {
                    let text = result.text;
                    println!("Transcription completed: {}", text);
                    Ok(text)
                }
                Err(e) => {
                    println!("Transcription failed: {}", e);
                    Err(anyhow!("Whisper transcription failed: {}", e))
                }
            },
            LoadedEngine::Parakeet { engine } => match engine.transcribe_file(audio_path, None) {
                Ok(result) => {
                    let text = result.text;
                    println!("Transcription completed: {}", text);
                    Ok(text)
                }
                Err(e) => {
                    println!("Transcription failed: {}", e);
                    Err(anyhow!("Parakeet transcription failed: {}", e))
                }
            },
        }
    }
}

/// Ensures a model is loaded in the cache, loading it if necessary.
///
/// Returns mutable references to the cached model ID and engine.
pub async fn ensure_loaded<'a>(
    cache: &'a mut Option<(ModelId, LoadedEngine)>,
    settings: &SettingsState,
) -> Result<(&'a ModelId, &'a mut LoadedEngine)> {
    let settings_data = settings.get().await;
    let descriptor = models::preferred_or_default(settings_data.preferred_model);
    let model_id = descriptor.id;
    let path = models::local_path(model_id)?;

    // Verify model is downloaded
    if !models::is_downloaded(model_id)? {
        return Err(anyhow!(
            "Model '{:?}' not downloaded. Please download it first.",
            model_id
        ));
    }

    // Load engine if cache is empty or ID changed
    let needs_load = !matches!(cache, Some((cached_id, _)) if *cached_id == model_id);

    if needs_load {
        println!("Loading transcription model from: {}", path.display());

        let engine = if descriptor.is_directory {
            // Parakeet model (directory-based)
            let mut parakeet_engine = ParakeetEngine::new();
            parakeet_engine
                .load_model_with_params(&path, ParakeetModelParams::int8())
                .map_err(|e| anyhow!("Failed to load Parakeet model: {}", e))?;
            LoadedEngine::Parakeet {
                engine: parakeet_engine,
            }
        } else {
            // Whisper model (file-based)
            let mut whisper_engine = WhisperEngine::new();
            whisper_engine.load_model(&path).map_err(|e| {
                let metadata = std::fs::metadata(&path).ok();
                let file_size = metadata.map(|m| m.len()).unwrap_or(0);

                if file_size < 1_000_000 {
                    anyhow!(
                        "Failed to load Whisper model (file may be corrupt, size: {} bytes): {}",
                        file_size,
                        e
                    )
                } else {
                    anyhow!("Failed to load Whisper model: {}", e)
                }
            })?;
            LoadedEngine::Whisper {
                engine: whisper_engine,
            }
        };

        println!("Model loaded successfully");
        *cache = Some((model_id, engine));
    }

    // Return references to the cached model
    let (id, engine) = cache.as_mut().unwrap();
    Ok((id, engine))
}
