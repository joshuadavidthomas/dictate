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
use tar::Archive;

const ASR_MODELS_BASE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
const MODEL_DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MODEL_DOWNLOAD_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const MODEL_DOWNLOAD_BODY_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub const DEFAULT_MODEL_ID: ModelId = ModelId::new("parakeet-tdt-0.6b-v2-int8");

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModelId(&'static str);

impl ModelId {
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ModelCatalogEntry {
    id: ModelId,
    display_name: &'static str,
    archive_name: &'static str,
    recognizer: SherpaRecognizerKind,
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
            archive_name,
            recognizer,
        }
    }

    pub fn all() -> &'static [Self] {
        MODEL_CATALOG
    }

    pub const fn id(self) -> ModelId {
        self.id
    }

    pub const fn display_name(self) -> &'static str {
        self.display_name
    }

    pub const fn archive_name(self) -> &'static str {
        self.archive_name
    }

    pub fn download_url(self) -> String {
        format!("{ASR_MODELS_BASE_URL}/{}", self.archive_name)
    }

    pub fn local_dir(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.id.as_str())
    }

    pub fn ensure_downloaded(self) -> Result<PathBuf> {
        let models_dir = local_models_dir()?;
        let model_dir = self.local_dir(&models_dir);
        if model_dir.exists() {
            return Ok(model_dir);
        }

        fs::create_dir_all(&models_dir)?;
        let archive_path = models_dir.join(self.archive_name());
        let download_url = self.download_url();

        eprintln!("downloading {}...", self.display_name());
        download_file(&download_url, &archive_path)?;

        eprintln!("extracting {}...", self.display_name());
        extract_tar_bz2(&archive_path, self.id().as_str())?;

        fs::remove_file(&archive_path).ok();
        eprintln!("{} ready", self.display_name());

        Ok(model_dir)
    }

    pub fn create_recognizer(self, model_dir: &Path) -> Result<OfflineRecognizer> {
        let config = self.recognizer.config(model_dir);

        OfflineRecognizer::create(&config).ok_or_else(|| {
            anyhow!(
                "failed to create sherpa-onnx recognizer for {}",
                self.display_name
            )
        })
    }
}

pub fn default_model() -> &'static ModelCatalogEntry {
    model_by_id(DEFAULT_MODEL_ID.as_str())
        .expect("default transcription model must exist in catalog")
}

pub fn model_by_id(id: &str) -> Option<&'static ModelCatalogEntry> {
    MODEL_CATALOG.iter().find(|model| model.id.as_str() == id)
}

pub fn local_models_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "dictate")
        .ok_or_else(|| anyhow!("could not determine dictate data directory"))?;
    Ok(dirs.data_dir().join("models"))
}

fn download_file(url: &str, output_path: &Path) -> Result<()> {
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
    let total = response.body().content_length().unwrap_or(0);
    let mut reader = response.body_mut().as_reader();
    let mut file = File::create(output_path)?;
    let mut buffer = [0_u8; 1024 * 1024];
    let mut downloaded = 0_u64;
    let mut next_report = 0_u64;

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
        if read == 0 {
            break;
        }

        file.write_all(&buffer[..read])?;
        downloaded += read as u64;

        if total > 0 && downloaded >= next_report {
            eprintln!(
                "downloaded {}/{} MB",
                downloaded / 1_000_000,
                total / 1_000_000
            );
            next_report = downloaded + 25_000_000;
        }
    }

    Ok(())
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
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false)
        })
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

const MODEL_CATALOG: &[ModelCatalogEntry] = &[
    WHISPER_TINY_EN,
    WHISPER_TINY,
    WHISPER_BASE_EN,
    WHISPER_BASE,
    WHISPER_SMALL_EN,
    WHISPER_SMALL,
    WHISPER_MEDIUM_EN,
    WHISPER_MEDIUM,
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
}

impl SherpaRecognizerKind {
    fn config(self, model_dir: &Path) -> OfflineRecognizerConfig {
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
        };

        OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        }
    }
}

fn cpu_model_config() -> OfflineModelConfig {
    OfflineModelConfig {
        num_threads: 2,
        provider: Some("cpu".to_string()),
        ..Default::default()
    }
}

fn model_file(model_dir: &Path, file_name: &str) -> String {
    model_dir.join(file_name).to_string_lossy().to_string()
}

pub struct VadModel;

impl VadModel {
    pub fn file_name() -> &'static str {
        "silero_vad.onnx"
    }

    pub fn display_name() -> &'static str {
        "Silero VAD"
    }

    pub fn download_url() -> &'static str {
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
    }

    pub fn local_path(models_dir: &Path) -> PathBuf {
        models_dir.join(Self::file_name())
    }
}
