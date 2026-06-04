use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::anyhow;
use sherpa_onnx::OfflineMoonshineModelConfig;
use sherpa_onnx::OfflineNemoEncDecCtcModelConfig;
use sherpa_onnx::OfflineRecognizer;
use sherpa_onnx::OfflineRecognizerConfig;
use sherpa_onnx::OfflineSenseVoiceModelConfig;
use sherpa_onnx::OfflineTransducerModelConfig;
use sherpa_onnx::OfflineWhisperModelConfig;

const ASR_MODELS_BASE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptionModel {
    WhisperTinyEn,
    WhisperTiny,
    WhisperBaseEn,
    WhisperBase,
    WhisperSmallEn,
    WhisperSmall,
    WhisperMediumEn,
    WhisperMedium,
    ParakeetTdtV2Int8,
    ParakeetTdtV3Int8,
    ParakeetTdtCtc110mInt8,
    SenseVoiceSmallInt8,
    MoonshineTinyEn,
    MoonshineBaseEn,
    MoonshineV2TinyEn,
    MoonshineV2BaseEn,
}

impl TranscriptionModel {
    pub const fn default() -> Self {
        Self::WhisperBaseEn
    }

    pub const fn all() -> &'static [Self] {
        &[
            Self::WhisperTinyEn,
            Self::WhisperTiny,
            Self::WhisperBaseEn,
            Self::WhisperBase,
            Self::WhisperSmallEn,
            Self::WhisperSmall,
            Self::WhisperMediumEn,
            Self::WhisperMedium,
            Self::ParakeetTdtV2Int8,
            Self::ParakeetTdtV3Int8,
            Self::ParakeetTdtCtc110mInt8,
            Self::SenseVoiceSmallInt8,
            Self::MoonshineTinyEn,
            Self::MoonshineBaseEn,
            Self::MoonshineV2TinyEn,
            Self::MoonshineV2BaseEn,
        ]
    }

    pub fn storage_name(self) -> &'static str {
        match self {
            Self::WhisperTinyEn => "whisper-tiny-en",
            Self::WhisperTiny => "whisper-tiny",
            Self::WhisperBaseEn => "whisper-base-en",
            Self::WhisperBase => "whisper-base",
            Self::WhisperSmallEn => "whisper-small-en",
            Self::WhisperSmall => "whisper-small",
            Self::WhisperMediumEn => "whisper-medium-en",
            Self::WhisperMedium => "whisper-medium",
            Self::ParakeetTdtV2Int8 => "parakeet-tdt-0.6b-v2-int8",
            Self::ParakeetTdtV3Int8 => "parakeet-tdt-0.6b-v3-int8",
            Self::ParakeetTdtCtc110mInt8 => "parakeet-tdt-ctc-110m-int8",
            Self::SenseVoiceSmallInt8 => "sense-voice-small-int8",
            Self::MoonshineTinyEn => "moonshine-tiny-en",
            Self::MoonshineBaseEn => "moonshine-base-en",
            Self::MoonshineV2TinyEn => "moonshine-v2-tiny-en",
            Self::MoonshineV2BaseEn => "moonshine-v2-base-en",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::WhisperTinyEn => "Whisper tiny.en",
            Self::WhisperTiny => "Whisper tiny",
            Self::WhisperBaseEn => "Whisper base.en",
            Self::WhisperBase => "Whisper base",
            Self::WhisperSmallEn => "Whisper small.en",
            Self::WhisperSmall => "Whisper small",
            Self::WhisperMediumEn => "Whisper medium.en",
            Self::WhisperMedium => "Whisper medium",
            Self::ParakeetTdtV2Int8 => "Parakeet TDT 0.6B v2 int8",
            Self::ParakeetTdtV3Int8 => "Parakeet TDT 0.6B v3 int8",
            Self::ParakeetTdtCtc110mInt8 => "Parakeet TDT-CTC 110M int8",
            Self::SenseVoiceSmallInt8 => "SenseVoice Small int8",
            Self::MoonshineTinyEn => "Moonshine Tiny English",
            Self::MoonshineBaseEn => "Moonshine Base English",
            Self::MoonshineV2TinyEn => "Moonshine v2 Tiny English",
            Self::MoonshineV2BaseEn => "Moonshine v2 Base English",
        }
    }

    pub fn archive_name(self) -> &'static str {
        match self {
            Self::WhisperTinyEn => "sherpa-onnx-whisper-tiny.en.tar.bz2",
            Self::WhisperTiny => "sherpa-onnx-whisper-tiny.tar.bz2",
            Self::WhisperBaseEn => "sherpa-onnx-whisper-base.en.tar.bz2",
            Self::WhisperBase => "sherpa-onnx-whisper-base.tar.bz2",
            Self::WhisperSmallEn => "sherpa-onnx-whisper-small.en.tar.bz2",
            Self::WhisperSmall => "sherpa-onnx-whisper-small.tar.bz2",
            Self::WhisperMediumEn => "sherpa-onnx-whisper-medium.en.tar.bz2",
            Self::WhisperMedium => "sherpa-onnx-whisper-medium.tar.bz2",
            Self::ParakeetTdtV2Int8 => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
            Self::ParakeetTdtV3Int8 => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
            Self::ParakeetTdtCtc110mInt8 => {
                "sherpa-onnx-nemo-parakeet_tdt_ctc_110m-en-36000-int8.tar.bz2"
            }
            Self::SenseVoiceSmallInt8 => {
                "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2"
            }
            Self::MoonshineTinyEn => "sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
            Self::MoonshineBaseEn => "sherpa-onnx-moonshine-base-en-int8.tar.bz2",
            Self::MoonshineV2TinyEn => "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
            Self::MoonshineV2BaseEn => "sherpa-onnx-moonshine-base-en-quantized-2026-02-27.tar.bz2",
        }
    }

    pub fn download_url(self) -> String {
        format!("{ASR_MODELS_BASE_URL}/{}", self.archive_name())
    }

    pub fn local_dir(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.storage_name())
    }

    pub fn create_recognizer(self, model_dir: &Path) -> Result<OfflineRecognizer> {
        let mut config = OfflineRecognizerConfig::default();
        self.configure(&mut config, model_dir);

        OfflineRecognizer::create(&config).ok_or_else(|| {
            anyhow!(
                "failed to create sherpa-onnx recognizer for {}",
                self.display_name()
            )
        })
    }

    fn configure(self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        match self {
            Self::WhisperTinyEn => configure_whisper(config, model_dir, "tiny.en", Some("en")),
            Self::WhisperTiny => configure_whisper(config, model_dir, "tiny", None),
            Self::WhisperBaseEn => configure_whisper(config, model_dir, "base.en", Some("en")),
            Self::WhisperBase => configure_whisper(config, model_dir, "base", None),
            Self::WhisperSmallEn => configure_whisper(config, model_dir, "small.en", Some("en")),
            Self::WhisperSmall => configure_whisper(config, model_dir, "small", None),
            Self::WhisperMediumEn => configure_whisper(config, model_dir, "medium.en", Some("en")),
            Self::WhisperMedium => configure_whisper(config, model_dir, "medium", None),
            Self::ParakeetTdtV2Int8 | Self::ParakeetTdtV3Int8 => {
                configure_parakeet_tdt(config, model_dir)
            }
            Self::ParakeetTdtCtc110mInt8 => configure_parakeet_ctc(config, model_dir),
            Self::SenseVoiceSmallInt8 => configure_sense_voice(config, model_dir),
            Self::MoonshineTinyEn | Self::MoonshineBaseEn => {
                configure_moonshine_v1(config, model_dir)
            }
            Self::MoonshineV2TinyEn | Self::MoonshineV2BaseEn => {
                configure_moonshine_v2(config, model_dir)
            }
        }
    }
}

