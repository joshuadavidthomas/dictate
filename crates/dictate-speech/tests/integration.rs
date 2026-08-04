use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use dictate_speech::CapturedUtterance;
use dictate_speech::DictationContext;
use dictate_speech::TranscriptionPlan;
use dictate_speech::TranscriptionResult;
use dictate_speech::default_model;
use dictate_speech::load_wav_utterance;
use dictate_speech::local_models_dir;
use dictate_speech::transcribe;

const MAX_WORD_ERROR_RATE: f64 = 0.08;
const MAX_CHARACTER_ERROR_RATE: f64 = 0.03;
const DEGRADATION_NOISE_SEED: u64 = 0x0d1c_7a7e_55ee_d001;

#[derive(Clone, Copy, Debug)]
enum Degradation {
    Gain(f32),
    Noise { snr_db: f64, seed: u64 },
}

impl Degradation {
    fn apply(self, samples: &[f32]) -> Vec<f32> {
        match self {
            Self::Gain(factor) => scale_gain(samples, factor),
            Self::Noise { snr_db, seed } => add_noise(samples, snr_db, seed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DegradationRow {
    id: &'static str,
    transform: Degradation,
    max_word_error_rate: f64,
    max_no_transcripts: usize,
}

const DEGRADATION_ROWS: [DegradationRow; 4] = [
    DegradationRow {
        id: "gain_x0_02",
        transform: Degradation::Gain(0.02),
        max_word_error_rate: MAX_WORD_ERROR_RATE,
        max_no_transcripts: 0,
    },
    DegradationRow {
        id: "gain_x0_005",
        transform: Degradation::Gain(0.005),
        max_word_error_rate: MAX_WORD_ERROR_RATE,
        // Baseline: LJ001-0002 returns no transcript with parakeet-tdt-0.6b-v2-int8.
        max_no_transcripts: 1,
    },
    DegradationRow {
        id: "noise_snr10",
        transform: Degradation::Noise {
            snr_db: 10.0,
            seed: DEGRADATION_NOISE_SEED,
        },
        // Baseline: 2.59% WER with parakeet-tdt-0.6b-v2-int8.
        max_word_error_rate: 0.04,
        max_no_transcripts: 0,
    },
    DegradationRow {
        id: "noise_snr0",
        transform: Degradation::Noise {
            snr_db: 0.0,
            seed: DEGRADATION_NOISE_SEED,
        },
        // Baseline: 4.31% WER with parakeet-tdt-0.6b-v2-int8.
        max_word_error_rate: 0.07,
        max_no_transcripts: 0,
    },
];

#[derive(Debug)]
struct TranscriptionFixture {
    id: String,
    audio: PathBuf,
    reference: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ErrorRate {
    edit_distance: usize,
    reference_len: usize,
    rate: f64,
}

impl ErrorRate {
    fn from_counts(edit_distance: usize, reference_len: usize) -> Self {
        let rate = if reference_len == 0 {
            if edit_distance == 0 { 0.0 } else { 1.0 }
        } else {
            usize_to_f64(edit_distance) / usize_to_f64(reference_len)
        };

        Self {
            edit_distance,
            reference_len,
            rate,
        }
    }
}

fn usize_to_f64(value: usize) -> f64 {
    u32::try_from(value).map_or(f64::from(u32::MAX), f64::from)
}

impl std::fmt::Display for ErrorRate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:.2}% ({}/{})",
            self.rate * 100.0,
            self.edit_distance,
            self.reference_len
        )
    }
}

#[test]
fn eval_transcribes_fixture_with_preinstalled_default_model() -> Result<()> {
    let model_dir = locate_preinstalled_default_model()?;
    let plan = TranscriptionPlan::new(default_model(), DictationContext::default());
    let session = dictate_speech::TranscriptionSession::from_model_dir(plan, &model_dir)
        .with_context(|| format!("failed to load model from {}", model_dir.display()))?;
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spoken-commands/clip-a.wav");

    let result = session.transcribe_file(&fixture)?;

    if result.raw.trim().is_empty() {
        bail!("raw transcript should not be empty");
    }
    if result.formatted.trim().is_empty() {
        bail!("formatted transcript should not be empty");
    }
    if result.timing.total_ms <= 0.0 {
        bail!("total transcription timing should be positive");
    }
    if result.timing.transcribe_ms <= 0.0 {
        bail!("transcription timing should be positive");
    }

    Ok(())
}

#[test]
fn committed_corpus_meets_transcription_thresholds() -> Result<()> {
    let fixtures = discover_transcription_fixtures()?;
    let model = default_model();
    let model_dir = locate_preinstalled_default_model()?;
    let recognizer = model
        .create_recognizer(&model_dir)
        .with_context(|| format!("failed to load model from {}", model_dir.display()))?;

    let mut reports = Vec::new();
    let mut word_edits = 0;
    let mut word_reference_len = 0;
    let mut character_edits = 0;
    let mut character_reference_len = 0;
    let mut failed_cases = Vec::new();

    for fixture in fixtures {
        let utterance = load_wav_utterance(&fixture.audio)
            .with_context(|| format!("failed to load fixture {}", fixture.id))?;

        let hypothesis = match transcribe(&recognizer, &utterance) {
            TranscriptionResult::Transcript(raw) => {
                let snapshot_name = fixture.id.trim_end_matches(".wav").replace('/', "__");
                insta::assert_snapshot!(snapshot_name, raw.as_str());
                raw.as_str().to_string()
            }
            TranscriptionResult::NoTranscript(reason) => {
                failed_cases.push(format!(
                    "{} produced no transcript: {}",
                    fixture.id,
                    reason.message()
                ));
                continue;
            }
        };

        let wer = word_error_rate(&fixture.reference, &hypothesis);
        let cer = character_error_rate(&fixture.reference, &hypothesis);

        word_edits += wer.edit_distance;
        word_reference_len += wer.reference_len;
        character_edits += cer.edit_distance;
        character_reference_len += cer.reference_len;

        reports.push(case_report(&fixture, &hypothesis, wer, cer));
    }

    let aggregate_word_rate = ErrorRate::from_counts(word_edits, word_reference_len);
    let aggregate_character_rate = ErrorRate::from_counts(character_edits, character_reference_len);
    let report = corpus_report(
        &failed_cases,
        &reports,
        aggregate_word_rate,
        aggregate_character_rate,
    );

    if !failed_cases.is_empty()
        || aggregate_word_rate.rate > MAX_WORD_ERROR_RATE
        || aggregate_character_rate.rate > MAX_CHARACTER_ERROR_RATE
    {
        bail!(
            "transcription corpus quality below threshold\n\
             max WER: {:.2}%\n\
             max CER: {:.2}%\n\
             {report}",
            MAX_WORD_ERROR_RATE * 100.0,
            MAX_CHARACTER_ERROR_RATE * 100.0,
        );
    }

    Ok(())
}

#[test]
fn degraded_corpus_meets_transcription_thresholds() -> Result<()> {
    let fixtures = discover_transcription_fixtures()?;
    let model = default_model();
    let model_dir = locate_preinstalled_default_model()?;
    let recognizer = model
        .create_recognizer(&model_dir)
        .with_context(|| format!("failed to load model from {}", model_dir.display()))?;
    let mut failed_rows = Vec::new();
    let mut row_reports = Vec::new();

    for row in DEGRADATION_ROWS {
        let mut reports = Vec::new();
        let mut word_edits = 0;
        let mut word_reference_len = 0;
        let mut character_edits = 0;
        let mut character_reference_len = 0;
        let mut failed_cases = Vec::new();

        for fixture in &fixtures {
            let utterance = load_wav_utterance(&fixture.audio)
                .with_context(|| format!("failed to load fixture {}", fixture.id))?;
            let degraded = CapturedUtterance::new(
                utterance.sample_rate(),
                row.transform.apply(utterance.samples()),
            )
            .with_context(|| format!("degradation {} emptied fixture {}", row.id, fixture.id))?;

            let hypothesis = match transcribe(&recognizer, &degraded) {
                TranscriptionResult::Transcript(raw) => raw.as_str().to_string(),
                TranscriptionResult::NoTranscript(reason) => {
                    failed_cases.push(format!(
                        "{} produced no transcript: {}",
                        fixture.id,
                        reason.message()
                    ));
                    String::new()
                }
            };

            let wer = word_error_rate(&fixture.reference, &hypothesis);
            let cer = character_error_rate(&fixture.reference, &hypothesis);

            word_edits += wer.edit_distance;
            word_reference_len += wer.reference_len;
            character_edits += cer.edit_distance;
            character_reference_len += cer.reference_len;

            reports.push(case_report(fixture, &hypothesis, wer, cer));
        }

        let aggregate_word_rate = ErrorRate::from_counts(word_edits, word_reference_len);
        let aggregate_character_rate =
            ErrorRate::from_counts(character_edits, character_reference_len);
        let report = corpus_report(
            &failed_cases,
            &reports,
            aggregate_word_rate,
            aggregate_character_rate,
        );

        row_reports.push(format!(
            "row: {}\nmax WER: {:.2}%\n{report}",
            row.id,
            row.max_word_error_rate * 100.0
        ));

        if failed_cases.len() > row.max_no_transcripts
            || aggregate_word_rate.rate > row.max_word_error_rate
        {
            failed_rows.push(format!(
                "{}: WER {}, CER {}, max WER {:.2}%, no-transcript failures: {}/{} allowed",
                row.id,
                aggregate_word_rate,
                aggregate_character_rate,
                row.max_word_error_rate * 100.0,
                failed_cases.len(),
                row.max_no_transcripts,
            ));
        }
    }

    if !failed_rows.is_empty() {
        bail!(
            "degraded transcription rows below threshold\n{}\n\n{}",
            failed_rows.join("\n"),
            row_reports.join("\n\n")
        );
    }

    Ok(())
}

fn scale_gain(samples: &[f32], factor: f32) -> Vec<f32> {
    samples.iter().map(|sample| sample * factor).collect()
}

fn add_noise(samples: &[f32], snr_db: f64, mut state: u64) -> Vec<f32> {
    let signal_rms = sample_rms(samples);
    let noise_rms = signal_rms / 10_f64.powf(snr_db / 20.0);

    samples
        .iter()
        .map(|sample| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let uniform = f64::from((state >> 33) as u32) / f64::from(1_u32 << 31) - 1.0;
            #[allow(clippy::cast_possible_truncation)]
            let noise_sample = (uniform * noise_rms * 3_f64.sqrt()) as f32;
            sample + noise_sample
        })
        .collect()
}

