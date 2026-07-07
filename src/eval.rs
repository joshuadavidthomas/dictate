use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use serde::Serialize;
use sherpa_onnx::OfflineRecognizer;

use crate::models::ModelCatalogEntry;
use crate::settings::Settings;
use crate::text::DictationContext;
use crate::text::DictationFormatter;
use crate::transcription::TranscriptionResult;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchResult {
    pub model_id: &'static str,
    pub raw: String,
    pub formatted: String,
    pub timing: BenchTiming,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BenchTiming {
    pub load_ms: f64,
    pub transcribe_ms: f64,
    pub format_ms: f64,
    pub total_ms: f64,
}

impl BenchTiming {
    fn from_parts(load: Duration, transcribe: Duration, format: Duration) -> Self {
        Self {
            load_ms: duration_ms(load),
            transcribe_ms: duration_ms(transcribe),
            format_ms: duration_ms(format),
            total_ms: duration_ms(load + transcribe + format),
        }
    }
}

pub struct TranscriptionSession {
    model: &'static ModelCatalogEntry,
    recognizer: OfflineRecognizer,
    formatter: DictationFormatter,
    context: DictationContext,
}

impl TranscriptionSession {
    pub fn new(model_override: Option<&str>) -> Result<Self> {
        let settings = crate::settings::load()?;
        Self::from_settings(settings, model_override)
    }

    pub fn from_settings(settings: Settings, model_override: Option<&str>) -> Result<Self> {
        let model = selected_model(&settings, model_override)?;
        let model_dir = model.ensure_downloaded()?;
        Self::from_model_dir(settings, model_override, &model_dir)
    }

    pub fn from_model_dir(
        settings: Settings,
        model_override: Option<&str>,
        model_dir: &Path,
    ) -> Result<Self> {
        let model = selected_model(&settings, model_override)?;
        let recognizer = model.create_recognizer(model_dir)?;

        Ok(Self {
            model,
            recognizer,
            formatter: DictationFormatter,
            context: settings.dictation_context(),
        })
    }

    pub fn model_id(&self) -> &'static str {
        self.model.id().as_str()
    }

    pub fn transcribe_file(&self, wav: &Path) -> Result<BenchResult> {
        let load_started = Instant::now();
        let utterance = crate::audio::load_wav_utterance(wav)?;
        let load_duration = load_started.elapsed();

        let transcribe_started = Instant::now();
        let result = crate::transcription::transcribe(&self.recognizer, &utterance);
        let transcribe_duration = transcribe_started.elapsed();

        let raw_transcript = match result {
            TranscriptionResult::Transcript(raw_transcript) => raw_transcript,
            TranscriptionResult::NoTranscript(failure) => bail!("{}", failure.message()),
        };

        let raw = raw_transcript.as_str().to_string();
        let format_started = Instant::now();
        let formatted = self.formatter.format(raw_transcript, &self.context);
        let format_duration = format_started.elapsed();

        Ok(BenchResult {
            model_id: self.model_id(),
            raw,
            formatted: formatted.as_str().to_string(),
            timing: BenchTiming::from_parts(load_duration, transcribe_duration, format_duration),
        })
    }
}

pub fn transcribe_file(wav: &Path, model_override: Option<&str>) -> Result<BenchResult> {
    TranscriptionSession::new(model_override)?.transcribe_file(wav)
}

fn selected_model(
    settings: &Settings,
    model_override: Option<&str>,
) -> Result<&'static ModelCatalogEntry> {
    match model_override {
        Some(model_id) => model_by_id_or_error(model_id),
        None => settings.model(),
    }
}

fn model_by_id_or_error(model_id: &str) -> Result<&'static ModelCatalogEntry> {
    crate::models::model_by_id(model_id).ok_or_else(|| {
        anyhow!(
            "unknown model id {:?}; valid model ids: {}; example: --model {}",
            model_id,
            valid_model_ids(),
            crate::models::DEFAULT_MODEL_ID.as_str()
        )
    })
}

fn valid_model_ids() -> String {
    ModelCatalogEntry::all()
        .iter()
        .map(|model| model.id().as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn bench_result_json_shape_is_stable() {
        let result = BenchResult {
            model_id: "parakeet-tdt-0.6b-v2-int8",
            raw: "hello comma world period".to_string(),
            formatted: "Hello, world.".to_string(),
            timing: BenchTiming {
                load_ms: 1.0,
                transcribe_ms: 2.5,
                format_ms: 0.25,
                total_ms: 3.75,
            },
        };

        let value: Value = serde_json::to_value(result).unwrap();

        assert_eq!(value["model_id"], "parakeet-tdt-0.6b-v2-int8");
        assert_eq!(value["raw"], "hello comma world period");
        assert_eq!(value["formatted"], "Hello, world.");
        assert_eq!(value["timing"]["load_ms"], 1.0);
        assert_eq!(value["timing"]["transcribe_ms"], 2.5);
        assert_eq!(value["timing"]["format_ms"], 0.25);
        assert_eq!(value["timing"]["total_ms"], 3.75);
    }

    #[test]
    fn invalid_model_override_reports_valid_ids_without_loading_model() {
        let error = model_by_id_or_error("not-a-model").unwrap_err().to_string();

        assert!(error.contains("not-a-model"));
        assert!(error.contains(crate::models::DEFAULT_MODEL_ID.as_str()));
    }
}
