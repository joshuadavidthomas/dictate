//! Model catalog, types, and storage operations for transcription models.

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

// ============================================================================
// Types
// ============================================================================

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

impl ModelId {
    /// Returns the engine family for this model.
    pub fn engine(self) -> ModelEngine {
        match self {
            ModelId::Whisper(_) => ModelEngine::Whisper,
            ModelId::Parakeet(_) => ModelEngine::Parakeet,
        }
    }
}

/// Static metadata for a single model.
///
/// Contains all immutable properties needed to locate, download, and identify
/// a model on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: ModelId,
    pub storage_name: &'static str,
    pub is_directory: bool,
    pub download_url: &'static str,
}

/// Information about model storage
#[derive(Debug)]
pub struct StorageInfo {
    pub models_dir: PathBuf,
    pub total_size: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

// ============================================================================
// Catalog
// ============================================================================

/// All supported models in the catalog.
const ALL_MODELS: &[ModelDescriptor] = &[
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Tiny),
        storage_name: "whisper-tiny",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
    },
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Base),
        storage_name: "whisper-base",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    },
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Small),
        storage_name: "whisper-small",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    },
    ModelDescriptor {
        id: ModelId::Whisper(WhisperModel::Medium),
        storage_name: "whisper-medium",
        is_directory: false,
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
    },
    ModelDescriptor {
        id: ModelId::Parakeet(ParakeetModel::V2),
        storage_name: "parakeet-v2",
        is_directory: true,
        download_url: "https://blob.handy.computer/parakeet-v2-int8.tar.gz",
    },
    ModelDescriptor {
        id: ModelId::Parakeet(ParakeetModel::V3),
        storage_name: "parakeet-v3",
        is_directory: true,
        download_url: "https://blob.handy.computer/parakeet-v3-int8.tar.gz",
    },
];

/// Returns the complete catalog of all supported models.
pub fn all_models() -> &'static [ModelDescriptor] {
    ALL_MODELS
}

/// Looks up a model descriptor by ID.
///
/// Returns `None` if the model is not in the catalog.
pub fn find(id: ModelId) -> Option<&'static ModelDescriptor> {
    ALL_MODELS.iter().find(|desc| desc.id == id)
}

/// Resolves the preferred model or falls back to defaults.
///
/// Fallback order:
/// 1. Preferred model (if provided and exists in catalog)
/// 2. Parakeet V3
/// 3. Whisper Base
pub fn preferred_or_default(pref: Option<ModelId>) -> &'static ModelDescriptor {
    // Try preferred model first
    if let Some(pref_id) = pref
        && let Some(desc) = find(pref_id)
    {
        return desc;
    }

    // Fall back to Parakeet V3
    if let Some(desc) = find(ModelId::Parakeet(ParakeetModel::V3)) {
        return desc;
    }

    // Final fallback to Whisper Base
    find(ModelId::Whisper(WhisperModel::Base))
        .expect("Whisper Base must exist in catalog as final fallback")
}

// ============================================================================
// Storage
// ============================================================================

