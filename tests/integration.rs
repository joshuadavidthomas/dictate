use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use dictate::models;
use dictate::transcription::TranscriptionResult;

const MAX_WORD_ERROR_RATE: f64 = 0.08;
const MAX_CHARACTER_ERROR_RATE: f64 = 0.03;

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
            edit_distance as f64 / reference_len as f64
        };

        Self {
            edit_distance,
            reference_len,
            rate,
        }
    }
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
fn committed_corpus_meets_transcription_thresholds() -> Result<()> {
    let fixtures = discover_transcription_fixtures()?;
    let model = models::default_model();
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
        let utterance = dictate::audio::load_wav_utterance(&fixture.audio)
            .with_context(|| format!("failed to load fixture {}", fixture.id))?;

        let hypothesis = match dictate::transcription::transcribe(&recognizer, &utterance) {
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

    let aggregate_wer = ErrorRate::from_counts(word_edits, word_reference_len);
    let aggregate_cer = ErrorRate::from_counts(character_edits, character_reference_len);
    let report = corpus_report(&failed_cases, &reports, aggregate_wer, aggregate_cer);

    if !failed_cases.is_empty()
        || aggregate_wer.rate > MAX_WORD_ERROR_RATE
        || aggregate_cer.rate > MAX_CHARACTER_ERROR_RATE
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

fn locate_preinstalled_default_model() -> Result<PathBuf> {
    let model = models::default_model();

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

    let model_dir = model.local_dir(&models::local_models_dir()?);
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
    aggregate_wer: ErrorRate,
    aggregate_cer: ErrorRate,
) -> String {
    let no_transcript_report = if failed_cases.is_empty() {
        "none".to_string()
    } else {
        failed_cases.join("\n")
    };

    format!(
        "aggregate WER: {aggregate_wer}\naggregate CER: {aggregate_cer}\nno-transcript failures:\n{no_transcript_report}\n\n{}",
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
        assert_eq!(rate.rate, 0.5);
    }

    #[test]
    fn character_error_rate_reports_edit_counts() {
        let rate = character_error_rate("abc", "adc");

        assert_eq!(rate.edit_distance, 1);
        assert_eq!(rate.reference_len, 3);
        assert_eq!(rate.rate, 1.0 / 3.0);
    }
}
