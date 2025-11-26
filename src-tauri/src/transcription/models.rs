use crate::conf;
use anyhow::{Result, anyhow};
use flate2::read::GzDecoder;
use fs2::available_space;
use futures::future;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tar::Archive;
use tokio::fs as async_fs;
use tokio::io::AsyncWriteExt;
use transcribe_rs::TranscriptionEngine as TranscribeTrait;

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

/// A transcription model.
///
/// This encodes the invariant that every model belongs to exactly one engine family
/// (Whisper or Parakeet) and to a finite set of variants within that family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    Whisper(WhisperModel),
    Parakeet(ParakeetModel),
}

impl Serialize for Model {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Model", 2)?;
        match self {
            Model::Whisper(variant) => {
                state.serialize_field("engine", "whisper")?;
                state.serialize_field("id", variant)?;
            }
            Model::Parakeet(variant) => {
                state.serialize_field("engine", "parakeet")?;
                state.serialize_field("id", variant)?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for Model {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ModelHelper {
            engine: String,
            id: serde_json::Value,
        }

        let helper = ModelHelper::deserialize(deserializer)?;
        match helper.engine.as_str() {
            "whisper" => {
                let variant = WhisperModel::deserialize(helper.id)
                    .map_err(serde::de::Error::custom)?;
                Ok(Model::Whisper(variant))
            }
            "parakeet" => {
                let variant = ParakeetModel::deserialize(helper.id)
                    .map_err(serde::de::Error::custom)?;
                Ok(Model::Parakeet(variant))
            }
            engine => Err(serde::de::Error::unknown_variant(engine, &["whisper", "parakeet"])),
        }
    }
}

impl Model {
    /// Returns the storage name for this model.
    pub fn storage_name(self) -> &'static str {
        match self {
            Model::Whisper(WhisperModel::Tiny) => "whisper-tiny",
            Model::Whisper(WhisperModel::Base) => "whisper-base",
            Model::Whisper(WhisperModel::Small) => "whisper-small",
            Model::Whisper(WhisperModel::Medium) => "whisper-medium",
            Model::Parakeet(ParakeetModel::V2) => "parakeet-v2",
            Model::Parakeet(ParakeetModel::V3) => "parakeet-v3",
        }
    }

    /// Returns whether this model is stored as a directory (vs. single file).
    pub fn is_directory(self) -> bool {
        matches!(self, Model::Parakeet(_))
    }

    /// Returns the download URL for this model.
    pub fn download_url(self) -> &'static str {
        match self {
            Model::Whisper(WhisperModel::Tiny) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
            }
            Model::Whisper(WhisperModel::Base) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
            }
            Model::Whisper(WhisperModel::Small) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            Model::Whisper(WhisperModel::Medium) => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            Model::Parakeet(ParakeetModel::V2) => {
                "https://blob.handy.computer/parakeet-v2-int8.tar.gz"
            }
            Model::Parakeet(ParakeetModel::V3) => {
                "https://blob.handy.computer/parakeet-v3-int8.tar.gz"
            }
        }
    }

    /// Returns all supported models.
    pub fn all() -> &'static [Model] {
        &[
            Model::Whisper(WhisperModel::Tiny),
            Model::Whisper(WhisperModel::Base),
            Model::Whisper(WhisperModel::Small),
            Model::Whisper(WhisperModel::Medium),
            Model::Parakeet(ParakeetModel::V2),
            Model::Parakeet(ParakeetModel::V3),
        ]
    }

    /// Resolves the preferred model or falls back to defaults.
    ///
    /// Fallback order:
    /// 1. Preferred model (if provided)
    /// 2. Parakeet V3
    /// 3. Whisper Base
    pub fn preferred_or_default(pref: Option<Model>) -> Model {
        pref.unwrap_or(Model::Parakeet(ParakeetModel::V3))
    }

    /// Builds the local filesystem path for this model (file or directory) regardless of download state.
    pub fn local_path(self) -> Result<PathBuf> {
        let dir = models_dir()?;
        let path = if self.is_directory() {
            dir.join(self.storage_name())
        } else {
            dir.join(format!("{}.bin", self.storage_name()))
        };
        Ok(path)
    }

    /// Checks if this model exists on disk.
    pub fn is_downloaded(self) -> Result<bool> {
        let path = self.local_path()?;
        Ok(path.exists())
    }

    /// Downloads this model with progress reporting.
    pub async fn download(self, broadcast: &crate::broadcast::BroadcastServer) -> Result<()> {
        let output_path = self.local_path()?;
        let name = self.storage_name();

        if output_path.exists() {
            println!("Model '{}' is already downloaded", name);
            broadcast.model_download_progress(self, 0, 0, "done").await;
            return Ok(());
        }

        let url = self.download_url();
        let client = reqwest::Client::new();

        if self.is_directory() {
            // Directory-based model (e.g., Parakeet) - download tar.gz and extract
            let dir = models_dir()?;
            let temp_archive = dir.join(format!("{}.tar.gz", name));

            println!("Downloading model '{}'...", name);
            broadcast
                .model_download_progress(self, 0, 0, "downloading")
                .await;
            download_file(&client, url, &temp_archive, Some((self, broadcast))).await?;

            println!("Extracting archive...");
            broadcast
                .model_download_progress(self, 0, 0, "extracting")
                .await;
            extract_tar_gz(&temp_archive, name).await?;

            // Clean up temporary archive
            async_fs::remove_file(&temp_archive).await?;

            println!("Model '{}' downloaded and extracted successfully", name);
        } else {
            // Single file download (e.g., Whisper models)
            if let Some(parent) = output_path.parent() {
                async_fs::create_dir_all(parent).await?;
            }
            broadcast
                .model_download_progress(self, 0, 0, "downloading")
                .await;
            download_file(&client, url, &output_path, Some((self, broadcast))).await?;
            println!("Model '{}' downloaded successfully", name);
        }

        broadcast.model_download_progress(self, 0, 0, "done").await;

        Ok(())
    }

    /// Removes this model from disk.
    pub async fn remove(self) -> Result<()> {
        let model_path = self.local_path()?;
        let name = self.storage_name();

        if !model_path.exists() {
            println!("Model '{}' is not downloaded", name);
            return Ok(());
        }

        if self.is_directory() {
            async_fs::remove_dir_all(&model_path).await?;
        } else {
            async_fs::remove_file(&model_path).await?;
        }

        println!("Model '{}' removed successfully", name);
        Ok(())
    }

    /// Loads this model from disk into a runtime engine.
    pub fn load_engine(self) -> Result<LoadedEngine> {
        let path = self.local_path()?;

        if !self.is_downloaded()? {
            return Err(anyhow!(
                "Model '{:?}' not downloaded. Please download it first.",
                self
            ));
        }

        println!("Loading transcription model from: {}", path.display());

        let engine = match self {
            Model::Parakeet(_) => {
                // Parakeet model (directory-based)
                use transcribe_rs::engines::parakeet::{ParakeetEngine, ParakeetModelParams};
                let mut parakeet_engine = ParakeetEngine::new();
                parakeet_engine
                    .load_model_with_params(&path, ParakeetModelParams::int8())
                    .map_err(|e| anyhow!("Failed to load Parakeet model: {}", e))?;
                LoadedEngine::Parakeet {
                    engine: parakeet_engine,
                }
            }
            Model::Whisper(_) => {
                // Whisper model (file-based)
                use transcribe_rs::engines::whisper::WhisperEngine;
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
            }
        };

        println!("Model loaded successfully");
        Ok(engine)
    }
}

