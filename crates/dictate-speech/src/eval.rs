use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::bail;
use serde::Serialize;

use crate::text::DictationFormatter;
use crate::text::ProcessedDictation;
use crate::transcription::RawTranscript;
use crate::transcription::Recognizer;
use crate::transcription::TranscriptionPlan;
use crate::transcription::TranscriptionResult;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchResult {
    pub model_id: &'static str,
    /// The compute device that ran the model.
    device: String,
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
    plan: TranscriptionPlan,
    recognizer: Recognizer,
}

impl TranscriptionSession {
    pub fn new(plan: TranscriptionPlan) -> Result<Self> {
        let model_dir = plan.model().ensure_downloaded()?;
        Self::from_model_dir(plan, &model_dir)
    }

    pub fn from_model_dir(plan: TranscriptionPlan, model_dir: &Path) -> Result<Self> {
        let recognizer = plan.model().create_recognizer(model_dir, plan.backend())?;

        Ok(Self { plan, recognizer })
    }

    #[must_use]
    fn model_id(&self) -> &'static str {
        self.plan.model().id().as_str()
    }

    /// Transcribe `wav` and format it with the plan's context.
    pub fn transcribe_file(&mut self, wav: &Path) -> Result<BenchResult> {
        let context = self.plan.context().clone();
        self.transcribe_file_with(wav, |raw| DictationFormatter.format(raw, &context))
    }

    /// Transcribe `wav` and turn the raw transcript into delivered text with
    /// `process`, which is timed as the format step.
    pub fn transcribe_file_with(
        &mut self,
        wav: &Path,
        process: impl FnOnce(&RawTranscript) -> ProcessedDictation,
    ) -> Result<BenchResult> {
        let load_started = Instant::now();
        let utterance = crate::audio::load_wav_utterance(wav)?;
        let load_duration = load_started.elapsed();

        let transcribe_started = Instant::now();
        let result = crate::transcription::transcribe(&mut self.recognizer, &utterance);
        let transcribe_duration = transcribe_started.elapsed();

        let raw_transcript = match result {
            TranscriptionResult::Transcript(raw_transcript) => raw_transcript,
            TranscriptionResult::NoTranscript(failure) => bail!("{}", failure.message()),
        };

        let raw = raw_transcript.as_str().to_string();
        let format_started = Instant::now();
        let formatted = process(&raw_transcript);
        let format_duration = format_started.elapsed();

        Ok(BenchResult {
            model_id: self.model_id(),
            device: self.recognizer.device().to_owned(),
            raw,
            formatted: formatted.as_str().to_string(),
            timing: BenchTiming::from_parts(load_duration, transcribe_duration, format_duration),
        })
    }
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
            model_id: "bark-3.0-clean-q8",
            device: "CPU".to_string(),
            raw: "hello comma world period".to_string(),
            formatted: "Hello, world.".to_string(),
            timing: BenchTiming {
                load_ms: 1.0,
                transcribe_ms: 2.5,
                format_ms: 0.25,
                total_ms: 3.75,
            },
        };

        let value: Value = serde_json::to_value(result).expect("bench result should serialize");

        assert_eq!(value["model_id"], "bark-3.0-clean-q8");
        assert_eq!(value["device"], "CPU");
        assert_eq!(value["raw"], "hello comma world period");
        assert_eq!(value["formatted"], "Hello, world.");
        assert_eq!(value["timing"]["load_ms"], 1.0);
        assert_eq!(value["timing"]["transcribe_ms"], 2.5);
        assert_eq!(value["timing"]["format_ms"], 0.25);
        assert_eq!(value["timing"]["total_ms"], 3.75);
    }
}
