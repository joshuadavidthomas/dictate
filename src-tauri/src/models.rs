mod catalog;
mod storage;

pub use catalog::*;
pub use storage::*;

use anyhow::Result;
use futures::future;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

pub struct ModelManager {
    client: reqwest::Client,
    cached_sizes: HashMap<ModelId, (u64, Instant)>,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        // Ensure models directory exists
        storage::models_dir()?;

        Ok(Self {
            client: reqwest::Client::new(),
            cached_sizes: HashMap::new(),
        })
    }

    pub fn list_available_models(&self) -> Vec<ModelId> {
        catalog::all_models().iter().map(|desc| desc.id).collect()
    }

    pub fn has_model(&self, id: ModelId) -> bool {
        catalog::find(id).is_some()
    }

    pub fn is_model_downloaded(&self, id: ModelId) -> bool {
        storage::is_downloaded(id).unwrap_or(false)
    }

    pub fn get_model_path(&self, id: ModelId) -> Option<PathBuf> {
        storage::local_path(id)
            .ok()
            .filter(|path| path.exists())
    }

    pub async fn get_all_model_sizes(&mut self) -> Result<HashMap<ModelId, u64>> {
        let cache_duration = Duration::from_secs(24 * 60 * 60); // 24 hours
        let now = Instant::now();
        let mut sizes = HashMap::new();
        let mut models_to_fetch = Vec::new();

        // Check cache first
        for desc in catalog::all_models() {
            let id = desc.id;
            if let Some((size, timestamp)) = self.cached_sizes.get(&id)
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
        if let Some(desc) = catalog::find(id) {
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

    pub async fn download_model(
        &self,
        id: ModelId,
        broadcast: &crate::broadcast::BroadcastServer,
    ) -> Result<()> {
        storage::download(id, broadcast).await
    }

    pub async fn remove_model(&self, id: ModelId) -> Result<()> {
        storage::remove(id).await
    }

    pub fn get_storage_info(&self) -> Result<StorageInfo> {
        storage::storage_info()
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new().unwrap_or_else(|e| {
            eprintln!("Failed to create ModelManager: {}", e);
            std::process::exit(1);
        })
    }
}
