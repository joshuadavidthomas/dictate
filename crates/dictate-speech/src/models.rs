use std::fs;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use directories::ProjectDirs;

use crate::transcription::ComputeBackend;
use crate::transcription::Recognizer;

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

/// One GGUF file from a Hugging Face repo, pinned to a commit so the
/// download cannot change underneath a model id. transcribe.cpp runs it.
#[derive(Clone, Copy, Debug)]
pub struct ModelCatalogEntry {
    id: ModelId,
    display_name: &'static str,
    repo: &'static str,
    revision: &'static str,
    file: &'static str,
}

impl ModelCatalogEntry {
    const fn new(
        id: ModelId,
        display_name: &'static str,
        repo: &'static str,
        revision: &'static str,
        file: &'static str,
    ) -> Self {
        Self {
            id,
            display_name,
            repo,
            revision,
            file,
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
    pub const fn display_name(self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub fn local_dir(self, catalog_dir: &Path) -> PathBuf {
        catalog_dir.join(self.id.as_str())
    }

    /// Whether the model file is on disk; false when the data directory is unknown.
    #[must_use]
    pub fn is_downloaded(self) -> bool {
        local_models_dir().is_ok_and(|dir| self.local_dir(&dir).join(self.file).is_file())
    }

    /// The model's directory, downloading the file first when it is missing.
    /// `progress` hears the bytes received so far and the total, when the
    /// server sends one.
    pub fn ensure_downloaded(self, progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
        let catalog_dir = local_models_dir()?;
        let model_dir = self.local_dir(&catalog_dir);
        let model_path = model_dir.join(self.file);
        if model_path.is_file() {
            return Ok(model_dir);
        }

        fs::create_dir_all(&model_dir)?;
        // Download beside the final path and rename, so an interrupted
        // download never looks like an installed model.
        let partial_path = model_dir.join(format!("{}.partial", self.file));

        eprintln!("downloading {}...", self.display_name());
        download_file(
            &format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                self.repo, self.revision, self.file
            ),
            &partial_path,
            progress,
        )?;
        fs::rename(&partial_path, &model_path)?;
        eprintln!("{} ready", self.display_name());

        Ok(model_dir)
    }

    pub fn create_recognizer(
        self,
        model_dir: &Path,
        backend: ComputeBackend,
        threads: Option<NonZeroUsize>,
    ) -> Result<Recognizer> {
        Recognizer::from_gguf(&model_dir.join(self.file), backend, threads)
            .map_err(|error| anyhow!("failed to load {}: {error}", self.display_name))
    }
}

#[must_use]
pub fn default_model() -> &'static ModelCatalogEntry {
    &BARK_3_0_CLEAN_Q8
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

fn download_file(
    url: &str,
    output_path: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
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
            progress(downloaded, expected);

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

const BARK_3_0_CLEAN_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("bark-3.0-clean-q8"),
    "Bark 3.0 Clean Q8_0",
    "joshthomasdev/underdog-bark-3.0-clean-gguf",
    "868701fd53d62c59d759ed1d79e095118f3fa7b9",
    "underdog-bark-3.0-clean-Q8_0.gguf",
);

const BARK_3_0_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("bark-3.0-q8"),
    "Bark 3.0 Q8_0",
    "joshthomasdev/underdog-bark-3.0-gguf",
    "cee1f12dfe250983ed4987f7b4a95f4577c2cb97",
    "underdog-bark-3.0-Q8_0.gguf",
);

const BARK_3_0_PLUS_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("bark-3.0-plus-q8"),
    "Bark 3.0 Plus Q8_0",
    "joshthomasdev/underdog-bark-3.0-plus-gguf",
    "3c7c39a890cb11a5b1854ce6643dea9167e58885",
    "underdog-bark-3.0-plus-Q8_0.gguf",
);

const BARK_3_0_PRO_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("bark-3.0-pro-q8"),
    "Bark 3.0 Pro Q8_0",
    "joshthomasdev/underdog-bark-3.0-pro-gguf",
    "5c10586263806666c0a8b4905a3346a7c679803e",
    "underdog-bark-3.0-pro-Q8_0.gguf",
);

const PARAKEET_TDT_V2_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-tdt-0.6b-v2-q8"),
    "Parakeet TDT 0.6B v2 Q8_0",
    "handy-computer/parakeet-tdt-0.6b-v2-gguf",
    "58239cf6d0ecb0af921e7936c1e5e0a8dba91dbb",
    "parakeet-tdt-0.6b-v2-Q8_0.gguf",
);

const PARAKEET_TDT_V3_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-tdt-0.6b-v3-q8"),
    "Parakeet TDT 0.6B v3 Q8_0",
    "handy-computer/parakeet-tdt-0.6b-v3-gguf",
    "d8d8c32cb2017c57e5307767343e97870f8a7f53",
    "parakeet-tdt-0.6b-v3-Q8_0.gguf",
);

