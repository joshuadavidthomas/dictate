use std::fs;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use bzip2::read::BzDecoder;
use directories::ProjectDirs;
use sherpa_onnx::OfflineModelConfig;
use sherpa_onnx::OfflineMoonshineModelConfig;
use sherpa_onnx::OfflineNemoEncDecCtcModelConfig;
use sherpa_onnx::OfflineRecognizer;
use sherpa_onnx::OfflineRecognizerConfig;
use sherpa_onnx::OfflineSenseVoiceModelConfig;
use sherpa_onnx::OfflineTransducerModelConfig;
use sherpa_onnx::OfflineWhisperModelConfig;
use sherpa_onnx::OnlineModelConfig;
use sherpa_onnx::OnlineNemoCtcModelConfig;
use sherpa_onnx::OnlineRecognizer;
use sherpa_onnx::OnlineRecognizerConfig;
use sherpa_onnx::OnlineTransducerModelConfig;
use tar::Archive;

use crate::transcription::Recognizer;
use crate::transcription::StreamingFinalization;

const ASR_MODELS_BASE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
const MODEL_DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MODEL_DOWNLOAD_RESPONSE_TIMEOUT: Duration = Duration::from_mins(1);
const MODEL_DOWNLOAD_BODY_TIMEOUT: Duration = Duration::from_mins(15);

pub const DEFAULT_MODEL_ID: ModelId = ModelId::new("bark-3.0-clean-q8");

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModelId(&'static str);

impl ModelId {
    #[must_use]
    const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ModelCatalogEntry {
    id: ModelId,
    display_name: &'static str,
    backend: ModelBackend,
}

#[derive(Clone, Copy, Debug)]
enum ModelBackend {
    /// A `.tar.bz2` from sherpa-onnx's `asr-models` release, run by sherpa-onnx.
    Sherpa {
        archive_name: &'static str,
        recognizer: SherpaRecognizerKind,
    },
    /// One GGUF file from a Hugging Face repo pinned to a commit, run by
    /// transcribe.cpp.
    Gguf {
        repo: &'static str,
        revision: &'static str,
        file: &'static str,
    },
}

impl ModelCatalogEntry {
    const fn new(
        id: ModelId,
        display_name: &'static str,
        archive_name: &'static str,
        recognizer: SherpaRecognizerKind,
    ) -> Self {
        Self {
            id,
            display_name,
            backend: ModelBackend::Sherpa {
                archive_name,
                recognizer,
            },
        }
    }

    const fn gguf(
        id: ModelId,
        display_name: &'static str,
        repo: &'static str,
        revision: &'static str,
        file: &'static str,
    ) -> Self {
        Self {
            id,
            display_name,
            backend: ModelBackend::Gguf {
                repo,
                revision,
                file,
            },
        }
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        MODEL_CATALOG
    }

    #[must_use]
    pub const fn id(self) -> ModelId {
        self.id
    }

    #[must_use]
    const fn display_name(self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub fn local_dir(self, catalog_dir: &Path) -> PathBuf {
        catalog_dir.join(self.id.as_str())
    }

    pub fn ensure_downloaded(self) -> Result<PathBuf> {
        let catalog_dir = local_models_dir()?;
        let model_dir = self.local_dir(&catalog_dir);
        match self.backend {
            ModelBackend::Sherpa { archive_name, .. } => {
                if model_dir.exists() {
                    return Ok(model_dir);
                }

                fs::create_dir_all(&catalog_dir)?;
                let archive_path = catalog_dir.join(archive_name);

                eprintln!("downloading {}...", self.display_name());
                download_file(
                    &format!("{ASR_MODELS_BASE_URL}/{archive_name}"),
                    &archive_path,
                )?;

                eprintln!("extracting {}...", self.display_name());
                extract_tar_bz2(&archive_path, self.id().as_str())?;

                drop(fs::remove_file(&archive_path));
            }
            ModelBackend::Gguf {
                repo,
                revision,
                file,
            } => {
                let model_path = model_dir.join(file);
                if model_path.is_file() {
                    return Ok(model_dir);
                }

                fs::create_dir_all(&model_dir)?;
                // Download beside the final path and rename, so an interrupted
                // download never looks like an installed model.
                let partial_path = model_dir.join(format!("{file}.partial"));

                eprintln!("downloading {}...", self.display_name());
                download_file(
                    &format!("https://huggingface.co/{repo}/resolve/{revision}/{file}"),
                    &partial_path,
                )?;
                fs::rename(&partial_path, &model_path)?;
            }
        }
        eprintln!("{} ready", self.display_name());

        Ok(model_dir)
    }

    pub fn create_recognizer(self, model_dir: &Path) -> Result<Recognizer> {
        let recognizer = match self.backend {
            ModelBackend::Sherpa { recognizer, .. } => recognizer,
            ModelBackend::Gguf { file, .. } => {
                return Recognizer::from_gguf(&model_dir.join(file))
                    .map_err(|error| anyhow!("failed to load {}: {error}", self.display_name));
            }
        };

        if recognizer.is_streaming() {
            let (config, finalization) = recognizer.online_config(model_dir)?;
            let recognizer = OnlineRecognizer::create(&config).ok_or_else(|| {
                anyhow!(
                    "failed to create sherpa-onnx streaming recognizer for {}",
                    self.display_name
                )
            })?;

            Ok(Recognizer::from_sherpa_online(recognizer, finalization))
        } else {
            let config = recognizer.offline_config(model_dir)?;
            let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
                anyhow!(
                    "failed to create sherpa-onnx recognizer for {}",
                    self.display_name
                )
            })?;

            Ok(Recognizer::from_sherpa(recognizer))
        }
    }

    #[must_use]
    pub const fn is_streaming(self) -> bool {
        match self.backend {
            ModelBackend::Sherpa { recognizer, .. } => recognizer.is_streaming(),
            ModelBackend::Gguf { .. } => false,
        }
    }
}

#[must_use]
pub fn default_model() -> &'static ModelCatalogEntry {
    &BARK_3_0_CLEAN_Q8
}

