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
        self.spec().storage_name
    }

    pub fn display_name(self) -> &'static str {
        self.spec().display_name
    }

    pub fn archive_name(self) -> &'static str {
        self.spec().archive_name
    }

    pub fn download_url(self) -> String {
        format!("{ASR_MODELS_BASE_URL}/{}", self.archive_name())
    }

    pub fn local_dir(self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.storage_name())
    }

    pub fn expected_files(self) -> Vec<String> {
        self.spec().family.expected_files()
    }

    pub fn create_recognizer(self, model_dir: &Path) -> Result<OfflineRecognizer> {
        let mut config = OfflineRecognizerConfig::default();
        self.spec().family.configure(&mut config, model_dir);

        OfflineRecognizer::create(&config).ok_or_else(|| {
            anyhow!(
                "failed to create sherpa-onnx recognizer for {}",
                self.display_name()
            )
        })
    }

    fn spec(self) -> ModelSpec {
        match self {
            Self::WhisperTinyEn => ModelSpec {
                storage_name: "whisper-tiny-en",
                display_name: "Whisper tiny.en",
                archive_name: "sherpa-onnx-whisper-tiny.en.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "tiny.en",
                    language: Some("en"),
                }),
            },
            Self::WhisperTiny => ModelSpec {
                storage_name: "whisper-tiny",
                display_name: "Whisper tiny",
                archive_name: "sherpa-onnx-whisper-tiny.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "tiny",
                    language: None,
                }),
            },
            Self::WhisperBaseEn => ModelSpec {
                storage_name: "whisper-base-en",
                display_name: "Whisper base.en",
                archive_name: "sherpa-onnx-whisper-base.en.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "base.en",
                    language: Some("en"),
                }),
            },
            Self::WhisperBase => ModelSpec {
                storage_name: "whisper-base",
                display_name: "Whisper base",
                archive_name: "sherpa-onnx-whisper-base.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "base",
                    language: None,
                }),
            },
            Self::WhisperSmallEn => ModelSpec {
                storage_name: "whisper-small-en",
                display_name: "Whisper small.en",
                archive_name: "sherpa-onnx-whisper-small.en.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "small.en",
                    language: Some("en"),
                }),
            },
            Self::WhisperSmall => ModelSpec {
                storage_name: "whisper-small",
                display_name: "Whisper small",
                archive_name: "sherpa-onnx-whisper-small.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "small",
                    language: None,
                }),
            },
            Self::WhisperMediumEn => ModelSpec {
                storage_name: "whisper-medium-en",
                display_name: "Whisper medium.en",
                archive_name: "sherpa-onnx-whisper-medium.en.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "medium.en",
                    language: Some("en"),
                }),
            },
            Self::WhisperMedium => ModelSpec {
                storage_name: "whisper-medium",
                display_name: "Whisper medium",
                archive_name: "sherpa-onnx-whisper-medium.tar.bz2",
                family: Family::Whisper(WhisperFamily {
                    prefix: "medium",
                    language: None,
                }),
            },
            Self::ParakeetTdtV2Int8 => ModelSpec {
                storage_name: "parakeet-tdt-0.6b-v2-int8",
                display_name: "Parakeet TDT 0.6B v2 int8",
                archive_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
                family: Family::NemoTransducer(NemoTransducerFamily),
            },
            Self::ParakeetTdtV3Int8 => ModelSpec {
                storage_name: "parakeet-tdt-0.6b-v3-int8",
                display_name: "Parakeet TDT 0.6B v3 int8",
                archive_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
                family: Family::NemoTransducer(NemoTransducerFamily),
            },
            Self::ParakeetTdtCtc110mInt8 => ModelSpec {
                storage_name: "parakeet-tdt-ctc-110m-int8",
                display_name: "Parakeet TDT-CTC 110M int8",
                archive_name: "sherpa-onnx-nemo-parakeet_tdt_ctc_110m-en-36000-int8.tar.bz2",
                family: Family::NemoCtc(NemoCtcFamily),
            },
            Self::SenseVoiceSmallInt8 => ModelSpec {
                storage_name: "sense-voice-small-int8",
                display_name: "SenseVoice Small int8",
                archive_name: "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2",
                family: Family::SenseVoice(SenseVoiceFamily {
                    language: "en",
                    use_itn: true,
                }),
            },
            Self::MoonshineTinyEn => ModelSpec {
                storage_name: "moonshine-tiny-en",
                display_name: "Moonshine Tiny English",
                archive_name: "sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
                family: Family::MoonshineV1(MoonshineV1Family),
            },
            Self::MoonshineBaseEn => ModelSpec {
                storage_name: "moonshine-base-en",
                display_name: "Moonshine Base English",
                archive_name: "sherpa-onnx-moonshine-base-en-int8.tar.bz2",
                family: Family::MoonshineV1(MoonshineV1Family),
            },
            Self::MoonshineV2TinyEn => ModelSpec {
                storage_name: "moonshine-v2-tiny-en",
                display_name: "Moonshine v2 Tiny English",
                archive_name: "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
                family: Family::MoonshineV2(MoonshineV2Family),
            },
            Self::MoonshineV2BaseEn => ModelSpec {
                storage_name: "moonshine-v2-base-en",
                display_name: "Moonshine v2 Base English",
                archive_name: "sherpa-onnx-moonshine-base-en-quantized-2026-02-27.tar.bz2",
                family: Family::MoonshineV2(MoonshineV2Family),
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ModelSpec {
    storage_name: &'static str,
    display_name: &'static str,
    archive_name: &'static str,
    family: Family,
}

trait ModelFamily {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path);
    fn expected_files(&self) -> Vec<String>;
}