/// Returns the models directory path, creating it if it doesn't exist
pub fn models_dir() -> Result<PathBuf> {
    let dir = conf::get_project_dirs()?.data_dir().join("models");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Builds the local filesystem path for a model (file or directory) regardless of download state
pub fn local_path(id: ModelId) -> Result<PathBuf> {
    let dir = models_dir()?;
    let desc = find(id).ok_or_else(|| anyhow!("Model '{:?}' not found in catalog", id))?;

    let path = if desc.is_directory {
        // Directory-based model (e.g., Parakeet)
        dir.join(desc.storage_name)
    } else {
        // File-based model (e.g., Whisper)
        dir.join(format!("{}.bin", desc.storage_name))
    };

    Ok(path)
}

/// Checks if a model exists on disk
pub fn is_downloaded(id: ModelId) -> Result<bool> {
    let path = local_path(id)?;
    Ok(path.exists())
}

/// Downloads a model with progress reporting
pub async fn download(id: ModelId, broadcast: &crate::broadcast::BroadcastServer) -> Result<()> {
    let desc = find(id).ok_or_else(|| anyhow!("Model '{:?}' not found in catalog", id))?;

    let output_path = local_path(id)?;
    let engine = id.engine();
    let name = desc.storage_name;

    if output_path.exists() {
        println!("Model '{}' is already downloaded", name);
        broadcast
            .model_download_progress(id, engine, 0, 0, "done")
            .await;
        return Ok(());
    }

    let url = desc.download_url;
    let client = reqwest::Client::new();

    if desc.is_directory {
        // Directory-based model (e.g., Parakeet) - download tar.gz and extract
        let dir = models_dir()?;
        let temp_archive = dir.join(format!("{}.tar.gz", name));

        println!("Downloading model '{}'...", name);
        broadcast
            .model_download_progress(id, engine, 0, 0, "downloading")
            .await;
        download_file(&client, url, &temp_archive, Some((id, engine, broadcast))).await?;

        println!("Extracting archive...");
        broadcast
            .model_download_progress(id, engine, 0, 0, "extracting")
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
            .model_download_progress(id, engine, 0, 0, "downloading")
            .await;
        download_file(&client, url, &output_path, Some((id, engine, broadcast))).await?;
        println!("Model '{}' downloaded successfully", name);
    }

    broadcast
        .model_download_progress(id, engine, 0, 0, "done")
        .await;

    Ok(())
}

/// Removes a model from disk
pub async fn remove(id: ModelId) -> Result<()> {
    let desc = find(id).ok_or_else(|| anyhow!("Model '{:?}' not found in catalog", id))?;

    let model_path = local_path(id)?;
    let name = desc.storage_name;

    if !model_path.exists() {
        println!("Model '{}' is not downloaded", name);
        return Ok(());
    }

    if desc.is_directory {
        // Remove directory recursively
        async_fs::remove_dir_all(&model_path).await?;
    } else {
        // Remove single file
        async_fs::remove_file(&model_path).await?;
    }

    println!("Model '{}' removed successfully", name);
    Ok(())
}

/// Computes total size and counts of downloaded models
pub fn storage_info() -> Result<StorageInfo> {
    let dir = models_dir()?;
    let mut total_size = 0u64;
    let mut downloaded_count = 0;
    let available_count = all_models().len();

    for desc in all_models() {
        let path = local_path(desc.id)?;

        if !path.exists() {
            continue;
        }

        if desc.is_directory {
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
    cache: &mut HashMap<ModelId, (u64, Instant)>,
) -> Result<HashMap<ModelId, u64>> {
    let cache_duration = Duration::from_secs(24 * 60 * 60); // 24 hours
    let now = Instant::now();
    let mut sizes = HashMap::new();
    let mut models_to_fetch = Vec::new();

    // Check cache first
    for desc in all_models() {
        let id = desc.id;
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

// ============================================================================
// Internal helpers
// ============================================================================

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    output_path: &Path,
    progress: Option<(ModelId, ModelEngine, &crate::broadcast::BroadcastServer)>,
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

        if let Some((id, engine, broadcast)) = progress {
            broadcast
                .model_download_progress(id, engine, downloaded, total_size, "downloading")
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

async fn fetch_model_size(client: &reqwest::Client, id: ModelId) -> Result<u64> {
    if let Some(desc) = find(id) {
        let response = client.head(desc.download_url).send().await?;

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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_models_count() {
        assert_eq!(all_models().len(), 6);
    }

    #[test]
    fn test_find_existing_model() {
        let desc = find(ModelId::Whisper(WhisperModel::Base));
        assert!(desc.is_some());
        assert_eq!(desc.unwrap().storage_name, "whisper-base");
    }

    #[test]
    fn test_preferred_or_default_with_valid_preference() {
        let desc = preferred_or_default(Some(ModelId::Whisper(WhisperModel::Small)));
        assert_eq!(desc.id, ModelId::Whisper(WhisperModel::Small));
    }

    #[test]
    fn test_preferred_or_default_fallback_to_parakeet_v3() {
        let desc = preferred_or_default(None);
        assert_eq!(desc.id, ModelId::Parakeet(ParakeetModel::V3));
    }

    #[test]
    fn test_whisper_models_are_files() {
        for desc in all_models() {
            if matches!(desc.id, ModelId::Whisper(_)) {
                assert!(!desc.is_directory, "{:?} should be a file", desc.id);
            }
        }
    }

    #[test]
    fn test_parakeet_models_are_directories() {
        for desc in all_models() {
            if matches!(desc.id, ModelId::Parakeet(_)) {
                assert!(desc.is_directory, "{:?} should be a directory", desc.id);
            }
        }
    }

    #[test]
    fn test_all_models_have_download_urls() {
        for desc in all_models() {
            assert!(
                !desc.download_url.is_empty(),
                "{:?} missing download URL",
                desc.id
            );
        }
    }
}
