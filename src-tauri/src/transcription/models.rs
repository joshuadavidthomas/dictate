use crate::conf;
use anyhow::{Result, anyhow};
use bzip2::read::BzDecoder;
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

/// Whisper model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    TinyEn,
    Tiny,
    BaseEn,
    Base,
    SmallEn,
    Small,
    MediumEn,
    Medium,
}

/// Moonshine model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MoonshineModel {
    TinyEn,
    BaseEn,
}

/// Parakeet TDT model variants (NVIDIA NeMo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParakeetTdtModel {
    /// English-only, 0.6B parameters
    V2,
    /// Multilingual (25 European languages), 0.6B parameters
    V3,
}

/// A transcription model.
///
/// This encodes the invariant that every model belongs to exactly one engine family
/// (Whisper, Moonshine, or ParakeetTdt) and to a finite set of variants within that family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    Whisper(WhisperModel),
    Moonshine(MoonshineModel),
    ParakeetTdt(ParakeetTdtModel),
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
            Model::Moonshine(variant) => {
                state.serialize_field("engine", "moonshine")?;
                state.serialize_field("id", variant)?;
            }
            Model::ParakeetTdt(variant) => {
                state.serialize_field("engine", "parakeet-tdt")?;
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
                let variant =
                    WhisperModel::deserialize(helper.id).map_err(serde::de::Error::custom)?;
                Ok(Model::Whisper(variant))
            }
            "moonshine" => {
                let variant =
                    MoonshineModel::deserialize(helper.id).map_err(serde::de::Error::custom)?;
                Ok(Model::Moonshine(variant))
            }
            "parakeet-tdt" => {
                let variant =
                    ParakeetTdtModel::deserialize(helper.id).map_err(serde::de::Error::custom)?;
                Ok(Model::ParakeetTdt(variant))
            }
            // Handle legacy "parakeet" engine for backwards compatibility
            "parakeet" => {
                // Map old parakeet v2/v3 to new parakeet-tdt
                let id_str = helper.id.as_str().unwrap_or("v2");
                let variant = match id_str {
                    "v3" => ParakeetTdtModel::V3,
                    _ => ParakeetTdtModel::V2,
                };
                Ok(Model::ParakeetTdt(variant))
            }
            engine => Err(serde::de::Error::unknown_variant(
                engine,
                &["whisper", "moonshine", "parakeet-tdt"],
            )),
        }
    }
}