#[derive(Clone, Copy, Debug)]
enum Family {
    Whisper(WhisperFamily),
    NemoTransducer(NemoTransducerFamily),
    NemoCtc(NemoCtcFamily),
    SenseVoice(SenseVoiceFamily),
    MoonshineV1(MoonshineV1Family),
    MoonshineV2(MoonshineV2Family),
}

impl ModelFamily for Family {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        match self {
            Self::Whisper(family) => family.configure(config, model_dir),
            Self::NemoTransducer(family) => family.configure(config, model_dir),
            Self::NemoCtc(family) => family.configure(config, model_dir),
            Self::SenseVoice(family) => family.configure(config, model_dir),
            Self::MoonshineV1(family) => family.configure(config, model_dir),
            Self::MoonshineV2(family) => family.configure(config, model_dir),
        }
    }

    fn expected_files(&self) -> Vec<String> {
        match self {
            Self::Whisper(family) => family.expected_files(),
            Self::NemoTransducer(family) => family.expected_files(),
            Self::NemoCtc(family) => family.expected_files(),
            Self::SenseVoice(family) => family.expected_files(),
            Self::MoonshineV1(family) => family.expected_files(),
            Self::MoonshineV2(family) => family.expected_files(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct WhisperFamily {
    prefix: &'static str,
    language: Option<&'static str>,
}

impl ModelFamily for WhisperFamily {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        config.model_config.whisper = OfflineWhisperModelConfig {
            encoder: Some(model_file(
                model_dir,
                &format!("{}-encoder.int8.onnx", self.prefix),
            )),
            decoder: Some(model_file(
                model_dir,
                &format!("{}-decoder.int8.onnx", self.prefix),
            )),
            language: self.language.map(str::to_string),
            task: Some("transcribe".to_string()),
            ..Default::default()
        };
        config.model_config.tokens = Some(model_file(
            model_dir,
            &format!("{}-tokens.txt", self.prefix),
        ));
        configure_cpu_defaults(config);
    }

    fn expected_files(&self) -> Vec<String> {
        vec![
            format!("{}-encoder.int8.onnx", self.prefix),
            format!("{}-decoder.int8.onnx", self.prefix),
            format!("{}-tokens.txt", self.prefix),
        ]
    }
}

#[derive(Clone, Copy, Debug)]
struct NemoTransducerFamily;

impl ModelFamily for NemoTransducerFamily {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(model_file(model_dir, "encoder.int8.onnx")),
            decoder: Some(model_file(model_dir, "decoder.int8.onnx")),
            joiner: Some(model_file(model_dir, "joiner.int8.onnx")),
        };
        config.model_config.tokens = Some(model_file(model_dir, "tokens.txt"));
        config.model_config.model_type = Some("nemo_transducer".to_string());
        configure_cpu_defaults(config);
    }

    fn expected_files(&self) -> Vec<String> {
        file_names(&[
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ])
    }
}