fn configure_whisper(
    config: &mut OfflineRecognizerConfig,
    model_dir: &Path,
    prefix: &str,
    language: Option<&str>,
) {
    config.model_config.whisper = OfflineWhisperModelConfig {
        encoder: Some(
            model_dir
                .join(format!("{prefix}-encoder.int8.onnx"))
                .to_string_lossy()
                .to_string(),
        ),
        decoder: Some(
            model_dir
                .join(format!("{prefix}-decoder.int8.onnx"))
                .to_string_lossy()
                .to_string(),
        ),
        language: language.map(str::to_string),
        task: Some("transcribe".to_string()),
        ..Default::default()
    };
    config.model_config.tokens = Some(
        model_dir
            .join(format!("{prefix}-tokens.txt"))
            .to_string_lossy()
            .to_string(),
    );
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
}

fn configure_parakeet_tdt(config: &mut OfflineRecognizerConfig, model_dir: &Path) {
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: Some(
            model_dir
                .join("encoder.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        decoder: Some(
            model_dir
                .join("decoder.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        joiner: Some(
            model_dir
                .join("joiner.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
    };
    config.model_config.tokens = Some(model_dir.join("tokens.txt").to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
}

fn configure_parakeet_ctc(config: &mut OfflineRecognizerConfig, model_dir: &Path) {
    config.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig {
        model: Some(
            model_dir
                .join("model.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
    };
    config.model_config.tokens = Some(model_dir.join("tokens.txt").to_string_lossy().to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
}

fn configure_sense_voice(config: &mut OfflineRecognizerConfig, model_dir: &Path) {
    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
        model: Some(
            model_dir
                .join("model.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        language: Some("en".to_string()),
        use_itn: true,
    };
    config.model_config.tokens = Some(model_dir.join("tokens.txt").to_string_lossy().to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
}

fn configure_moonshine_v1(config: &mut OfflineRecognizerConfig, model_dir: &Path) {
    config.model_config.moonshine = OfflineMoonshineModelConfig {
        preprocessor: Some(
            model_dir
                .join("preprocess.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        encoder: Some(
            model_dir
                .join("encode.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        uncached_decoder: Some(
            model_dir
                .join("uncached_decode.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        cached_decoder: Some(
            model_dir
                .join("cached_decode.int8.onnx")
                .to_string_lossy()
                .to_string(),
        ),
        merged_decoder: None,
    };
    config.model_config.tokens = Some(model_dir.join("tokens.txt").to_string_lossy().to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
}

fn configure_moonshine_v2(config: &mut OfflineRecognizerConfig, model_dir: &Path) {
    config.model_config.moonshine = OfflineMoonshineModelConfig {
        preprocessor: None,
        encoder: Some(
            model_dir
                .join("encoder_model.ort")
                .to_string_lossy()
                .to_string(),
        ),
        uncached_decoder: None,
        cached_decoder: None,
        merged_decoder: Some(
            model_dir
                .join("decoder_model_merged.ort")
                .to_string_lossy()
                .to_string(),
        ),
    };
    config.model_config.tokens = Some(model_dir.join("tokens.txt").to_string_lossy().to_string());
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
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
