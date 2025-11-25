//! Model management and transcription
//!
//! Everything related to ML models: downloading, storage, loading, and inference.
//! Merges what was previously models.rs + transcription.rs.

use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use flate2::read::GzDecoder;
use fs2::available_space;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::fs as async_fs;
use tokio::io::AsyncWriteExt;
use transcribe_rs::{
    TranscriptionEngine as TranscribeTrait,
    engines::parakeet::{ParakeetEngine, ParakeetModelParams},
    engines::whisper::WhisperEngine,
};

// ============================================================================
// Model Identification
// ============================================================================

/// Whisper model variants
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
    Small,
    Medium,
}

/// Parakeet model variants
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParakeetModel {
    V2,
    V3,
}

/// Unified model identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "engine", content = "id", rename_all = "lowercase")]
pub enum ModelId {
    Whisper(WhisperModel),
    Parakeet(ParakeetModel),
}

impl ModelId {
    /// All available models
    pub const ALL: &'static [ModelId] = &[
        ModelId::Whisper(WhisperModel::Tiny),
        ModelId::Whisper(WhisperModel::Base),
        ModelId::Whisper(WhisperModel::Small),
        ModelId::Whisper(WhisperModel::Medium),
        ModelId::Parakeet(ParakeetModel::V2),
        ModelId::Parakeet(ParakeetModel::V3),
    ];

    /// Storage name (used for file/directory naming)
    pub fn storage_name(self) -> &'static str {
        match self {
            ModelId::Whisper(WhisperModel::Tiny) => "whisper-tiny",
            ModelId::Whisper(WhisperModel::Base) => "whisper-base",
            ModelId::Whisper(WhisperModel::Small) => "whisper-small",
            ModelId::Whisper(WhisperModel::Medium) => "whisper-medium",
            ModelId::Parakeet(ParakeetModel::V2) => "parakeet-v2",
            ModelId::Parakeet(ParakeetModel::V3) => "parakeet-v3",
        }
    }

    /// Whether this model is stored as a directory (vs single file)
    pub fn is_directory(self) -> bool {
        matches!(self, ModelId::Parakeet(_))
    }

    /// Download URL
    pub fn download_url(self) -> &'static str {
        match self {
            ModelId::Whisper(WhisperModel::Tiny) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
            }
            ModelId::Whisper(WhisperModel::Base) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
            }
            ModelId::Whisper(WhisperModel::Small) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            ModelId::Whisper(WhisperModel::Medium) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            ModelId::Parakeet(ParakeetModel::V2) => {
                "https://blob.handy.computer/parakeet-v2-int8.tar.gz"
            }
            ModelId::Parakeet(ParakeetModel::V3) => {
                "https://blob.handy.computer/parakeet-v3-int8.tar.gz"
            }
        }
    }

    /// Engine type for this model
    pub fn engine_name(self) -> &'static str {
        match self {
            ModelId::Whisper(_) => "whisper",
            ModelId::Parakeet(_) => "parakeet",
        }
    }
}

// ============================================================================
// Model Manager
// ============================================================================

/// Model information for UI display
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: ModelId,
    pub is_downloaded: bool,
    pub local_path: PathBuf,
}

/// Storage usage information
#[derive(Debug, Clone, Serialize)]
pub struct StorageInfo {
    pub models_dir: PathBuf,
    pub total_size_bytes: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

/// Manages model storage and downloads
pub struct ModelManager {
    models_dir: PathBuf,
    client: reqwest::Client,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| anyhow!("Failed to get project directories"))?;

        let models_dir = project_dirs.data_dir().join("models");
        fs::create_dir_all(&models_dir)?;

