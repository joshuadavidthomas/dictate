use std::path::Path;
use std::time::Duration;

use crate::dictation::CapturedUtterance;
use crate::models::ModelCatalogEntry;
use crate::text::DictationContext;

const MIN_DICTATION_DURATION: Duration = Duration::from_millis(400);
#[derive(Clone, Debug)]
pub struct TranscriptionPlan {
    model: &'static ModelCatalogEntry,
    context: DictationContext,
}

impl TranscriptionPlan {
    #[must_use]
    pub fn new(model: &'static ModelCatalogEntry, context: DictationContext) -> Self {
        Self { model, context }
    }

    #[must_use]
    pub const fn model(&self) -> &'static ModelCatalogEntry {
        self.model
    }

    #[must_use]
    pub fn context(&self) -> &DictationContext {
        &self.context
    }
}

/// A loaded transcribe.cpp model session.
///
/// `Session::run` needs `&mut self`, so decoding takes `&mut Recognizer`:
/// each recognizer has one owning thread and no lock sits on the
/// transcription path.
pub struct Recognizer {
    session: transcribe_cpp::Session,
}

impl Recognizer {
    /// Load a GGUF model through transcribe.cpp on the best available
    /// backend (Metal on macOS, CPU elsewhere).
    pub(crate) fn from_gguf(path: &Path) -> transcribe_cpp::Result<Self> {
        // Route native diagnostics (per-decode timing lines, ggml notes) to the
        // `log` facade instead of stderr; errors still come back as `Result`s.
        static ROUTE_LOGS: std::sync::Once = std::sync::Once::new();
        ROUTE_LOGS.call_once(transcribe_cpp::init_logging);

        let model = transcribe_cpp::Model::load(path)?;
        // Two threads measured at ~0.15x real time for Bark Clean Q8_0 on a
        // laptop CPU.
        let session = model.session_with(&transcribe_cpp::SessionOptions {
            n_threads: 2,
            ..transcribe_cpp::SessionOptions::default()
        })?;
        Ok(Self { session })
    }

