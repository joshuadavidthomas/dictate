use crate::conf;
use anyhow::{Result, anyhow};
use flate2::read::GzDecoder;
use fs2::available_space;
use futures::{StreamExt, future};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tar::Archive;
use tokio::fs as async_fs;
use tokio::io::AsyncWriteExt;

/// Engine families for transcription models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelEngine {
    Whisper,
    Parakeet,
}

/// Whisper model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
    Small,
    Medium,
}

/// Parakeet model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParakeetModel {
    V2,
    V3,
}

/// Global identifier for all supported models.
///
/// This encodes the invariant that every model belongs to exactly one engine
/// and to a finite set of variants within that engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "engine", content = "id", rename_all = "lowercase")]
pub enum ModelId {
    Whisper(WhisperModel),
    Parakeet(ParakeetModel),
}

/// Static per-model metadata.
///
/// This captures the invariants for each model family: engine, download
/// location, storage layout (file vs directory), and display/storage name.
pub trait ModelSpec: Copy {
    fn engine(self) -> ModelEngine;
    fn storage_name(self) -> &'static str;
    fn is_directory(self) -> bool;
    fn download_url(self) -> Option<&'static str>;
}

impl ModelSpec for WhisperModel {
    fn engine(self) -> ModelEngine {
        ModelEngine::Whisper
    }

    fn storage_name(self) -> &'static str {
        match self {
            WhisperModel::Tiny => "whisper-tiny",
            WhisperModel::Base => "whisper-base",
            WhisperModel::Small => "whisper-small",
            WhisperModel::Medium => "whisper-medium",
        }
    }

    fn is_directory(self) -> bool {
        false
    }

    fn download_url(self) -> Option<&'static str> {
        Some(match self {
            WhisperModel::Tiny => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
            }
            WhisperModel::Base => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
            }
            WhisperModel::Small => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            WhisperModel::Medium => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
        })
    }
}

impl ModelSpec for ParakeetModel {
    fn engine(self) -> ModelEngine {
        ModelEngine::Parakeet
    }

    fn storage_name(self) -> &'static str {
        match self {
            ParakeetModel::V2 => "parakeet-v2",
            ParakeetModel::V3 => "parakeet-v3",
        }
    }

    fn is_directory(self) -> bool {
        true
    }

    fn download_url(self) -> Option<&'static str> {
        Some(match self {
            ParakeetModel::V2 => "https://blob.handy.computer/parakeet-v2-int8.tar.gz",
            ParakeetModel::V3 => "https://blob.handy.computer/parakeet-v3-int8.tar.gz",
        })
    }
}

impl ModelSpec for ModelId {
    fn engine(self) -> ModelEngine {
        match self {
            ModelId::Whisper(_) => ModelEngine::Whisper,
            ModelId::Parakeet(_) => ModelEngine::Parakeet,
        }
    }

    fn storage_name(self) -> &'static str {
        match self {
            ModelId::Whisper(m) => m.storage_name(),
            ModelId::Parakeet(m) => m.storage_name(),
        }
    }

    fn is_directory(self) -> bool {
        match self {
            ModelId::Whisper(m) => m.is_directory(),
            ModelId::Parakeet(m) => m.is_directory(),
        }
    }

    fn download_url(self) -> Option<&'static str> {
        match self {
            ModelId::Whisper(m) => m.download_url(),
            ModelId::Parakeet(m) => m.download_url(),
        }
    }
}

/// All supported models, used to construct the manager's state.
const ALL_MODELS: &[ModelId] = &[
    ModelId::Whisper(WhisperModel::Tiny),
    ModelId::Whisper(WhisperModel::Base),
    ModelId::Whisper(WhisperModel::Small),
    ModelId::Whisper(WhisperModel::Medium),
    ModelId::Parakeet(ParakeetModel::V2),
    ModelId::Parakeet(ParakeetModel::V3),
];

/// Dynamic runtime state for a single model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: ModelId,
    pub local_path: PathBuf,
}