impl Model {
    /// Returns the storage name for this model.
    pub fn storage_name(self) -> &'static str {
        match self {
            Model::Whisper(WhisperModel::TinyEn) => "sherpa-onnx-whisper-tiny.en",
            Model::Whisper(WhisperModel::Tiny) => "sherpa-onnx-whisper-tiny",
            Model::Whisper(WhisperModel::BaseEn) => "sherpa-onnx-whisper-base.en",
            Model::Whisper(WhisperModel::Base) => "sherpa-onnx-whisper-base",
            Model::Whisper(WhisperModel::SmallEn) => "sherpa-onnx-whisper-small.en",
            Model::Whisper(WhisperModel::Small) => "sherpa-onnx-whisper-small",
            Model::Whisper(WhisperModel::MediumEn) => "sherpa-onnx-whisper-medium.en",
            Model::Whisper(WhisperModel::Medium) => "sherpa-onnx-whisper-medium",
            Model::Moonshine(MoonshineModel::TinyEn) => "sherpa-onnx-moonshine-tiny-en-int8",
            Model::Moonshine(MoonshineModel::BaseEn) => "sherpa-onnx-moonshine-base-en-int8",
            Model::ParakeetTdt(ParakeetTdtModel::V2) => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
            Model::ParakeetTdt(ParakeetTdtModel::V3) => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        }
    }

    /// Returns the display name for this model.
    pub fn display_name(self) -> &'static str {
        match self {
            Model::Whisper(WhisperModel::TinyEn) => "Whisper Tiny (English)",
            Model::Whisper(WhisperModel::Tiny) => "Whisper Tiny",
            Model::Whisper(WhisperModel::BaseEn) => "Whisper Base (English)",
            Model::Whisper(WhisperModel::Base) => "Whisper Base",
            Model::Whisper(WhisperModel::SmallEn) => "Whisper Small (English)",
            Model::Whisper(WhisperModel::Small) => "Whisper Small",
            Model::Whisper(WhisperModel::MediumEn) => "Whisper Medium (English)",
            Model::Whisper(WhisperModel::Medium) => "Whisper Medium",
            Model::Moonshine(MoonshineModel::TinyEn) => "Moonshine Tiny",
            Model::Moonshine(MoonshineModel::BaseEn) => "Moonshine Base",
            Model::ParakeetTdt(ParakeetTdtModel::V2) => "Parakeet TDT v2 (English)",
            Model::ParakeetTdt(ParakeetTdtModel::V3) => "Parakeet TDT v3 (Multilingual)",
        }
    }

    /// Returns whether this model is stored as a directory (vs. single file).
    /// All sherpa-onnx models are directory-based.
    pub fn is_directory(self) -> bool {
        true
    }

    /// Returns the download URL for this model.
    pub fn download_url(self) -> &'static str {
        const BASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
        match self {
            Model::Whisper(WhisperModel::TinyEn) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-tiny.en.tar.bz2")
            }
            Model::Whisper(WhisperModel::Tiny) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-tiny.tar.bz2")
            }
            Model::Whisper(WhisperModel::BaseEn) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-base.en.tar.bz2")
            }
            Model::Whisper(WhisperModel::Base) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-base.tar.bz2")
            }
            Model::Whisper(WhisperModel::SmallEn) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-small.en.tar.bz2")
            }
            Model::Whisper(WhisperModel::Small) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-small.tar.bz2")
            }
            Model::Whisper(WhisperModel::MediumEn) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-medium.en.tar.bz2")
            }
            Model::Whisper(WhisperModel::Medium) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-whisper-medium.tar.bz2")
            }
            Model::Moonshine(MoonshineModel::TinyEn) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-moonshine-tiny-en-int8.tar.bz2")
            }
            Model::Moonshine(MoonshineModel::BaseEn) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-moonshine-base-en-int8.tar.bz2")
            }
            Model::ParakeetTdt(ParakeetTdtModel::V2) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2")
            }
            Model::ParakeetTdt(ParakeetTdtModel::V3) => {
                concat!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2")
            }
        }
    }

    /// Returns all supported models.
    pub fn all() -> &'static [Model] {
        &[
            // Moonshine - fastest, English-only
            Model::Moonshine(MoonshineModel::TinyEn),
            Model::Moonshine(MoonshineModel::BaseEn),
            // Parakeet TDT - great balance of speed/accuracy
            Model::ParakeetTdt(ParakeetTdtModel::V2),
            Model::ParakeetTdt(ParakeetTdtModel::V3),
            // Whisper - most language support
            Model::Whisper(WhisperModel::TinyEn),
            Model::Whisper(WhisperModel::Tiny),
            Model::Whisper(WhisperModel::BaseEn),
            Model::Whisper(WhisperModel::Base),
            Model::Whisper(WhisperModel::SmallEn),
            Model::Whisper(WhisperModel::Small),
            Model::Whisper(WhisperModel::MediumEn),
            Model::Whisper(WhisperModel::Medium),
        ]
    }

    /// Resolves the preferred model or falls back to defaults.
    ///
    /// Fallback order:
    /// 1. Preferred model (if provided)
    /// 2. Moonshine Tiny (fastest)
    pub fn preferred_or_default(pref: Option<Model>) -> Model {
        pref.unwrap_or(Model::Moonshine(MoonshineModel::TinyEn))
    }

    /// Builds the local filesystem path for this model directory.
    pub fn local_path(self) -> Result<PathBuf> {
        let dir = models_dir()?;
        Ok(dir.join(self.storage_name()))
    }

    /// Checks if this model exists on disk.
    pub fn is_downloaded(self) -> Result<bool> {
        let path = self.local_path()?;
        Ok(path.exists() && path.is_dir())
    }

    /// Downloads this model with progress reporting.
    pub async fn download(self, broadcast: &crate::broadcast::BroadcastServer) -> Result<()> {
        let output_path = self.local_path()?;
        let name = self.storage_name();

        if output_path.exists() {
            log::info!("Model '{}' is already downloaded", name);
            broadcast.model_download_progress(self, 0, 0, "done").await;
            return Ok(());
        }

        let url = self.download_url();
        let client = reqwest::Client::new();

        let dir = models_dir()?;
        let temp_archive = dir.join(format!("{}.tar.bz2", name));

        log::info!("Downloading model '{}'...", name);
        broadcast
            .model_download_progress(self, 0, 0, "downloading")
            .await;
        download_file(&client, url, &temp_archive, Some((self, broadcast))).await?;

        log::info!("Extracting archive...");
        broadcast
            .model_download_progress(self, 0, 0, "extracting")
            .await;
        extract_tar_bz2(&temp_archive, name).await?;

        // Clean up temporary archive
        async_fs::remove_file(&temp_archive).await?;

        log::info!("Model '{}' downloaded and extracted successfully", name);
        broadcast.model_download_progress(self, 0, 0, "done").await;

        Ok(())
    }

    /// Removes this model from disk.
    pub async fn remove(self) -> Result<()> {
        let model_path = self.local_path()?;
        let name = self.storage_name();

        if !model_path.exists() {
            log::debug!("Model '{}' is not downloaded", name);
            return Ok(());
        }

        async_fs::remove_dir_all(&model_path).await?;

        log::info!("Model '{}' removed successfully", name);
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

        log::info!("Loading transcription model from: {}", path.display());

        let engine = match self {
            Model::Whisper(variant) => {
                use sherpa_rs::whisper::{WhisperConfig, WhisperRecognizer};

                let (encoder_file, decoder_file) = match variant {
                    WhisperModel::TinyEn | WhisperModel::BaseEn | WhisperModel::SmallEn | WhisperModel::MediumEn => {
                        let name = match variant {
                            WhisperModel::TinyEn => "tiny.en",
                            WhisperModel::BaseEn => "base.en",
                            WhisperModel::SmallEn => "small.en",
                            WhisperModel::MediumEn => "medium.en",
                            _ => unreachable!(),
                        };
                        (
                            format!("{}-encoder.int8.onnx", name),
                            format!("{}-decoder.int8.onnx", name),
                        )
                    }
                    WhisperModel::Tiny | WhisperModel::Base | WhisperModel::Small | WhisperModel::Medium => {
                        let name = match variant {
                            WhisperModel::Tiny => "tiny",
                            WhisperModel::Base => "base",
                            WhisperModel::Small => "small",
                            WhisperModel::Medium => "medium",
                            _ => unreachable!(),
                        };
                        (
                            format!("{}-encoder.int8.onnx", name),
                            format!("{}-decoder.int8.onnx", name),
                        )
                    }
                };

                let config = WhisperConfig {
                    encoder: path.join(&encoder_file).to_string_lossy().to_string(),
                    decoder: path.join(&decoder_file).to_string_lossy().to_string(),
                    tokens: path.join("tokens.txt").to_string_lossy().to_string(),
                    language: Some("en".to_string()),
                    ..Default::default()
                };

                let recognizer = WhisperRecognizer::new(config)
                    .map_err(|e| anyhow!("Failed to load Whisper model: {}", e))?;

                LoadedEngine::Whisper { recognizer }
            }
            Model::Moonshine(_variant) => {
                use sherpa_rs::moonshine::{MoonshineConfig, MoonshineRecognizer};

                let config = MoonshineConfig {
                    preprocessor: path.join("preprocess.onnx").to_string_lossy().to_string(),
                    encoder: path.join("encode.int8.onnx").to_string_lossy().to_string(),
                    uncached_decoder: path.join("uncached_decode.int8.onnx").to_string_lossy().to_string(),
                    cached_decoder: path.join("cached_decode.int8.onnx").to_string_lossy().to_string(),
                    tokens: path.join("tokens.txt").to_string_lossy().to_string(),
                    ..Default::default()
                };

                let recognizer = MoonshineRecognizer::new(config)
                    .map_err(|e| anyhow!("Failed to load Moonshine model: {}", e))?;

                LoadedEngine::Moonshine { recognizer }
            }
            Model::ParakeetTdt(_variant) => {
                use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};

                let config = TransducerConfig {
                    encoder: path.join("encoder.int8.onnx").to_string_lossy().to_string(),
                    decoder: path.join("decoder.int8.onnx").to_string_lossy().to_string(),
                    joiner: path.join("joiner.int8.onnx").to_string_lossy().to_string(),
                    tokens: path.join("tokens.txt").to_string_lossy().to_string(),
                    model_type: "nemo_transducer".to_string(),
                    sample_rate: 16_000,
                    feature_dim: 80,
                    ..Default::default()
                };

                let recognizer = TransducerRecognizer::new(config)
                    .map_err(|e| anyhow!("Failed to load Parakeet TDT model: {}", e))?;

                LoadedEngine::ParakeetTdt { recognizer }
            }
        };

        log::info!("Model loaded successfully");
        Ok(engine)
    }
}