    fn decode(&mut self, utterance: &CapturedUtterance) -> Option<RawTranscript> {
        let transcript = match self
            .session
            .run(utterance.samples(), &transcribe_cpp::RunOptions::default())
        {
            Ok(transcript) => transcript,
            Err(error) => {
                eprintln!("transcription failed: {error}");
                return None;
            }
        };
        let text = transcript.text.trim();
        if text.is_empty() {
            None
        } else {
            Some(RawTranscript::new(text))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawTranscript {
    text: String,
}

impl RawTranscript {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptionResult {
    Transcript(RawTranscript),
    NoTranscript(TranscriptionFailure),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapturedSignalMetrics {
    duration: Duration,
    sample_count: usize,
    rms: f32,
}

impl CapturedSignalMetrics {
    #[must_use]
    pub fn measure(utterance: &CapturedUtterance) -> Self {
        Self {
            duration: utterance.duration(),
            sample_count: utterance.samples().len(),
            rms: rms(utterance.samples()),
        }
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.duration
    }

    #[must_use]
    pub const fn sample_count(self) -> usize {
        self.sample_count
    }

    #[must_use]
    pub const fn rms(self) -> f32 {
        self.rms
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TranscriptionFailure {
    TooShort(CapturedSignalMetrics),
    Empty,
    Noise,
}

impl TranscriptionFailure {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::TooShort(_) => "captured dictation was too short",
            Self::Empty => "captured dictation produced no transcript",
            Self::Noise => "captured dictation looked like non-speech noise",
        }
    }
}

#[must_use]
pub fn transcribe(
    recognizer: &mut Recognizer,
    utterance: &CapturedUtterance,
) -> TranscriptionResult {
    classify_transcript(utterance, recognizer.decode(utterance))
}

/// Classify a finished hypothesis the same way everywhere: reject clips that
/// are too short to have held speech, then reject empty or noise-like text.
#[must_use]
fn classify_transcript(
    utterance: &CapturedUtterance,
    raw: Option<RawTranscript>,
) -> TranscriptionResult {
    if let Some(metrics) = rejected_signal_metrics(utterance) {
        return TranscriptionResult::NoTranscript(TranscriptionFailure::TooShort(metrics));
    }

    let Some(raw) = raw else {
        return TranscriptionResult::NoTranscript(TranscriptionFailure::Empty);
    };

    if transcript_is_noise(raw.as_str()) {
        TranscriptionResult::NoTranscript(TranscriptionFailure::Noise)
    } else {
        TranscriptionResult::Transcript(raw)
    }
}

fn rejected_signal_metrics(utterance: &CapturedUtterance) -> Option<CapturedSignalMetrics> {
    let metrics = CapturedSignalMetrics::measure(utterance);

    (metrics.duration < MIN_DICTATION_DURATION).then_some(metrics)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let (sum_squares, sample_count) = samples.iter().fold((0.0, 0.0), |(sum, count), sample| {
        (sum + sample * sample, count + 1.0)
    });
    (sum_squares / sample_count).sqrt()
}

fn transcript_is_noise(text: &str) -> bool {
    if text.is_empty() || repeated_punctuation(text) {
        return true;
    }

    matches!(
        text.trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '(' | ')' | '.')
        })
        .to_ascii_lowercase()
        .as_str(),
        "cough" | "coughing" | "static" | "phone buzz" | "buzz" | "noise" | "music" | "laughter"
    )
}

fn repeated_punctuation(text: &str) -> bool {
    let mut chars = text.chars().filter(|character| !character.is_whitespace());
    let Some(first) = chars.next() else {
        return true;
    };

    first.is_ascii_punctuation() && chars.all(|character| character == first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::SampleRate;

    fn test_sample_rate() -> SampleRate {
        SampleRate::new(16_000).expect("test sample rate should be non-zero")
    }

    fn test_utterance(samples: Vec<f32>) -> CapturedUtterance {
        CapturedUtterance::new(test_sample_rate(), samples)
            .expect("test samples should produce an utterance")
    }

    #[test]
    fn raw_transcript_trims_only_at_decode_boundary() {
        assert_eq!(RawTranscript::new(" hello ").as_str(), " hello ");
    }

    #[test]
    fn rms_is_zero_for_empty_samples() {
        assert!(rms(&[]).abs() <= f32::EPSILON);
    }

    #[test]
    fn rms_measures_sample_energy() {
        assert!((rms(&[3.0, 4.0]) - 3.535_534).abs() <= 0.000_001);
    }

    #[test]
    fn short_utterance_reports_its_signal_metrics() {
        let short = test_utterance(vec![1.0; 100]);

        let metrics = rejected_signal_metrics(&short).expect("short input should be rejected");
        assert_eq!(
            metrics.duration(),
            Duration::from_secs_f64(100.0 / 16_000.0)
        );
        assert_eq!(metrics.sample_count(), 100);
        assert!((metrics.rms() - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn quiet_utterance_reaches_the_recognizer() {
        let quiet = test_utterance(vec![0.002_141; 47_360]);

        let metrics = CapturedSignalMetrics::measure(&quiet);
        assert_eq!(metrics.duration(), Duration::from_secs_f64(2.96));
        assert_eq!(metrics.sample_count(), 47_360);
        assert!((metrics.rms() - 0.002_141).abs() <= 0.000_001);
        assert!(rejected_signal_metrics(&quiet).is_none());
    }

    #[test]
    fn transcript_noise_filters_asr_junk() {
        assert!(transcript_is_noise("..."));
        assert!(transcript_is_noise("(cough)"));
        assert!(transcript_is_noise("music"));
        assert!(!transcript_is_noise("ship this please"));
    }

    #[test]
    fn transcript_noise_filters_punctuated_asr_junk() {
        // The default Parakeet model attaches a sentence-final '.' to
        // single-token hypotheses; the filter must strip it before matching
        // the blocklist so punctuated noise does not reach the user.
        assert!(transcript_is_noise("cough."));
        assert!(transcript_is_noise("Cough."));
        assert!(transcript_is_noise("music."));
        assert!(transcript_is_noise("Laughter."));
        assert!(transcript_is_noise("(cough)."));
        assert!(transcript_is_noise("\tnoise.\n"));
        // Non-blocklist words with the same trailing '.' are not noise.
        assert!(!transcript_is_noise("hello."));
        assert!(!transcript_is_noise("ship this please."));
    }

    #[test]
    fn noise_word_with_trailing_period_is_noise_at_classify_boundary() {
        // 2.96 s, passes `rejected_signal_metrics`, so the noise filter is
        // the only remaining gate that can return NoTranscript here.
        let utterance = test_utterance(vec![0.002_141; 47_360]);

        let result = classify_transcript(&utterance, Some(RawTranscript::new("cough.")));
        assert!(
            matches!(
                result,
                TranscriptionResult::NoTranscript(TranscriptionFailure::Noise)
            ),
            "'cough.' should be classified as Noise, got {result:?}"
        );

        let result_cap = classify_transcript(&utterance, Some(RawTranscript::new("Cough.")));
        assert!(
            matches!(
                result_cap,
                TranscriptionResult::NoTranscript(TranscriptionFailure::Noise)
            ),
            "'Cough.' should be classified as Noise, got {result_cap:?}"
        );
    }
}
