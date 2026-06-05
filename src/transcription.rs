use std::time::Duration;

use sherpa_onnx::OfflineRecognizer;

use crate::dictation::CapturedUtterance;
use crate::text::RawTranscript;

const MIN_DICTATION_DURATION: Duration = Duration::from_millis(400);
const MIN_DICTATION_RMS: f32 = 0.01;

pub(crate) fn too_short_or_quiet(utterance: &CapturedUtterance) -> bool {
    utterance.duration() < MIN_DICTATION_DURATION || rms(utterance.samples()) < MIN_DICTATION_RMS
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

pub(crate) fn transcribe(
    recognizer: &OfflineRecognizer,
    utterance: &CapturedUtterance,
) -> Option<RawTranscript> {
    let stream = recognizer.create_stream();
    stream.accept_waveform(utterance.sample_rate().as_hz() as i32, utterance.samples());
    recognizer.decode(&stream);

    let result = stream.get_result()?;
    let text = result.text.trim();
    if text.is_empty() {
        None
    } else {
        Some(RawTranscript::new(text))
    }
}

pub(crate) fn transcript_is_noise(text: &str) -> bool {
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
    use crate::dictation::AudioSampleRate;

    #[test]
    fn rms_is_zero_for_empty_samples() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_measures_sample_energy() {
        assert_eq!(rms(&[3.0, 4.0]), 3.535534);
    }

    #[test]
    fn short_or_quiet_utterance_is_not_worth_transcribing() {
        let short = CapturedUtterance::new(AudioSampleRate::new(16_000).unwrap(), vec![1.0; 100])
            .expect("samples");
        let quiet = CapturedUtterance::new(
            AudioSampleRate::new(16_000).unwrap(),
            vec![MIN_DICTATION_RMS / 2.0; 16_000],
        )
        .expect("samples");

        assert!(too_short_or_quiet(&short));
        assert!(too_short_or_quiet(&quiet));
    }

    #[test]
    fn loud_enough_utterance_is_worth_transcribing() {
        let utterance = CapturedUtterance::new(
            AudioSampleRate::new(16_000).unwrap(),
            vec![MIN_DICTATION_RMS * 2.0; 16_000],
        )
        .expect("samples");

        assert!(!too_short_or_quiet(&utterance));
    }

    #[test]
    fn transcript_noise_filters_asr_junk() {
        assert!(transcript_is_noise("..."));
        assert!(transcript_is_noise("(cough)"));
        assert!(transcript_is_noise("music"));
        assert!(!transcript_is_noise("ship this please"));
    }
}