#[derive(Clone, Copy, Debug)]
struct NemoCtcFamily;

impl ModelFamily for NemoCtcFamily {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        config.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig {
            model: Some(model_file(model_dir, "model.int8.onnx")),
        };
        config.model_config.tokens = Some(model_file(model_dir, "tokens.txt"));
        configure_cpu_defaults(config);
    }

    fn expected_files(&self) -> Vec<String> {
        file_names(&["model.int8.onnx", "tokens.txt"])
    }
}

#[derive(Clone, Copy, Debug)]
struct SenseVoiceFamily {
    language: &'static str,
    use_itn: bool,
}

impl ModelFamily for SenseVoiceFamily {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model_file(model_dir, "model.int8.onnx")),
            language: Some(self.language.to_string()),
            use_itn: self.use_itn,
        };
        config.model_config.tokens = Some(model_file(model_dir, "tokens.txt"));
        configure_cpu_defaults(config);
    }

    fn expected_files(&self) -> Vec<String> {
        file_names(&["model.int8.onnx", "tokens.txt"])
    }
}

#[derive(Clone, Copy, Debug)]
struct MoonshineV1Family;

impl ModelFamily for MoonshineV1Family {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        config.model_config.moonshine = OfflineMoonshineModelConfig {
            preprocessor: Some(model_file(model_dir, "preprocess.onnx")),
            encoder: Some(model_file(model_dir, "encode.int8.onnx")),
            uncached_decoder: Some(model_file(model_dir, "uncached_decode.int8.onnx")),
            cached_decoder: Some(model_file(model_dir, "cached_decode.int8.onnx")),
            merged_decoder: None,
        };
        config.model_config.tokens = Some(model_file(model_dir, "tokens.txt"));
        configure_cpu_defaults(config);
    }

    fn expected_files(&self) -> Vec<String> {
        file_names(&[
            "preprocess.onnx",
            "encode.int8.onnx",
            "uncached_decode.int8.onnx",
            "cached_decode.int8.onnx",
            "tokens.txt",
        ])
    }
}

#[derive(Clone, Copy, Debug)]
struct MoonshineV2Family;

impl ModelFamily for MoonshineV2Family {
    fn configure(&self, config: &mut OfflineRecognizerConfig, model_dir: &Path) {
        config.model_config.moonshine = OfflineMoonshineModelConfig {
            preprocessor: None,
            encoder: Some(model_file(model_dir, "encoder_model.ort")),
            uncached_decoder: None,
            cached_decoder: None,
            merged_decoder: Some(model_file(model_dir, "decoder_model_merged.ort")),
        };
        config.model_config.tokens = Some(model_file(model_dir, "tokens.txt"));
        configure_cpu_defaults(config);
    }

    fn expected_files(&self) -> Vec<String> {
        file_names(&[
            "encoder_model.ort",
            "decoder_model_merged.ort",
            "tokens.txt",
        ])
    }
}

fn configure_cpu_defaults(config: &mut OfflineRecognizerConfig) {
    config.model_config.num_threads = 2;
    config.model_config.provider = Some("cpu".to_string());
}

fn model_file(model_dir: &Path, file_name: &str) -> String {
    model_dir.join(file_name).to_string_lossy().to_string()
}

fn file_names(file_names: &[&str]) -> Vec<String> {
    file_names
        .iter()
        .map(|file_name| file_name.to_string())
        .collect()
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