/// Streaming model that feeds live partials while the final model decodes
/// the completed recording. Chosen for realtime CPU decode, not accuracy.
#[must_use]
pub fn default_partials_model() -> &'static ModelCatalogEntry {
    &FAST_CONFORMER_CTC_80MS_INT8
}

#[must_use]
pub fn model_by_id(id: &str) -> Option<&'static ModelCatalogEntry> {
    MODEL_CATALOG.iter().find(|model| model.id.as_str() == id)
}

pub fn local_models_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "dictate")
        .ok_or_else(|| anyhow!("could not determine dictate data directory"))?;
    Ok(dirs.data_dir().join("models"))
}

fn download_file(url: &str, output_path: &Path) -> Result<()> {
    let result = (|| {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(MODEL_DOWNLOAD_CONNECT_TIMEOUT))
            .timeout_recv_response(Some(MODEL_DOWNLOAD_RESPONSE_TIMEOUT))
            // ureq 3.3's recv-body timeout is a phase-wide deadline, not a
            // per-read stall timeout, so keep it large enough for healthy model
            // downloads over slow links.
            .timeout_recv_body(Some(MODEL_DOWNLOAD_BODY_TIMEOUT))
            .build()
            .into();
        let mut response = agent
            .get(url)
            .call()
            .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
        let expected = response.body().content_length();
        let total = expected.unwrap_or(0);
        let mut reader = response.body_mut().as_reader();
        let mut file = File::create(output_path)
            .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
        let mut buffer = vec![0_u8; 1024 * 1024].into_boxed_slice();
        let mut downloaded = 0_u64;
        let mut next_report = 0_u64;

        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
            if read == 0 {
                break;
            }

            file.write_all(&buffer[..read])
                .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
            downloaded += u64::try_from(read).map_or(0, |bytes| bytes);

            if total > 0 && downloaded >= next_report {
                eprintln!(
                    "downloaded {}/{} MB",
                    downloaded / 1_000_000,
                    total / 1_000_000
                );
                next_report = downloaded + 25_000_000;
            }
        }

        verify_download_length(expected, downloaded)
            .map_err(|error| anyhow!("failed to download {url}: {error}"))
    })();

    if result.is_err() {
        drop(fs::remove_file(output_path));
    }

    result
}

