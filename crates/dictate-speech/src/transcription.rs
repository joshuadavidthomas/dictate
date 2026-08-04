use std::time::Duration;

use sherpa_onnx::OfflineRecognizer;

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

pub struct Recognizer {
    inner: OfflineRecognizer,
}

impl Recognizer {
    pub(crate) fn from_sherpa(inner: OfflineRecognizer) -> Self {
        Self { inner }
    }

    fn decode(&self, utterance: &CapturedUtterance) -> Option<RawTranscript> {
        let stream = self.inner.create_stream();
        let sample_rate_hz =
            i32::try_from(utterance.sample_rate().as_hz()).map_or(i32::MAX, |hz| hz);
        stream.accept_waveform(sample_rate_hz, utterance.samples());
        self.inner.decode(&stream);

        let result = stream.get_result()?;
        let text = result.text.trim();
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
pub fn transcribe(recognizer: &Recognizer, utterance: &CapturedUtterance) -> TranscriptionResult {
    if let Some(metrics) = rejected_signal_metrics(utterance) {
        return TranscriptionResult::NoTranscript(TranscriptionFailure::TooShort(metrics));
    }

    let Some(raw) = recognizer.decode(utterance) else {
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
        text.trim_matches(['(', ')'])
            .trim()
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
}
