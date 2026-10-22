use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::bail;
use serde::Serialize;

use crate::dictation::CapturedUtterance;
use crate::text::DictationFormatter;
use crate::text::ProcessedDictation;
use crate::transcription::Recognizer;
use crate::transcription::TranscriptionFailure;
use crate::transcription::TranscriptionPlan;
use crate::transcription::TranscriptionResult;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchResult {
    pub model_id: &'static str,
    /// The compute device that ran the model; `None` when the model was
    /// never needed.
    device: Option<String>,
    /// Decode threads the model ran with.
    threads: Option<usize>,
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

/// Transcribes files with a plan's model, loaded the first time a decode
/// needs it.
pub struct TranscriptionSession {
    plan: TranscriptionPlan,
    /// Where the model is; downloaded on first use when `None`.
    model_dir: Option<PathBuf>,
    recognizer: Option<Recognizer>,
}

impl TranscriptionSession {
    #[must_use]
    pub fn new(plan: TranscriptionPlan) -> Self {
        Self {
            plan,
            model_dir: None,
            recognizer: None,
        }
    }

    pub fn from_model_dir(plan: TranscriptionPlan, model_dir: &Path) -> Result<Self> {
        let mut session = Self {
            plan,
            model_dir: Some(model_dir.to_owned()),
            recognizer: None,
        };
        load_recognizer(
            &mut session.recognizer,
            &session.plan,
            session.model_dir.as_deref(),
        )?;
        Ok(session)
    }

    #[must_use]
    fn model_id(&self) -> &'static str {
        self.plan.model().id().as_str()
    }

    /// Transcribe `wav` and format it with the plan's context.
    pub fn transcribe_file(&mut self, wav: &Path) -> Result<BenchResult> {
        let context = self.plan.context().clone();
        self.transcribe_file_with(wav, |samples, decode| match decode(samples)? {
            TranscriptionResult::Transcript(raw) => {
                let formatted = DictationFormatter.format(&raw, &context);
                Ok((raw.as_str().to_owned(), formatted))
            }
            TranscriptionResult::NoTranscript(failure) => bail!("{}", failure.message()),
        })
    }

    /// Load `wav` and hand its samples to `run`, with a function that
    /// decodes samples with this session's model, loading it first if
    /// needed. `run` returns the raw and delivered text; the time it spends
    /// outside decoding is the format step, and loading the model counts as
    /// load time.
    pub fn transcribe_file_with(
        &mut self,
        wav: &Path,
        run: impl FnOnce(
            Vec<f32>,
            &mut dyn FnMut(Vec<f32>) -> Result<TranscriptionResult>,
        ) -> Result<(String, ProcessedDictation)>,
    ) -> Result<BenchResult> {
        let load_started = Instant::now();
        let utterance = crate::audio::load_wav_utterance(wav)?;
        let load_duration = load_started.elapsed();

        let rate = utterance.sample_rate();
        let (plan, model_dir) = (&self.plan, self.model_dir.as_deref());
        let slot = &mut self.recognizer;
        let mut model_load_duration = Duration::ZERO;
        let mut transcribe_duration = Duration::ZERO;
        let run_started = Instant::now();
        let (raw, formatted) = run(utterance.samples().to_vec(), &mut |samples| {
            let load_started = Instant::now();
            let recognizer = load_recognizer(slot, plan, model_dir)?;
            model_load_duration += load_started.elapsed();
            let decode_started = Instant::now();
            let result = CapturedUtterance::new(rate, samples).map_or(
                TranscriptionResult::NoTranscript(TranscriptionFailure::Empty),
                |utterance| crate::transcription::transcribe(recognizer, &utterance),
            );
            transcribe_duration += decode_started.elapsed();
            Ok(result)
        })?;
        let format_duration = run_started
            .elapsed()
            .saturating_sub(transcribe_duration + model_load_duration);
        let load_duration = load_duration + model_load_duration;

        Ok(BenchResult {
            model_id: self.model_id(),
            device: self.recognizer.as_ref().map(|r| r.device().to_owned()),
            threads: self.recognizer.as_ref().map(|r| r.threads().get()),
            raw,
            formatted: formatted.as_str().to_string(),
            timing: BenchTiming::from_parts(load_duration, transcribe_duration, format_duration),
        })
    }
}

/// The session's recognizer, loaded into `slot` on first use.
fn load_recognizer<'a>(
    slot: &'a mut Option<Recognizer>,
    plan: &TranscriptionPlan,
    model_dir: Option<&Path>,
) -> Result<&'a mut Recognizer> {
    if let Some(recognizer) = slot {
        return Ok(recognizer);
    }
    let model_dir = match model_dir {
        Some(dir) => dir.to_owned(),
        None => plan.model().ensure_downloaded()?,
    };
    let recognizer = plan
        .model()
        .create_recognizer(&model_dir, plan.backend(), plan.threads())?;
    Ok(slot.insert(recognizer))
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
            device: Some("CPU".to_string()),
            threads: Some(3),
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
        assert_eq!(value["threads"], 3);
        assert_eq!(value["raw"], "hello comma world period");
        assert_eq!(value["formatted"], "Hello, world.");
        assert_eq!(value["timing"]["load_ms"], 1.0);
        assert_eq!(value["timing"]["transcribe_ms"], 2.5);
        assert_eq!(value["timing"]["format_ms"], 0.25);
        assert_eq!(value["timing"]["total_ms"], 3.75);
    }
}