fn verify_download_length(expected: Option<u64>, downloaded: u64) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };

    if expected == downloaded {
        return Ok(());
    }

    Err(anyhow!(
        "download length mismatch: received {downloaded} bytes, expected {expected}"
    ))
}

fn extract_tar_bz2(archive_path: &Path, model_name: &str) -> Result<()> {
    let models_dir = local_models_dir()?;
    let temp_extract_dir = models_dir.join(format!("{model_name}.extracting"));
    let final_model_dir = models_dir.join(model_name);

    if temp_extract_dir.exists() {
        fs::remove_dir_all(&temp_extract_dir)?;
    }
    fs::create_dir_all(&temp_extract_dir)?;

    let tar_bz2 = File::open(archive_path)?;
    let tar = BzDecoder::new(tar_bz2);
    let mut archive = Archive::new(tar);
    archive.unpack(&temp_extract_dir)?;

    let extracted_dirs = fs::read_dir(&temp_extract_dir)?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .collect::<Vec<_>>();

    if final_model_dir.exists() {
        fs::remove_dir_all(&final_model_dir)?;
    }

    if extracted_dirs.len() == 1 {
        fs::rename(extracted_dirs[0].path(), &final_model_dir)?;
        fs::remove_dir_all(&temp_extract_dir)?;
    } else {
        fs::rename(&temp_extract_dir, &final_model_dir)?;
    }

    Ok(())
}

const WHISPER_TINY_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-tiny-en"),
    "Whisper tiny.en",
    "sherpa-onnx-whisper-tiny.en.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "tiny.en",
        language: Some("en"),
    },
);

const WHISPER_TINY: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-tiny"),
    "Whisper tiny",
    "sherpa-onnx-whisper-tiny.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "tiny",
        language: None,
    },
);

const WHISPER_BASE_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-base-en"),
    "Whisper base.en",
    "sherpa-onnx-whisper-base.en.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "base.en",
        language: Some("en"),
    },
);

const WHISPER_BASE: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-base"),
    "Whisper base",
    "sherpa-onnx-whisper-base.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "base",
        language: None,
    },
);

const WHISPER_SMALL_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-small-en"),
    "Whisper small.en",
    "sherpa-onnx-whisper-small.en.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "small.en",
        language: Some("en"),
    },
);

const WHISPER_SMALL: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-small"),
    "Whisper small",
    "sherpa-onnx-whisper-small.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "small",
        language: None,
    },
);

const WHISPER_MEDIUM_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-medium-en"),
    "Whisper medium.en",
    "sherpa-onnx-whisper-medium.en.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "medium.en",
        language: Some("en"),
    },
);

const WHISPER_MEDIUM: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-medium"),
    "Whisper medium",
    "sherpa-onnx-whisper-medium.tar.bz2",
    SherpaRecognizerKind::Whisper {
        prefix: "medium",
        language: None,
    },
);

const PARAKEET_TDT_V2_INT8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-tdt-0.6b-v2-int8"),
    "Parakeet TDT 0.6B v2 int8",
    "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
    SherpaRecognizerKind::NemoTransducer,
);

const PARAKEET_TDT_V3_INT8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-tdt-0.6b-v3-int8"),
    "Parakeet TDT 0.6B v3 int8",
    "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
    SherpaRecognizerKind::NemoTransducer,
);

const PARAKEET_TDT_CTC_110M_INT8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-tdt-ctc-110m-int8"),
    "Parakeet TDT-CTC 110M int8",
    "sherpa-onnx-nemo-parakeet_tdt_ctc_110m-en-36000-int8.tar.bz2",
    SherpaRecognizerKind::NemoCtc,
);

const PARAKEET_UNIFIED_STREAMING_560_INT8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-unified-0.6b-int8-streaming-560ms"),
    "Parakeet Unified 0.6B streaming 560ms int8",
    "sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms.tar.bz2",
    SherpaRecognizerKind::ParakeetUnifiedStreaming,
);