        Ok(Self {
            models_dir,
            client: reqwest::Client::new(),
        })
    }

    /// Get the local path for a model (whether or not it's downloaded)
    pub fn model_path(&self, id: ModelId) -> PathBuf {
        if id.is_directory() {
            self.models_dir.join(id.storage_name())
        } else {
            self.models_dir.join(format!("{}.bin", id.storage_name()))
        }
    }

    /// Check if a model is downloaded
    pub fn is_downloaded(&self, id: ModelId) -> bool {
        self.model_path(id).exists()
    }

    /// Get path only if model is downloaded
    pub fn get_downloaded_path(&self, id: ModelId) -> Option<PathBuf> {
        let path = self.model_path(id);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// List all models with their status
    pub fn list_models(&self) -> Vec<ModelInfo> {
        ModelId::ALL
            .iter()
            .map(|&id| ModelInfo {
                id,
                is_downloaded: self.is_downloaded(id),
                local_path: self.model_path(id),
            })
            .collect()
    }

    /// Get storage usage information
    pub fn storage_info(&self) -> Result<StorageInfo> {
        let mut total_size = 0u64;
        let mut downloaded_count = 0;

        for &id in ModelId::ALL {
            let path = self.model_path(id);
            if path.exists() {
                downloaded_count += 1;
                total_size += if id.is_directory() {
                    dir_size(&path).unwrap_or(0)
                } else {
                    fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                };
            }
        }

        Ok(StorageInfo {
            models_dir: self.models_dir.clone(),
            total_size_bytes: total_size,
            downloaded_count,
            available_count: ModelId::ALL.len(),
        })
    }

    /// Download a model with progress reporting
    pub async fn download(
        &self,
        id: ModelId,
        on_progress: impl Fn(u64, u64, &str),
    ) -> Result<()> {
        if self.is_downloaded(id) {
            on_progress(0, 0, "done");
            return Ok(());
        }

        let url = id.download_url();
        on_progress(0, 0, "downloading");

        if id.is_directory() {
            // Download tar.gz and extract
            let archive_path = self.models_dir.join(format!("{}.tar.gz", id.storage_name()));
            self.download_file(url, &archive_path, &on_progress).await?;

            on_progress(0, 0, "extracting");
            self.extract_tar_gz(&archive_path, id.storage_name()).await?;

            async_fs::remove_file(&archive_path).await?;
        } else {
            // Direct file download
            let output_path = self.model_path(id);
            self.download_file(url, &output_path, &on_progress).await?;
        }

        on_progress(0, 0, "done");
        Ok(())
    }

    /// Remove a downloaded model
    pub async fn remove(&self, id: ModelId) -> Result<()> {
        let path = self.model_path(id);

        if !path.exists() {
            return Ok(());
        }

        if id.is_directory() {
            async_fs::remove_dir_all(&path).await?;
        } else {
            async_fs::remove_file(&path).await?;
        }

        Ok(())
    }

    /// Fetch download sizes for all models
    pub async fn fetch_model_sizes(&self) -> Result<HashMap<ModelId, u64>> {
        let mut sizes = HashMap::new();

        for &id in ModelId::ALL {
            if let Ok(size) = self.fetch_size(id.download_url()).await {
                sizes.insert(id, size);
            }
        }

        Ok(sizes)
    }

    async fn fetch_size(&self, url: &str) -> Result<u64> {
        let response = self.client.head(url).send().await?;

        // Try content-length, then x-linked-size (HuggingFace)
        for header in ["content-length", "x-linked-size"] {
            if let Some(value) = response.headers().get(header) {
                if let Ok(size) = value.to_str().unwrap_or("").parse::<u64>() {
                    return Ok(size);
                }
            }
        }

        Ok(0)
    }

    async fn download_file(
        &self,
        url: &str,
        output_path: &Path,
        on_progress: &impl Fn(u64, u64, &str),
    ) -> Result<()> {
        if let Some(parent) = output_path.parent() {
            async_fs::create_dir_all(parent).await?;
        }

        let response = self.client.get(url).send().await?;
        let total_size = response.content_length().unwrap_or(0);

        // Check disk space
        if total_size > 0 {
            if let Some(parent) = output_path.parent() {
                if let Ok(available) = available_space(parent) {
                    let required = (total_size as f64 * 1.1) as u64;
                    if available < required {
                        return Err(anyhow!(
                            "Insufficient disk space. Need {} MB, have {} MB",
                            required / 1_000_000,
                            available / 1_000_000
                        ));
                    }
                }
            }
        }

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap_or_else(|_| ProgressStyle::default_spinner())
                .progress_chars("#>-"),
        );

        let mut stream = response.bytes_stream();
        let mut file = async_fs::File::create(output_path).await?;
        let mut downloaded = 0u64;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
            on_progress(downloaded, total_size, "downloading");
        }

        pb.finish_with_message("Download complete!");
        Ok(())
    }

    async fn extract_tar_gz(&self, archive_path: &Path, model_name: &str) -> Result<()> {
        let temp_dir = self.models_dir.join(format!("{}.extracting", model_name));
        let final_dir = self.models_dir.join(model_name);

        // Clean up any previous incomplete extraction
        if temp_dir.exists() {
            async_fs::remove_dir_all(&temp_dir).await?;
        }

        async_fs::create_dir_all(&temp_dir).await?;

        // Extract in blocking task
        let archive = archive_path.to_path_buf();
        let temp = temp_dir.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let tar_gz = fs::File::open(&archive)?;
            let tar = GzDecoder::new(tar_gz);
            let mut archive = Archive::new(tar);
            archive.unpack(&temp)?;
            Ok(())
        })
        .await??;

        // Clean up Apple resource fork files
        clean_resource_forks(&temp_dir)?;

        // Handle nested directory structure
        let entries: Vec<_> = fs::read_dir(&temp_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .collect();

        if entries.len() == 1 {
            // Single directory - move its contents to final location
            let source = entries[0].path();
            if final_dir.exists() {
                async_fs::remove_dir_all(&final_dir).await?;
            }
            async_fs::rename(&source, &final_dir).await?;
            async_fs::remove_dir_all(&temp_dir).await?;
        } else {
            // Multiple items or flat structure - rename temp dir
            if final_dir.exists() {
                async_fs::remove_dir_all(&final_dir).await?;
            }
            async_fs::rename(&temp_dir, &final_dir).await?;
        }

        Ok(())
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            total += if meta.is_dir() {
                dir_size(&entry.path())?
            } else {
                meta.len()
            };
        }
    }
    Ok(total)
}

