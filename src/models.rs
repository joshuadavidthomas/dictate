use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use fs2::available_space;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use futures::future;
use tokio::fs as async_fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ModelDefinition {
    pub name: &'static str,
    pub download_url: &'static str,
    pub sha256_hash: Option<&'static str>,
}

// Static model definitions
const MODEL_DEFINITIONS: [ModelDefinition; 4] = [
    ModelDefinition {
        name: "tiny",
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        sha256_hash: None,
    },
    ModelDefinition {
        name: "base",
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        sha256_hash: None,
    },
    ModelDefinition {
        name: "small",
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        sha256_hash: None,
    },
    ModelDefinition {
        name: "medium",
        download_url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        sha256_hash: None,
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

    pub fn download_url(&self) -> &str {
        self.definition.download_url
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
            let model_path = self.models_dir.join(format!("{}.bin", definition.name));
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

        let response = client.head(model_info.download_url).send().await?;

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

        self.download_file(model_info.download_url(), output_path)
            .await?;
        println!("Model '{}' downloaded successfully", name);
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

        fs::remove_file(model_path)?;
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

    pub fn get_storage_info(&self) -> Result<StorageInfo> {
        let mut total_size = 0u64;
        let mut downloaded_count = 0;

        for model in self.available_models.values() {
            if model.is_downloaded() {
                if let Some(local_path) = &model.local_path {
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