const FAST_CONFORMER_CTC_80MS_INT8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("fast-conformer-ctc-en-80ms-int8"),
    "NeMo streaming Fast Conformer CTC 80ms int8",
    "sherpa-onnx-nemo-streaming-fast-conformer-ctc-en-80ms-int8.tar.bz2",
    SherpaRecognizerKind::NemoStreamingCtc,
);

const SENSE_VOICE_SMALL_INT8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("sense-voice-small-int8"),
    "SenseVoice Small int8",
    "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2",
    SherpaRecognizerKind::SenseVoice {
        language: "en",
        use_itn: true,
    },
);

const MOONSHINE_TINY_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("moonshine-tiny-en"),
    "Moonshine Tiny English",
    "sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
    SherpaRecognizerKind::MoonshineV1,
);

const MOONSHINE_BASE_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("moonshine-base-en"),
    "Moonshine Base English",
    "sherpa-onnx-moonshine-base-en-int8.tar.bz2",
    SherpaRecognizerKind::MoonshineV1,
);

const MOONSHINE_V2_TINY_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("moonshine-v2-tiny-en"),
    "Moonshine v2 Tiny English",
    "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
    SherpaRecognizerKind::MoonshineV2,
);

const MOONSHINE_V2_BASE_EN: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("moonshine-v2-base-en"),
    "Moonshine v2 Base English",
    "sherpa-onnx-moonshine-base-en-quantized-2026-02-27.tar.bz2",
    SherpaRecognizerKind::MoonshineV2,
);

const BARK_3_0_CLEAN_Q8: ModelCatalogEntry = ModelCatalogEntry::gguf(
    ModelId::new("bark-3.0-clean-q8"),
    "Bark 3.0 Clean Q8_0",
    "joshthomasdev/underdog-bark-3.0-clean-gguf",
    "868701fd53d62c59d759ed1d79e095118f3fa7b9",
    "underdog-bark-3.0-clean-Q8_0.gguf",
);

const BARK_3_0_Q8: ModelCatalogEntry = ModelCatalogEntry::gguf(
    ModelId::new("bark-3.0-q8"),
    "Bark 3.0 Q8_0",
    "joshthomasdev/underdog-bark-3.0-gguf",
    "cee1f12dfe250983ed4987f7b4a95f4577c2cb97",
    "underdog-bark-3.0-Q8_0.gguf",
);

const BARK_3_0_PLUS_Q8: ModelCatalogEntry = ModelCatalogEntry::gguf(
    ModelId::new("bark-3.0-plus-q8"),
    "Bark 3.0 Plus Q8_0",
    "joshthomasdev/underdog-bark-3.0-plus-gguf",
    "3c7c39a890cb11a5b1854ce6643dea9167e58885",
    "underdog-bark-3.0-plus-Q8_0.gguf",
);

const BARK_3_0_PRO_Q8: ModelCatalogEntry = ModelCatalogEntry::gguf(
    ModelId::new("bark-3.0-pro-q8"),
    "Bark 3.0 Pro Q8_0",
    "joshthomasdev/underdog-bark-3.0-pro-gguf",
    "5c10586263806666c0a8b4905a3346a7c679803e",
    "underdog-bark-3.0-pro-Q8_0.gguf",
);

const PARAKEET_TDT_V2_Q8: ModelCatalogEntry = ModelCatalogEntry::gguf(
    ModelId::new("parakeet-tdt-0.6b-v2-q8"),
    "Parakeet TDT 0.6B v2 Q8_0",
    "handy-computer/parakeet-tdt-0.6b-v2-gguf",
    "58239cf6d0ecb0af921e7936c1e5e0a8dba91dbb",
    "parakeet-tdt-0.6b-v2-Q8_0.gguf",
);