fn clean_resource_forks(dir: &Path) -> Result<usize> {
    let mut removed = 0;

    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();

            if path.is_dir() {
                removed += clean_resource_forks(&path)?;
            } else if name.to_string_lossy().starts_with("._") {
                fs::remove_file(&path)?;
                removed += 1;
            }
        }
    }

    Ok(removed)
}

// ============================================================================
// Transcription Engine
// ============================================================================

enum Backend {
    Whisper(WhisperEngine),
    Parakeet(ParakeetEngine),
}

/// Transcription engine that wraps Whisper or Parakeet
pub struct Engine {
    backend: Option<Backend>,
    loaded_model: Option<ModelId>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            backend: None,
            loaded_model: None,
        }
    }

    /// Load a model from path
    pub fn load(&mut self, id: ModelId, path: &Path) -> Result<()> {
        eprintln!("Loading model {:?} from: {}", id, path.display());

        if !path.exists() {
            return Err(anyhow!("Model path not found: {}", path.display()));
        }

        match id {
            ModelId::Parakeet(_) => {
                let mut engine = ParakeetEngine::new();
                engine
                    .load_model_with_params(&path.to_path_buf(), ParakeetModelParams::int8())
                    .map_err(|e| anyhow!("Failed to load Parakeet model: {}", e))?;
                self.backend = Some(Backend::Parakeet(engine));
            }
            ModelId::Whisper(_) => {
                let mut engine = WhisperEngine::new();
                engine
                    .load_model(path)
                    .map_err(|e| anyhow!("Failed to load Whisper model: {}", e))?;
                self.backend = Some(Backend::Whisper(engine));
            }
        }

        self.loaded_model = Some(id);
        eprintln!("Model loaded successfully");
        Ok(())
    }

    /// Transcribe an audio file
    pub fn transcribe(&mut self, audio_path: &Path) -> Result<String> {
        let backend = self
            .backend
            .as_mut()
            .ok_or_else(|| anyhow!("No model loaded"))?;

        eprintln!("Transcribing: {}", audio_path.display());

        let result = match backend {
            Backend::Whisper(engine) => engine
                .transcribe_file(audio_path, None)
                .map_err(|e| anyhow!("Whisper transcription failed: {}", e))?,
            Backend::Parakeet(engine) => engine
                .transcribe_file(audio_path, None)
                .map_err(|e| anyhow!("Parakeet transcription failed: {}", e))?,
        };

        eprintln!("Transcription complete: {}", result.text);
        Ok(result.text)
    }

    /// Check if a model is loaded
    pub fn is_loaded(&self) -> bool {
        self.backend.is_some()
    }

    /// Get the currently loaded model
    pub fn loaded_model(&self) -> Option<ModelId> {
        self.loaded_model
    }

    /// Unload the current model
    pub fn unload(&mut self) {
        if let Some(backend) = &mut self.backend {
            match backend {
                Backend::Whisper(engine) => engine.unload_model(),
                Backend::Parakeet(engine) => engine.unload_model(),
            }
        }
        self.backend = None;
        self.loaded_model = None;
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper: Find best available model
// ============================================================================

/// Find the best available model, respecting user preference
pub fn find_available_model(
    manager: &ModelManager,
    preferred: Option<ModelId>,
) -> Option<(ModelId, PathBuf)> {
    // Try preferred model first
    if let Some(id) = preferred {
        if let Some(path) = manager.get_downloaded_path(id) {
            return Some((id, path));
        }
    }

    // Fallback order: parakeet-v3, then whisper-base
    for id in [
        ModelId::Parakeet(ParakeetModel::V3),
        ModelId::Whisper(WhisperModel::Base),
    ] {
        if let Some(path) = manager.get_downloaded_path(id) {
            return Some((id, path));
        }
    }

    None
}