const PARAKEET_TDT_CTC_110M_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("parakeet-tdt-ctc-110m-q8"),
    "Parakeet TDT-CTC 110M Q8_0",
    "handy-computer/parakeet-tdt_ctc-110m-gguf",
    "1c3f0d5378fdd358905cc410a93ed9e64f52d170",
    "parakeet-tdt_ctc-110m-Q8_0.gguf",
);

const WHISPER_TINY_EN_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-tiny-en-q8"),
    "Whisper tiny.en Q8_0",
    "handy-computer/whisper-tiny.en-gguf",
    "9dd46e00ef0577e354e658c07b3ac5580e9cd211",
    "whisper-tiny.en-Q8_0.gguf",
);

const WHISPER_TINY_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-tiny-q8"),
    "Whisper tiny Q8_0",
    "handy-computer/whisper-tiny-gguf",
    "1bd61cda36bbd22c88fc5a33842498a6e1d30f45",
    "whisper-tiny-Q8_0.gguf",
);

const WHISPER_BASE_EN_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-base-en-q8"),
    "Whisper base.en Q8_0",
    "handy-computer/whisper-base.en-gguf",
    "0dd374130837de419035c77d76440aad61ca135c",
    "whisper-base.en-Q8_0.gguf",
);

const WHISPER_BASE_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-base-q8"),
    "Whisper base Q8_0",
    "handy-computer/whisper-base-gguf",
    "42fcd2077de47c36d779309e0e91b76ec620f56d",
    "whisper-base-Q8_0.gguf",
);

const WHISPER_SMALL_EN_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-small-en-q8"),
    "Whisper small.en Q8_0",
    "handy-computer/whisper-small.en-gguf",
    "ab86448a84fcc15a96a9a2216d93a8dac8427da3",
    "whisper-small.en-Q8_0.gguf",
);

const WHISPER_SMALL_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-small-q8"),
    "Whisper small Q8_0",
    "handy-computer/whisper-small-gguf",
    "4afdc7942ecf1997f359deae48a63506b062eac4",
    "whisper-small-Q8_0.gguf",
);

const WHISPER_MEDIUM_EN_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-medium-en-q8"),
    "Whisper medium.en Q8_0",
    "handy-computer/whisper-medium.en-gguf",
    "e6370f41cfa6d73e2a89969f4f5055f8c7e4dbb6",
    "whisper-medium.en-Q8_0.gguf",
);

const WHISPER_MEDIUM_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("whisper-medium-q8"),
    "Whisper medium Q8_0",
    "handy-computer/whisper-medium-gguf",
    "60cfb00405a9a5bb85ee347ef93b68df7a311dc5",
    "whisper-medium-Q8_0.gguf",
);

const SENSE_VOICE_SMALL_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("sense-voice-small-q8"),
    "SenseVoice Small Q8_0",
    "handy-computer/SenseVoiceSmall-gguf",
    "ff8f085a8fa2e5ccae0e849ef19ebc6dc18e608b",
    "SenseVoiceSmall-Q8_0.gguf",
);

const MOONSHINE_TINY_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("moonshine-tiny-q8"),
    "Moonshine tiny Q8_0",
    "handy-computer/moonshine-tiny-gguf",
    "c71a93b8f1cb2b7f7336f31bb54bc96960c16f41",
    "moonshine-tiny-Q8_0.gguf",
);

const MOONSHINE_BASE_Q8: ModelCatalogEntry = ModelCatalogEntry::new(
    ModelId::new("moonshine-base-q8"),
    "Moonshine base Q8_0",
    "handy-computer/moonshine-base-gguf",
    "124f8cf8d3c1c56ad0f6ebc44aaafc0622e62d97",
    "moonshine-base-Q8_0.gguf",
);

const MODEL_CATALOG: &[ModelCatalogEntry] = &[
    BARK_3_0_CLEAN_Q8,
    BARK_3_0_Q8,
    BARK_3_0_PLUS_Q8,
    BARK_3_0_PRO_Q8,
    PARAKEET_TDT_V2_Q8,
    PARAKEET_TDT_V3_Q8,
    PARAKEET_TDT_CTC_110M_Q8,
    WHISPER_TINY_EN_Q8,
    WHISPER_TINY_Q8,
    WHISPER_BASE_EN_Q8,
    WHISPER_BASE_Q8,
    WHISPER_SMALL_EN_Q8,
    WHISPER_SMALL_Q8,
    WHISPER_MEDIUM_EN_Q8,
    WHISPER_MEDIUM_Q8,
    SENSE_VOICE_SMALL_Q8,
    MOONSHINE_TINY_Q8,
    MOONSHINE_BASE_Q8,
];

#[cfg(test)]
mod tests {
    use super::*;

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