const MODEL_CATALOG: &[ModelCatalogEntry] = &[
    BARK_3_0_CLEAN_Q8,
    BARK_3_0_Q8,
    BARK_3_0_PLUS_Q8,
    BARK_3_0_PRO_Q8,
    PARAKEET_TDT_V2_Q8,
    WHISPER_TINY_EN,
    WHISPER_TINY,
    WHISPER_BASE_EN,
    WHISPER_BASE,
    WHISPER_SMALL_EN,
    WHISPER_SMALL,
    WHISPER_MEDIUM_EN,
    WHISPER_MEDIUM,
    PARAKEET_UNIFIED_STREAMING_560_INT8,
    FAST_CONFORMER_CTC_80MS_INT8,
    PARAKEET_TDT_V2_INT8,
    PARAKEET_TDT_V3_INT8,
    PARAKEET_TDT_CTC_110M_INT8,
    SENSE_VOICE_SMALL_INT8,
    MOONSHINE_TINY_EN,
    MOONSHINE_BASE_EN,
    MOONSHINE_V2_TINY_EN,
    MOONSHINE_V2_BASE_EN,
];

#[derive(Clone, Copy, Debug)]
enum SherpaRecognizerKind {
    Whisper {
        prefix: &'static str,
        language: Option<&'static str>,
    },
    NemoTransducer,
    NemoCtc,
    SenseVoice {
        language: &'static str,
        use_itn: bool,
    },
    MoonshineV1,
    MoonshineV2,
    ParakeetUnifiedStreaming,
    NemoStreamingCtc,
}

impl SherpaRecognizerKind {
    const fn is_streaming(self) -> bool {
        matches!(
            self,
            Self::ParakeetUnifiedStreaming | Self::NemoStreamingCtc
        )
    }

    fn online_config(
        self,
        model_dir: &Path,
    ) -> Result<(OnlineRecognizerConfig, StreamingFinalization)> {
        match self {
            Self::ParakeetUnifiedStreaming => Ok((
                OnlineRecognizerConfig {
                    model_config: OnlineModelConfig {
                        transducer: OnlineTransducerModelConfig {
                            encoder: Some(model_file(model_dir, "encoder.int8.onnx")),
                            decoder: Some(model_file(model_dir, "decoder.int8.onnx")),
                            joiner: Some(model_file(model_dir, "joiner.int8.onnx")),
                        },
                        tokens: Some(model_file(model_dir, "tokens.txt")),
                        ..online_cpu_model_config(PARAKEET_UNIFIED_STREAMING_THREADS)
                    },
                    ..Default::default()
                },
                StreamingFinalization::EndOfInput,
            )),
            Self::NemoStreamingCtc => Ok((
                OnlineRecognizerConfig {
                    model_config: OnlineModelConfig {
                        nemo_ctc: OnlineNemoCtcModelConfig {
                            model: Some(model_file(model_dir, "model.int8.onnx")),
                        },
                        tokens: Some(model_file(model_dir, "tokens.txt")),
                        ..online_cpu_model_config(NEMO_STREAMING_CTC_THREADS)
                    },
                    ..Default::default()
                },
                StreamingFinalization::CtcTailPadding,
            )),
            Self::Whisper { .. }
            | Self::NemoTransducer
            | Self::NemoCtc
            | Self::SenseVoice { .. }
            | Self::MoonshineV1
            | Self::MoonshineV2 => Err(anyhow!("offline model kinds have no streaming config")),
        }
    }