/// Runtime inference engine loaded into memory.
pub enum LoadedEngine {
    Whisper {
        recognizer: sherpa_rs::whisper::WhisperRecognizer,
    },
    Moonshine {
        recognizer: sherpa_rs::moonshine::MoonshineRecognizer,
    },
    ParakeetTdt {
        recognizer: sherpa_rs::transducer::TransducerRecognizer,
    },
}

impl LoadedEngine {
    /// Transcribes an audio file to text.
    pub fn transcribe(&mut self, audio_path: &Path) -> Result<String> {
        log::debug!("Transcribing audio file: {}", audio_path.display());

        // Read audio file using sherpa-rs utility
        let (samples, sample_rate) = sherpa_rs::read_audio_file(audio_path)
            .map_err(|e| anyhow!("Failed to read audio file: {}", e))?;

        if sample_rate != 16000 {
            return Err(anyhow!(
                "Audio sample rate must be 16000 Hz, got {} Hz",
                sample_rate
            ));
        }

        let text = match self {
            LoadedEngine::Whisper { recognizer } => {
                recognizer.transcribe(sample_rate, &samples)
            }
            LoadedEngine::Moonshine { recognizer } => {
                let result = recognizer.transcribe(sample_rate, &samples);
                result.text
            }
            LoadedEngine::ParakeetTdt { recognizer } => {
                recognizer.transcribe(sample_rate, &samples)
            }
        };

        let text = text.trim().to_string();
        log::info!("Transcription completed: {}", text);
        Ok(text)
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

        // Calculate directory size recursively
        if let Ok(size) = calculate_dir_size(&path) {
            total_size += size;
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
                    log::warn!("Failed to fetch model size: {}", e);
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
    log::debug!("Downloading to {}", output_path.display());

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

async fn extract_tar_bz2(archive_path: &Path, model_name: &str) -> Result<()> {
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
        let tar_bz2 = std::fs::File::open(&archive_path)?;
        let tar = BzDecoder::new(tar_bz2);
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

    // Fall back to x-linked-size header (GitHub specific)
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
        assert_eq!(Model::all().len(), 12);
    }

    #[test]
    fn test_model_storage_name() {
        let model = Model::Whisper(WhisperModel::BaseEn);
        assert_eq!(model.storage_name(), "sherpa-onnx-whisper-base.en");
    }

    #[test]
    fn test_preferred_or_default_with_valid_preference() {
        let model = Model::preferred_or_default(Some(Model::Whisper(WhisperModel::SmallEn)));
        assert_eq!(model, Model::Whisper(WhisperModel::SmallEn));
    }

    #[test]
    fn test_preferred_or_default_fallback_to_moonshine() {
        let model = Model::preferred_or_default(None);
        assert_eq!(model, Model::Moonshine(MoonshineModel::TinyEn));
    }

    #[test]
    fn test_all_models_are_directories() {
        for &model in Model::all() {
            assert!(model.is_directory(), "{:?} should be a directory", model);
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

    #[test]
    fn test_model_serialization() {
        let model = Model::Whisper(WhisperModel::TinyEn);
        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("\"engine\""));
        assert!(json.contains("\"whisper\""));
        assert!(json.contains("\"id\""));
        assert!(json.contains("\"tiny-en\""));
    }

    #[test]
    fn test_legacy_parakeet_deserialization() {
        // Test that old "parakeet" engine values are migrated to "parakeet-tdt"
        let json = r#"{"engine":"parakeet","id":"v3"}"#;
        let model: Model = serde_json::from_str(json).unwrap();
        assert_eq!(model, Model::ParakeetTdt(ParakeetTdtModel::V3));
    }
}