/// Runtime inference engine loaded into memory.
pub enum LoadedEngine {
    Whisper {
        engine: transcribe_rs::engines::whisper::WhisperEngine,
    },
    Parakeet {
        engine: transcribe_rs::engines::parakeet::ParakeetEngine,
    },
}

impl LoadedEngine {
    /// Transcribes an audio file to text.
    pub fn transcribe(&mut self, audio_path: &Path) -> Result<String> {
        println!("Transcribing audio file: {}", audio_path.display());

        match self {
            LoadedEngine::Whisper { engine } => {
                use transcribe_rs::TranscriptionEngine as TranscribeTrait;
                match engine.transcribe_file(audio_path, None) {
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
            LoadedEngine::Parakeet { engine } => {
                use transcribe_rs::TranscriptionEngine as TranscribeTrait;
                match engine.transcribe_file(audio_path, None) {
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
        }
    }
}

/// Ensures a model is loaded in the cache, loading it if necessary.
///
/// Returns mutable references to the cached model and engine.
pub async fn ensure_loaded<'a>(
    cache: &'a mut Option<(Model, LoadedEngine)>,
    settings: &crate::conf::SettingsState,
) -> Result<(&'a Model, &'a mut LoadedEngine)> {
    let settings_data = settings.get().await;
    let model = Model::preferred_or_default(settings_data.preferred_model);

    // Load engine if cache is empty or model changed
    let needs_load = !matches!(cache, Some((cached_model, _)) if *cached_model == model);

    if needs_load {
        let engine = model.load_engine()?;
        *cache = Some((model, engine));
    }

    // Return references to the cached model and engine
    let (model, engine) = cache.as_mut().unwrap();
    Ok((model, engine))
}

/// Information about model storage
#[derive(Debug, Serialize)]
pub struct StorageInfo {
    pub models_dir: PathBuf,
    pub total_size: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

/// Returns the models directory path, creating it if it doesn't exist
pub fn models_dir() -> Result<PathBuf> {
    let dir = conf::get_project_dirs()?.data_dir().join("models");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Computes total size and counts of downloaded models
pub fn storage_info() -> Result<StorageInfo> {
    let dir = models_dir()?;
    let mut total_size = 0u64;
    let mut downloaded_count = 0;
    let available_count = Model::all().len();

    for &id in Model::all() {
        let path = id.local_path()?;

        if !path.exists() {
            continue;
        }

        if id.is_directory() {
            // Calculate directory size recursively
            if let Ok(size) = calculate_dir_size(&path) {
                total_size += size;
                downloaded_count += 1;
            }
        } else if let Ok(metadata) = fs::metadata(&path) {
            total_size += metadata.len();
            downloaded_count += 1;
        }
    }

    Ok(StorageInfo {
        models_dir: dir,
        total_size,
        downloaded_count,
        available_count,
    })
}

/// Fetches sizes for all models from their download URLs
/// Results are cached for 24 hours to avoid unnecessary network requests
pub async fn get_all_model_sizes(
    client: &reqwest::Client,
    cache: &mut HashMap<Model, (u64, Instant)>,
) -> Result<HashMap<Model, u64>> {
    let cache_duration = Duration::from_secs(24 * 60 * 60); // 24 hours
    let now = Instant::now();
    let mut sizes = HashMap::new();
    let mut models_to_fetch = Vec::new();

    // Check cache first
    for &id in Model::all() {
        if let Some((size, timestamp)) = cache.get(&id)
            && now.duration_since(*timestamp) < cache_duration
        {
            sizes.insert(id, *size);
            continue;
        }
        models_to_fetch.push(id);
    }

    // Fetch missing sizes in parallel
    if !models_to_fetch.is_empty() {
        let fetch_futures: Vec<_> = models_to_fetch
            .iter()
            .map(|id| {
                let client = client.clone();
                let id = *id;
                async move {
                    let size = fetch_model_size(&client, id).await?;
                    Ok::<(Model, u64), anyhow::Error>((id, size))
                }
            })
            .collect();

        let results = future::join_all(fetch_futures).await;

        for result in results {
            match result {
                Ok((id, size)) => {
                    sizes.insert(id, size);
                    // Update cache
                    cache.insert(id, (size, now));
                }
                Err(e) => {
                    eprintln!("Warning: Failed to fetch model size: {}", e);
                }
            }
        }
    }

    Ok(sizes)
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    output_path: &Path,
    progress: Option<(Model, &crate::broadcast::BroadcastServer)>,
) -> Result<()> {
    println!("Downloading to {}", output_path.display());

    if let Some(parent) = output_path.parent() {
        async_fs::create_dir_all(parent).await?;
    }

    let response = client.get(url).send().await?;
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

    let mut stream = response.bytes_stream();
    let mut file = async_fs::File::create(output_path).await?;

    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if let Some((id, broadcast)) = progress {
            broadcast
                .model_download_progress(id, downloaded, total_size, "downloading")
                .await;
        }
    }

    Ok(())
}

async fn extract_tar_gz(archive_path: &Path, model_name: &str) -> Result<()> {
    let dir = models_dir()?;
    let temp_extract_dir = dir.join(format!("{}.extracting", model_name));
    let final_model_dir = dir.join(model_name);

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
    let cleaned = clean_apple_resource_forks(&temp_extract_dir)?;
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
                removed_count += clean_apple_resource_forks(&path)?;
            } else if name_str.starts_with("._") {
                // Remove Apple resource fork file
                fs::remove_file(&path)?;
                removed_count += 1;
            }
        }
    }

    Ok(removed_count)
}

fn calculate_dir_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;