    fn offline_config(self, model_dir: &Path) -> Result<OfflineRecognizerConfig> {
        let model_config = match self {
            Self::Whisper { prefix, language } => OfflineModelConfig {
                whisper: OfflineWhisperModelConfig {
                    encoder: Some(model_file(
                        model_dir,
                        &format!("{prefix}-encoder.int8.onnx"),
                    )),
                    decoder: Some(model_file(
                        model_dir,
                        &format!("{prefix}-decoder.int8.onnx"),
                    )),
                    language: language.map(str::to_string),
                    task: Some("transcribe".to_string()),
                    ..Default::default()
                },
                tokens: Some(model_file(model_dir, &format!("{prefix}-tokens.txt"))),
                ..cpu_model_config()
            },
            Self::NemoTransducer => OfflineModelConfig {
                transducer: OfflineTransducerModelConfig {
                    encoder: Some(model_file(model_dir, "encoder.int8.onnx")),
                    decoder: Some(model_file(model_dir, "decoder.int8.onnx")),
                    joiner: Some(model_file(model_dir, "joiner.int8.onnx")),
                },
                tokens: Some(model_file(model_dir, "tokens.txt")),
                model_type: Some("nemo_transducer".to_string()),
                ..cpu_model_config()
            },
            Self::NemoCtc => OfflineModelConfig {
                nemo_ctc: OfflineNemoEncDecCtcModelConfig {
                    model: Some(model_file(model_dir, "model.int8.onnx")),
                },
                tokens: Some(model_file(model_dir, "tokens.txt")),
                ..cpu_model_config()
            },
            Self::SenseVoice { language, use_itn } => OfflineModelConfig {
                sense_voice: OfflineSenseVoiceModelConfig {
                    model: Some(model_file(model_dir, "model.int8.onnx")),
                    language: Some(language.to_string()),
                    use_itn,
                },
                tokens: Some(model_file(model_dir, "tokens.txt")),
                ..cpu_model_config()
            },
            Self::MoonshineV1 => OfflineModelConfig {
                moonshine: OfflineMoonshineModelConfig {
                    preprocessor: Some(model_file(model_dir, "preprocess.onnx")),
                    encoder: Some(model_file(model_dir, "encode.int8.onnx")),
                    uncached_decoder: Some(model_file(model_dir, "uncached_decode.int8.onnx")),
                    cached_decoder: Some(model_file(model_dir, "cached_decode.int8.onnx")),
                    merged_decoder: None,
                },
                tokens: Some(model_file(model_dir, "tokens.txt")),
                ..cpu_model_config()
            },
            Self::MoonshineV2 => OfflineModelConfig {
                moonshine: OfflineMoonshineModelConfig {
                    preprocessor: None,
                    encoder: Some(model_file(model_dir, "encoder_model.ort")),
                    uncached_decoder: None,
                    cached_decoder: None,
                    merged_decoder: Some(model_file(model_dir, "decoder_model_merged.ort")),
                },
                tokens: Some(model_file(model_dir, "tokens.txt")),
                ..cpu_model_config()
            },
            Self::ParakeetUnifiedStreaming | Self::NemoStreamingCtc => {
                return Err(anyhow!("streaming model kinds have no offline config"));
            }
        };

        Ok(OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        })
    }
}

fn cpu_model_config() -> OfflineModelConfig {
    OfflineModelConfig {
        num_threads: 2,
        provider: Some("cpu".to_string()),
        ..Default::default()
    }
}

// Measured on a Ryzen 7 PRO 5850U: Parakeet Unified streaming needs 8 threads
// to reach ~4x realtime; the fast-conformer partials model reaches ~8x at 4.
const PARAKEET_UNIFIED_STREAMING_THREADS: i32 = 8;
const NEMO_STREAMING_CTC_THREADS: i32 = 4;

fn online_cpu_model_config(num_threads: i32) -> OnlineModelConfig {
    OnlineModelConfig {
        num_threads,
        provider: Some("cpu".to_string()),
        ..Default::default()
    }
}

fn model_file(model_dir: &Path, file_name: &str) -> String {
    model_dir.join(file_name).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_models_use_decoder_specific_finalization() {
        assert_eq!(
            SherpaRecognizerKind::ParakeetUnifiedStreaming
                .online_config(Path::new("model"))
                .expect("streaming model should have an online config")
                .1,
            StreamingFinalization::EndOfInput
        );
        assert_eq!(
            SherpaRecognizerKind::NemoStreamingCtc
                .online_config(Path::new("model"))
                .expect("streaming model should have an online config")
                .1,
            StreamingFinalization::CtcTailPadding
        );
    }

    #[test]
    fn verify_download_length_allows_unknown_expected_size() {
        assert!(verify_download_length(None, 42).is_ok());
    }

    #[test]
    fn verify_download_length_allows_exact_match() {
        assert!(verify_download_length(Some(42), 42).is_ok());
    }

    #[test]
    fn verify_download_length_rejects_short_body() {
        let error = verify_download_length(Some(42), 12)
            .expect_err("short body should fail length validation")
            .to_string();

        assert!(error.contains("download length mismatch: received 12 bytes, expected 42"));
    }

    #[test]
    fn verify_download_length_rejects_long_body() {
        assert!(verify_download_length(Some(42), 100).is_err());
    }
}
