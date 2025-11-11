use anyhow::{Result, anyhow};
use directories::ProjectDirs;
use flate2::read::GzDecoder;
use fs2::available_space;
use futures::future;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tar::Archive;
use tokio::fs as async_fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum EngineType {
    Whisper,
    Parakeet,
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineType::Whisper => write!(f, "Whisper"),
            EngineType::Parakeet => write!(f, "Parakeet"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModelDefinition {
    pub name: &'static str,
    pub engine_type: EngineType,
    pub download_url: Option<&'static str>,
    pub sha256_hash: Option<&'static str>,
    pub is_directory: bool,
}

// Static model definitions
const MODEL_DEFINITIONS: [ModelDefinition; 6] = [
    ModelDefinition {
        name: "whisper-tiny",
        engine_type: EngineType::Whisper,
        download_url: Some(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        ),
        sha256_hash: None,
        is_directory: false,
    },
    ModelDefinition {
        name: "whisper-base",
        engine_type: EngineType::Whisper,
        download_url: Some(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        ),
        sha256_hash: None,
        is_directory: false,
    },
    ModelDefinition {
        name: "whisper-small",
        engine_type: EngineType::Whisper,
        download_url: Some(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        ),
        sha256_hash: None,
        is_directory: false,
    },
    ModelDefinition {
        name: "whisper-medium",
        engine_type: EngineType::Whisper,
        download_url: Some(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        ),
        sha256_hash: None,
        is_directory: false,
    },
    ModelDefinition {
        name: "parakeet-v2",
        engine_type: EngineType::Parakeet,
        download_url: Some("https://blob.handy.computer/parakeet-v2-int8.tar.gz"),
        sha256_hash: None,
        is_directory: true,
    },
    ModelDefinition {
        name: "parakeet-v3",
        engine_type: EngineType::Parakeet,
        download_url: Some("https://blob.handy.computer/parakeet-v3-int8.tar.gz"),
        sha256_hash: None,
        is_directory: true,
    },
];

// Dynamic runtime state
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub definition: ModelDefinition,
    pub local_path: Option<PathBuf>,
}

impl ModelInfo {
    pub fn new(definition: ModelDefinition) -> Self {
        Self {
            definition,
            local_path: None,
        }
    }

    pub fn with_local_path(mut self, path: PathBuf) -> Self {
        self.local_path = Some(path);
        self
    }

    pub fn name(&self) -> &str {
        self.definition.name
    }

    pub fn download_url(&self) -> Option<&str> {
        self.definition.download_url
    }

    pub fn engine_type(&self) -> EngineType {
        self.definition.engine_type
    }

    pub fn is_directory(&self) -> bool {
        self.definition.is_directory
    }

    pub fn is_downloaded(&self) -> bool {
        self.local_path
            .as_ref()
            .map(|path| path.exists())
            .unwrap_or(false)
    }
}

pub struct ModelManager {
    models_dir: PathBuf,
    available_models: HashMap<String, ModelInfo>,
    client: reqwest::Client,
    cached_sizes: HashMap<String, (u64, Instant)>,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| anyhow!("Failed to get project directories"))?;

        let models_dir = project_dirs.data_dir().join("models");
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
        for definition in &MODEL_DEFINITIONS {
            let model_path = if definition.is_directory {
                // Directory-based model (e.g., Parakeet)
                self.models_dir.join(definition.name)
            } else {
                // File-based model (e.g., Whisper)
                self.models_dir.join(format!("{}.bin", definition.name))
            };
            let model_info = ModelInfo::new(*definition).with_local_path(model_path);
            self.available_models
                .insert(definition.name.to_string(), model_info);
        }

        Ok(())
    }

    pub fn list_available_models(&self) -> Vec<&ModelInfo> {
        self.available_models.values().collect()
    }

    pub fn get_model_info(&self, name: &str) -> Option<&ModelInfo> {
        self.available_models.get(name)
    }

    pub fn get_model_path(&self, name: &str) -> Option<PathBuf> {
        self.available_models
            .get(name)
            .and_then(|model| model.local_path.as_ref())
            .filter(|path| path.exists())
            .cloned()
    }

    pub async fn get_all_model_sizes(&mut self) -> Result<HashMap<String, u64>> {
        let cache_duration = Duration::from_secs(24 * 60 * 60); // 24 hours
        let now = Instant::now();
        let mut sizes = HashMap::new();
        let mut models_to_fetch = Vec::new();

        // Check cache first
        for model_name in self.available_models.keys() {
            if let Some((size, timestamp)) = self.cached_sizes.get(model_name) {
                if now.duration_since(*timestamp) < cache_duration {
                    sizes.insert(model_name.clone(), *size);
                    continue;
                }
            }
            models_to_fetch.push(model_name.clone());
        }

        // Fetch missing sizes in parallel
        if !models_to_fetch.is_empty() {
            let fetch_futures: Vec<_> = models_to_fetch
                .iter()
                .map(|model_name| {
                    let client = self.client.clone();
                    let model_name = model_name.clone();
                    async move {
                        let size = Self::fetch_single_model_size(&client, &model_name).await?;
                        Ok::<(String, u64), anyhow::Error>((model_name, size))
                    }
                })
                .collect();

            let results = future::join_all(fetch_futures).await;

            for result in results {
                match result {
                    Ok((model_name, size)) => {
                        sizes.insert(model_name.clone(), size);
                        // Update cache
                        self.cached_sizes.insert(model_name, (size, now));
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to fetch model size: {}", e);
                    }
                }
            }
        }

        Ok(sizes)
    }

    async fn fetch_single_model_size(client: &reqwest::Client, model_name: &str) -> Result<u64> {
        // Find the model info
        let model_info = MODEL_DEFINITIONS
            .iter()
            .find(|def| def.name == model_name)
            .ok_or_else(|| anyhow!("Model '{}' not found", model_name))?;

        // Fetch size from HTTP headers (works for both single files and tar.gz)
        if let Some(url) = model_info.download_url {
            let response = client.head(url).send().await?;

            // Try content-length header first
            if let Some(size) = response.headers().get("content-length") {
                if let Ok(size_str) = size.to_str() {
                    if let Ok(size) = size_str.parse::<u64>() {
                        return Ok(size);
                    }
                }
            }

            // Fall back to x-linked-size header (HuggingFace specific)
            if let Some(size) = response.headers().get("x-linked-size") {
                if let Ok(size_str) = size.to_str() {
                    if let Ok(size) = size_str.parse::<u64>() {
                        return Ok(size);
                    }
                }
            }
        }

        Ok(0)
    }

    pub async fn download_model(&self, name: &str) -> Result<()> {
        let model_info = self
            .available_models
            .get(name)
            .ok_or_else(|| anyhow!("Model '{}' not found", name))?;

        if model_info.is_downloaded() {
            println!("Model '{}' is already downloaded", name);
            return Ok(());
        }

        let output_path = model_info
            .local_path
            .as_ref()
            .ok_or_else(|| anyhow!("No local path defined for model"))?;

        let url = model_info
            .download_url()
            .ok_or_else(|| anyhow!("Model '{}' has no download URL defined", name))?;

        if model_info.is_directory() {
            // Directory-based model (e.g., Parakeet) - download tar.gz and extract
            let temp_archive = self.models_dir.join(format!("{}.tar.gz", name));

            println!("Downloading model '{}'...", name);
            self.download_file(url, &temp_archive).await?;

            println!("Extracting archive...");
            self.extract_tar_gz(&temp_archive, name).await?;

            // Clean up temporary archive
            async_fs::remove_file(&temp_archive).await?;

            println!("Model '{}' downloaded and extracted successfully", name);
        } else {
            // Single file download (e.g., Whisper models)
            if let Some(parent) = output_path.parent() {
                async_fs::create_dir_all(parent).await?;
            }
            self.download_file(url, output_path).await?;
            println!("Model '{}' downloaded successfully", name);
        }

        Ok(())
    }

    pub async fn remove_model(&self, name: &str) -> Result<()> {
        let model_info = self
            .available_models
            .get(name)
            .ok_or_else(|| anyhow!("Model '{}' not found", name))?;

        if !model_info.is_downloaded() {
            println!("Model '{}' is not downloaded", name);
            return Ok(());
        }

        let model_path = model_info
            .local_path
            .as_ref()
            .ok_or_else(|| anyhow!("No local path defined for model"))?;

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

    async fn download_file(&self, url: &str, output_path: &Path) -> Result<()> {
        println!("Downloading to {}", output_path.display());

        if let Some(parent) = output_path.parent() {
            async_fs::create_dir_all(parent).await?;
        }

        let response = self.client.get(url).send().await?;
        let total_size = response.content_length().unwrap_or(0);

        // Check disk space
        if total_size > 0 {
            if let Some(parent) = output_path.parent() {
                if let Ok(available) = available_space(parent) {
                    let required_space = (total_size as f64 * 1.1) as u64;
                    if available < required_space {
                        return Err(anyhow!(
                            "Insufficient disk space. Need {} MB, available {} MB",
                            required_space / 1_000_000,
                            available / 1_000_000
                        ));
                    }
                }
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

        let bytes = response.bytes().await?;
        let mut file = async_fs::File::create(output_path).await?;

        const CHUNK_SIZE: usize = 8192;
        let mut downloaded = 0u64;

        for chunk in bytes.chunks(CHUNK_SIZE) {
            file.write_all(chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
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

    pub fn get_storage_info(&self) -> Result<StorageInfo> {
        let mut total_size = 0u64;
        let mut downloaded_count = 0;

        for model in self.available_models.values() {
            if model.is_downloaded() {
                if let Some(local_path) = &model.local_path {
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