            if metadata.is_dir() {
                total += calculate_dir_size(&entry.path())?;
            } else {
                total += metadata.len();
            }
        }
    }

    Ok(total)
}

async fn fetch_model_size(client: &reqwest::Client, id: Model) -> Result<u64> {
    let response = client.head(id.download_url()).send().await?;

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

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_models_count() {
        assert_eq!(Model::all().len(), 6);
    }

    #[test]
    fn test_model_storage_name() {
        let model = Model::Whisper(WhisperModel::Base);
        assert_eq!(model.storage_name(), "whisper-base");
    }

    #[test]
    fn test_preferred_or_default_with_valid_preference() {
        let model = Model::preferred_or_default(Some(Model::Whisper(WhisperModel::Small)));
        assert_eq!(model, Model::Whisper(WhisperModel::Small));
    }

    #[test]
    fn test_preferred_or_default_fallback_to_parakeet_v3() {
        let model = Model::preferred_or_default(None);
        assert_eq!(model, Model::Parakeet(ParakeetModel::V3));
    }

    #[test]
    fn test_whisper_models_are_files() {
        for &model in Model::all() {
            if matches!(model, Model::Whisper(_)) {
                assert!(!model.is_directory(), "{:?} should be a file", model);
            }
        }
    }

    #[test]
    fn test_parakeet_models_are_directories() {
        for &model in Model::all() {
            if matches!(model, Model::Parakeet(_)) {
                assert!(model.is_directory(), "{:?} should be a directory", model);
            }
        }
    }

    #[test]
    fn test_all_models_have_download_urls() {
        for &model in Model::all() {
            assert!(
                !model.download_url().is_empty(),
                "{:?} missing download URL",
                model
            );
        }
    }

}

    #[test]
    fn test_model_serialization() {
        let model = Model::Whisper(WhisperModel::Tiny);
        let json = serde_json::to_string(&model).unwrap();
        println!("Serialized: {}", json);
        assert!(json.contains("\"engine\""));
        assert!(json.contains("\"whisper\""));
        assert!(json.contains("\"id\""));
        assert!(json.contains("\"tiny\""));
    }