fn sample_rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let mean_square = samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / usize_to_f64(samples.len());
    mean_square.sqrt()
}

fn locate_preinstalled_default_model() -> Result<PathBuf> {
    let model = default_model();

    if let Some(model_dir) = std::env::var_os("DICTATE_MODEL_DIR") {
        let model_dir = PathBuf::from(model_dir);
        if model_dir.is_dir() {
            return Ok(model_dir);
        }

        bail!(
            "DICTATE_MODEL_DIR={} does not exist or is not a directory; point it at a preinstalled {} model directory and rerun `just test-integration`",
            model_dir.display(),
            model.id().as_str()
        );
    }

    let model_dir = model.local_dir(&local_models_dir()?);
    if model_dir.is_dir() {
        return Ok(model_dir);
    }

    bail!(
        "model {} is not installed at {}; start `dictate daemon` once to download the default model, or set DICTATE_MODEL_DIR=/path/to/{} before running `just test-integration`",
        model.id().as_str(),
        model_dir.display(),
        model.id().as_str()
    )
}

fn discover_transcription_fixtures() -> Result<Vec<TranscriptionFixture>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut fixtures = Vec::new();

    collect_wav_fixtures(&root, &root, &mut fixtures)?;
    fixtures.sort_by(|left, right| left.id.cmp(&right.id));

    if fixtures.is_empty() {
        bail!(
            "no transcription WAV fixtures found under {}",
            root.display()
        );
    }

    Ok(fixtures)
}

