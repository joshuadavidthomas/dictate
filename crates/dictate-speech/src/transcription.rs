use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use crate::dictation::CapturedUtterance;
use crate::models::ModelCatalogEntry;
use crate::text::DictationContext;

const MIN_DICTATION_DURATION: Duration = Duration::from_millis(400);
#[derive(Clone, Debug)]
pub struct TranscriptionPlan {
    model: &'static ModelCatalogEntry,
    backend: ComputeBackend,
    threads: Option<NonZeroUsize>,
    context: DictationContext,
}

impl TranscriptionPlan {
    #[must_use]
    pub fn new(model: &'static ModelCatalogEntry, context: DictationContext) -> Self {
        Self {
            model,
            backend: ComputeBackend::default(),
            threads: None,
            context,
        }
    }

    #[must_use]
    pub fn with_backend(mut self, backend: ComputeBackend) -> Self {
        self.backend = backend;
        self
    }

    /// Decode threads; `None` uses one per CPU core but one.
    #[must_use]
    pub const fn with_threads(mut self, threads: Option<NonZeroUsize>) -> Self {
        self.threads = threads;
        self
    }

    #[must_use]
    pub const fn threads(&self) -> Option<NonZeroUsize> {
        self.threads
    }

    #[must_use]
    pub const fn model(&self) -> &'static ModelCatalogEntry {
        self.model
    }

    #[must_use]
    pub const fn backend(&self) -> ComputeBackend {
        self.backend
    }

    #[must_use]
    pub fn context(&self) -> &DictationContext {
        &self.context
    }
}

/// Where transcription runs. The names are platform-neutral so one config
/// file works on every OS: `Gpu` is the GPU backend this build compiles in
/// (Metal on macOS, Vulkan on Linux and Windows).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ComputeBackend {
    /// The best registered device, falling back to the CPU.
    #[default]
    Auto,
    /// CPU only. GPU drivers are never loaded, which keeps a broken driver
    /// out of the process.
    Cpu,
    /// The GPU backend, or a load error when no GPU device is available.
    Gpu,
}

impl ComputeBackend {
    const fn request(self) -> transcribe_cpp::Backend {
        match self {
            Self::Auto => transcribe_cpp::Backend::Auto,
            Self::Cpu => transcribe_cpp::Backend::Cpu,
            Self::Gpu if cfg!(target_os = "macos") => transcribe_cpp::Backend::Metal,
            Self::Gpu => transcribe_cpp::Backend::Vulkan,
        }
    }
}

/// A loaded transcribe.cpp model session.
///
/// `Session::run` needs `&mut self`, so decoding takes `&mut Recognizer`:
/// each recognizer has one owning thread and no lock sits on the
/// transcription path.
pub struct Recognizer {
    session: transcribe_cpp::Session,
    device: String,
    threads: NonZeroUsize,
}

/// One decode thread per core but one, leaving a core for capture, the
/// overlay, and plugins. Decode time scales almost linearly with threads
/// (Bark Clean `Q8_0`, 9.7 s clip, 4-core CPU: 2.6 s on 1, 1.4 s on 2,
/// 0.98 s on 3, 0.73 s on 4).
fn default_threads() -> NonZeroUsize {
    std::thread::available_parallelism()
        .ok()
        .and_then(|cores| NonZeroUsize::new(cores.get() - 1))
        .unwrap_or(NonZeroUsize::MIN)
}

impl Recognizer {
    /// Load a GGUF model through transcribe.cpp on `backend`.
    ///
    /// The first load fixes which backends may register for the rest of the
    /// process; a later load asking for a different set fails.
    pub(crate) fn from_gguf(
        path: &Path,
        backend: ComputeBackend,
        threads: Option<NonZeroUsize>,
    ) -> transcribe_cpp::Result<Self> {
        // Route native diagnostics (per-decode timing lines, ggml notes) to the
        // `log` facade instead of stderr; errors still come back as `Result`s.
        static ROUTE_LOGS: std::sync::Once = std::sync::Once::new();
        ROUTE_LOGS.call_once(transcribe_cpp::init_logging);

        let request = backend.request();
        transcribe_cpp::init_backends_with(
            None::<&Path>,
            transcribe_cpp::BackendMask::for_backend(request),
        )?;
        if !transcribe_cpp::backend_available(request) {
            return Err(transcribe_cpp::Error::Backend(format!(
                "no {request:?} device is available; set backend = \"auto\" or \"cpu\""
            )));
        }

        let model = transcribe_cpp::Model::load_with(
            path,
            &transcribe_cpp::ModelOptions {
                backend: request,
                device: None,
            },
        )?;
        let device = model.device().map_or_else(
            |_| model.backend(),
            |device| format!("{} ({})", device.name, device.description),
        );
        let threads = threads.unwrap_or_else(default_threads);
        let session = model.session_with(&transcribe_cpp::SessionOptions {
            n_threads: i32::try_from(threads.get()).unwrap_or(i32::MAX),
            ..transcribe_cpp::SessionOptions::default()
        })?;
        Ok(Self {
            session,
            device,
            threads,
        })
    }

    /// Decode one second of silence, so the first dictation does not pay for
    /// building the compute graph or, on a GPU, compiling shaders.
    pub fn warm_up(&mut self) {
        let silence = vec![0.0; 16_000];
        if let Err(error) = self
            .session
            .run(&silence, &transcribe_cpp::RunOptions::default())
        {
            eprintln!("transcription warm-up failed: {error}");
        }
    }

    /// Decode threads this recognizer uses.
    #[must_use]
    pub fn threads(&self) -> NonZeroUsize {
        self.threads
    }

    /// The compute device holding the model's weights, e.g.
    /// `Vulkan0 (AMD Radeon Graphics)` or `CPU (...)`.
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    fn decode(&mut self, utterance: &CapturedUtterance) -> Option<RawTranscript> {
        let transcript = match self
            .session
            .run(utterance.samples(), &transcribe_cpp::RunOptions::default())
        {
            Ok(transcript) => transcript,
            // A truncated, repeating, or aborted decode still carries the
            // text decoded before it stopped; keep it rather than lose it all.
            Err(error) => {
                let Some(partial) = error.partial() else {
                    eprintln!("transcription failed: {error}");
                    return None;
                };
                eprintln!("transcription stopped early, keeping the partial transcript: {error}");
                partial.clone()
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

impl TranscriptionResult {
    /// The transcript, or `""` with why there is none written to stderr.
    #[must_use]
    pub fn into_text(self) -> String {
        match self {
            Self::Transcript(raw) => raw.text,
            Self::NoTranscript(reason @ TranscriptionFailure::TooShort(metrics)) => {
                eprintln!(
                    "{}: duration={:.3}s, samples={}, rms={:.6}",
                    reason.message(),
                    metrics.duration().as_secs_f64(),
                    metrics.sample_count(),
                    metrics.rms(),
                );
                String::new()
            }
            Self::NoTranscript(reason) => {
                eprintln!("{}", reason.message());
                String::new()
            }
        }
    }
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