impl ModelInfo {
    pub fn engine(&self) -> ModelEngine {
        self.id.engine()
    }

    pub fn storage_name(&self) -> &'static str {
        self.id.storage_name()
    }

    pub fn is_directory(&self) -> bool {
        self.id.is_directory()
    }

    pub fn is_downloaded(&self) -> bool {
        self.local_path.exists()
    }

    pub fn download_url(&self) -> Option<&'static str> {
        self.id.download_url()
    }
}

pub struct ModelManager {
    models_dir: PathBuf,
    available_models: HashMap<ModelId, ModelInfo>,
    client: reqwest::Client,
    cached_sizes: HashMap<ModelId, (u64, Instant)>,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        let models_dir = conf::get_project_dirs()?.data_dir().join("models");
        fs::create_dir_all(&models_dir)?;

        let mut manager = Self {
            models_dir,
            available_models: HashMap::new(),
            client: reqwest::Client::new(),
            cached_sizes: HashMap::new(),
        };

        manager.initialize_models()?;
        Ok(manager)
    }

    fn initialize_models(&mut self) -> Result<()> {
        for id in ALL_MODELS {
            let storage_name = id.storage_name();
            let model_path = if id.is_directory() {
                // Directory-based model (e.g., Parakeet)
                self.models_dir.join(storage_name)
            } else {
                // File-based model (e.g., Whisper)
                self.models_dir.join(format!("{storage_name}.bin"))
            };

            let info = ModelInfo {
                id: *id,
                local_path: model_path,
            };

            self.available_models.insert(*id, info);
        }

        Ok(())
    }

    pub fn list_available_models(&self) -> Vec<&ModelInfo> {
        self.available_models.values().collect()
    }

    pub fn get_model_info(&self, id: ModelId) -> Option<&ModelInfo> {
        self.available_models.get(&id)
    }

    pub fn get_model_path(&self, id: ModelId) -> Option<PathBuf> {
        self.available_models
            .get(&id)
            .map(|model| model.local_path.clone())
            .filter(|path| path.exists())
    }

    pub async fn get_all_model_sizes(&mut self) -> Result<HashMap<ModelId, u64>> {
        let cache_duration = Duration::from_secs(24 * 60 * 60); // 24 hours
        let now = Instant::now();
        let mut sizes = HashMap::new();
        let mut models_to_fetch = Vec::new();

        // Check cache first
        for id in self.available_models.keys() {
            if let Some((size, timestamp)) = self.cached_sizes.get(id)
                && now.duration_since(*timestamp) < cache_duration
            {
                sizes.insert(*id, *size);
                continue;
            }
            models_to_fetch.push(*id);
        }

        // Fetch missing sizes in parallel
        if !models_to_fetch.is_empty() {
            let fetch_futures: Vec<_> = models_to_fetch
                .iter()
                .map(|id| {
                    let client = self.client.clone();
                    let id = *id;
                    async move {
                        let size = Self::fetch_single_model_size(&client, id).await?;
                        Ok::<(ModelId, u64), anyhow::Error>((id, size))
                    }
                })
                .collect();

            let results = future::join_all(fetch_futures).await;

            for result in results {
                match result {
                    Ok((id, size)) => {
                        sizes.insert(id, size);
                        // Update cache
                        self.cached_sizes.insert(id, (size, now));
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to fetch model size: {}", e);
                    }
                }
            }
        }

        Ok(sizes)
    }

    async fn fetch_single_model_size(client: &reqwest::Client, id: ModelId) -> Result<u64> {
        if let Some(url) = id.download_url() {
            let response = client.head(url).send().await?;

            // Try content-length header first
            if let Some(size) = response.headers().get("content-length")
                && let Ok(size_str) = size.to_str()
                && let Ok(size) = size_str.parse::<u64>()
            {
                return Ok(size);
            }

            // Fall back to x-linked-size header (HuggingFace specific)
            if let Some(size) = response.headers().get("x-linked-size")
                && let Ok(size_str) = size.to_str()
                && let Ok(size) = size_str.parse::<u64>()
            {
                return Ok(size);
            }
        }

        Ok(0)
    }

    pub async fn download_model(
        &self,
        id: ModelId,
        broadcast: &crate::broadcast::BroadcastServer,
    ) -> Result<()> {
        let model_info = self
            .available_models
            .get(&id)
            .ok_or_else(|| anyhow!("Model '{:?}' not found", id))?;

        let engine = model_info.engine();
        let name = model_info.storage_name();

        if model_info.is_downloaded() {
            println!("Model '{}' is already downloaded", name);
            broadcast
                .model_download_progress(id, engine, 0, 0, "done")
                .await;
            return Ok(());
        }

        let output_path = &model_info.local_path;

        let url = model_info
            .download_url()
            .ok_or_else(|| anyhow!("Model '{}' has no download URL defined", name))?;

        if model_info.is_directory() {
            // Directory-based model (e.g., Parakeet) - download tar.gz and extract
            let temp_archive = self.models_dir.join(format!("{}.tar.gz", name));

            println!("Downloading model '{}'...", name);
            broadcast
                .model_download_progress(id, engine, 0, 0, "downloading")
                .await;
            self.download_file(url, &temp_archive, Some((id, engine, broadcast)))
                .await?;

            println!("Extracting archive...");
            broadcast
                .model_download_progress(id, engine, 0, 0, "extracting")
                .await;
            self.extract_tar_gz(&temp_archive, name).await?;

            // Clean up temporary archive
            async_fs::remove_file(&temp_archive).await?;

            println!("Model '{}' downloaded and extracted successfully", name);
        } else {
            // Single file download (e.g., Whisper models)
            if let Some(parent) = output_path.parent() {
                async_fs::create_dir_all(parent).await?;
            }
            broadcast
                .model_download_progress(id, engine, 0, 0, "downloading")
                .await;
            self.download_file(url, output_path, Some((id, engine, broadcast)))
                .await?;
            println!("Model '{}' downloaded successfully", name);
        }

        broadcast
            .model_download_progress(id, engine, 0, 0, "done")
            .await;

        Ok(())
    }

    pub async fn remove_model(&self, id: ModelId) -> Result<()> {
        let model_info = self
            .available_models
            .get(&id)
            .ok_or_else(|| anyhow!("Model '{:?}' not found", id))?;

        let name = model_info.storage_name();

        if !model_info.is_downloaded() {
            println!("Model '{}' is not downloaded", name);
            return Ok(());
        }

        let model_path = &model_info.local_path;

        if model_info.is_directory() {
            // Remove directory recursively
            async_fs::remove_dir_all(model_path).await?;
        } else {
            // Remove single file
            async_fs::remove_file(model_path).await?;
        }

        println!("Model '{}' removed successfully", name);
        Ok(())
    }

    async fn download_file(
        &self,
        url: &str,
        output_path: &Path,
        progress: Option<(ModelId, ModelEngine, &crate::broadcast::BroadcastServer)>,
    ) -> Result<()> {
        println!("Downloading to {}", output_path.display());

        if let Some(parent) = output_path.parent() {
            async_fs::create_dir_all(parent).await?;
        }

        let response = self.client.get(url).send().await?;
        let total_size = response.content_length().unwrap_or(0);

        // Check disk space
        if total_size > 0
            && let Some(parent) = output_path.parent()
            && let Ok(available) = available_space(parent)
        {
            let required_space = (total_size as f64 * 1.1) as u64;
            if available < required_space {
                return Err(anyhow!(
                    "Insufficient disk space. Need {} MB, available {} MB",
                    required_space / 1_000_000,
                    available / 1_000_000
                ));
            }
        }

        // Progress bar
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

            if let Some((id, engine, broadcast)) = progress {
                broadcast
                    .model_download_progress(id, engine, downloaded, total_size, "downloading")
                    .await;
            }
        }

        pb.finish_with_message("Download complete!");
        Ok(())
    }

    async fn extract_tar_gz(&self, archive_path: &Path, model_name: &str) -> Result<()> {
        let temp_extract_dir = self.models_dir.join(format!("{}.extracting", model_name));
        let final_model_dir = self.models_dir.join(model_name);

        // Clean up any previous incomplete extraction
        if temp_extract_dir.exists() {
            async_fs::remove_dir_all(&temp_extract_dir).await?;
        }

        // Create temporary extraction directory
        async_fs::create_dir_all(&temp_extract_dir).await?;

        // Extract in blocking task to avoid blocking async runtime
        let archive_path = archive_path.to_path_buf();
        let temp_dir = temp_extract_dir.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let tar_gz = std::fs::File::open(&archive_path)?;
            let tar = GzDecoder::new(tar_gz);
            let mut archive = Archive::new(tar);
            archive.unpack(&temp_dir)?;
            Ok(())
        })
        .await??;

        // Clean up Apple resource fork files (._* files)
        let cleaned = Self::clean_apple_resource_forks(&temp_extract_dir)?;
        if cleaned > 0 {
            println!("Removed {} Apple resource fork file(s)", cleaned);
        }

        // Find extracted directory (archive might have nested structure)
        let extracted_dirs: Vec<_> = fs::read_dir(&temp_extract_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
            .collect();

        if extracted_dirs.len() == 1 {
            // Single directory extracted - move it to final location
            let source_dir = extracted_dirs[0].path();
            if final_model_dir.exists() {
                async_fs::remove_dir_all(&final_model_dir).await?;
            }
            async_fs::rename(&source_dir, &final_model_dir).await?;
            // Clean up temp directory
            async_fs::remove_dir_all(&temp_extract_dir).await?;
        } else {
            // Multiple items or no directories - rename temp dir itself
            if final_model_dir.exists() {
                async_fs::remove_dir_all(&final_model_dir).await?;
            }
            async_fs::rename(&temp_extract_dir, &final_model_dir).await?;
        }

        Ok(())
    }

    /// Remove Apple resource fork files (._*) from extracted directory
    fn clean_apple_resource_forks(dir: &Path) -> Result<usize> {
        let mut removed_count = 0;

        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let file_name = entry.file_name();
                let name_str = file_name.to_string_lossy();

                if path.is_dir() {
                    // Recurse into subdirectories
                    removed_count += Self::clean_apple_resource_forks(&path)?;
                } else if name_str.starts_with("._") {
                    // Remove Apple resource fork file
                    fs::remove_file(&path)?;
                    removed_count += 1;
                }
            }
        }

        Ok(removed_count)
    }

    pub fn get_storage_info(&self) -> Result<StorageInfo> {
        let mut total_size = 0u64;
        let mut downloaded_count = 0;

        for model in self.available_models.values() {
            if model.is_downloaded() {
                let local_path = &model.local_path;

                if model.is_directory() {
                    // Calculate directory size recursively
                    if let Ok(size) = Self::calculate_dir_size(local_path) {
                        total_size += size;
                        downloaded_count += 1;
                    }
                } else {
                    // Single file
                    if let Ok(metadata) = fs::metadata(local_path) {
                        total_size += metadata.len();
                        downloaded_count += 1;
                    }
                }
            }
        }

        Ok(StorageInfo {
            models_dir: self.models_dir.clone(),
            total_size,
            downloaded_count,
            available_count: self.available_models.len(),
        })
    }

    fn calculate_dir_size(path: &Path) -> Result<u64> {
        let mut total = 0u64;

        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let metadata = entry.metadata()?;

                if metadata.is_dir() {
                    total += Self::calculate_dir_size(&entry.path())?;
                } else {
                    total += metadata.len();
                }
            }
        }

        Ok(total)
    }
}

#[derive(Debug)]
pub struct StorageInfo {
    pub models_dir: PathBuf,
    pub total_size: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            eprintln!("Failed to create ModelManager: {}", e);
            std::process::exit(1);
        })
    }
}