fn collect_wav_fixtures(
    root: &Path,
    directory: &Path,
    fixtures: &mut Vec<TranscriptionFixture>,
) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("failed to read fixture directory {}", directory.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to read fixture directory entry under {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type for {}", path.display()))?;

        if file_type.is_dir() {
            collect_wav_fixtures(root, &path, fixtures)?;
            continue;
        }

        if !is_wav_path(&path) {
            continue;
        }

        let transcript = path.with_extension("txt");
        if !transcript.is_file() {
            bail!(
                "transcription fixture {} is missing sibling transcript {}",
                path.display(),
                transcript.display()
            );
        }

        let reference = fs::read_to_string(&transcript)
            .with_context(|| format!("failed to read transcript {}", transcript.display()))?
            .trim()
            .to_string();
        if reference.is_empty() {
            bail!(
                "transcription fixture {} has an empty transcript",
                path.display()
            );
        }

        let id = path
            .strip_prefix(root)
            .with_context(|| {
                format!(
                    "fixture path {} was not under {}",
                    path.display(),
                    root.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");

        fixtures.push(TranscriptionFixture {
            id,
            audio: path,
            reference,
        });
    }

    Ok(())
}

fn is_wav_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
}

fn word_error_rate(reference: &str, hypothesis: &str) -> ErrorRate {
    let reference = normalize_for_asr_score(reference);
    let hypothesis = normalize_for_asr_score(hypothesis);
    let reference_words = reference.split_whitespace().collect::<Vec<_>>();
    let hypothesis_words = hypothesis.split_whitespace().collect::<Vec<_>>();
    let edit_distance = strsim::generic_levenshtein(&reference_words, &hypothesis_words);

    ErrorRate::from_counts(edit_distance, reference_words.len())
}

fn character_error_rate(reference: &str, hypothesis: &str) -> ErrorRate {
    let reference = normalize_for_asr_score(reference);
    let hypothesis = normalize_for_asr_score(hypothesis);
    let reference_chars = reference.chars().collect::<Vec<_>>();
    let hypothesis_chars = hypothesis.chars().collect::<Vec<_>>();
    let edit_distance = strsim::generic_levenshtein(&reference_chars, &hypothesis_chars);

    ErrorRate::from_counts(edit_distance, reference_chars.len())
}

fn normalize_for_asr_score(text: &str) -> String {
    let without_punctuation: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect();

    without_punctuation
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn corpus_report(
    failed_cases: &[String],
    reports: &[String],
    aggregate_word_rate: ErrorRate,
    aggregate_character_rate: ErrorRate,
) -> String {
    let no_transcript_report = if failed_cases.is_empty() {
        "none".to_string()
    } else {
        failed_cases.join("\n")
    };

    format!(
        "aggregate WER: {aggregate_word_rate}\naggregate CER: {aggregate_character_rate}\nno-transcript failures:\n{no_transcript_report}\n\n{}",
        reports.join("\n\n")
    )
}

fn case_report(
    fixture: &TranscriptionFixture,
    hypothesis: &str,
    wer: ErrorRate,
    cer: ErrorRate,
) -> String {
    format!(
        "case: {}\nreference: {}\nhypothesis: {}\nnormalized reference: {}\nnormalized hypothesis: {}\nWER: {wer}\nCER: {cer}",
        fixture.id,
        fixture.reference,
        hypothesis,
        normalize_for_asr_score(&fixture.reference),
        normalize_for_asr_score(hypothesis)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_punctuation_and_whitespace() {
        assert_eq!(
            normalize_for_asr_score("  Hello, WORLD!\nNew-line.  "),
            "hello world new line"
        );
    }

    #[test]
    fn word_error_rate_reports_edit_counts() {
        let rate = word_error_rate("hello world", "hello there world");

        assert_eq!(rate.edit_distance, 1);
        assert_eq!(rate.reference_len, 2);
        assert!((rate.rate - 0.5).abs() <= f64::EPSILON);
    }

    #[test]
    fn character_error_rate_reports_edit_counts() {
        let rate = character_error_rate("abc", "adc");

        assert_eq!(rate.edit_distance, 1);
        assert_eq!(rate.reference_len, 3);
        assert!((rate.rate - (1.0 / 3.0)).abs() <= f64::EPSILON);
    }

    #[test]
    fn degradation_transforms_are_deterministic_and_hit_rms_targets() {
        let samples = (0..32_000)
            .map(|index| if index % 2 == 0 { 0.25 } else { -0.25 })
            .collect::<Vec<_>>();

        let scaled = scale_gain(&samples, 0.02);
        let expected_scaled_rms = sample_rms(&samples) * 0.02;
        assert_rms_within_five_percent(sample_rms(&scaled), expected_scaled_rms);

        let first = add_noise(&samples, 10.0, DEGRADATION_NOISE_SEED);
        let second = add_noise(&samples, 10.0, DEGRADATION_NOISE_SEED);
        assert_eq!(first, second);

        let noise = first
            .iter()
            .zip(&samples)
            .map(|(degraded, clean)| degraded - clean)
            .collect::<Vec<_>>();
        let expected_noise_rms = sample_rms(&samples) / 10_f64.powf(10.0 / 20.0);
        assert_rms_within_five_percent(sample_rms(&noise), expected_noise_rms);
    }

    fn assert_rms_within_five_percent(actual: f64, expected: f64) {
        let relative_error = (actual - expected).abs() / expected;
        assert!(
            relative_error <= 0.05,
            "RMS {actual} differs from target {expected} by {:.2}%",
            relative_error * 100.0
        );
    }
}
