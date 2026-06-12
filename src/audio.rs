use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;

use crate::dictation::CapturedUtterance;
use crate::dictation::DICTATION_SAMPLE_RATE;

pub fn load_wav_utterance(path: &Path) -> Result<CapturedUtterance> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open WAV audio {}", path.display()))?;
    let spec = reader.spec();

    if spec.channels != 1 {
        bail!(
            "audio file {} has {} channels; expected mono",
            path.display(),
            spec.channels
        );
    }

    if spec.sample_rate != DICTATION_SAMPLE_RATE.as_hz() {
        bail!(
            "audio file {} has {} Hz sample rate; expected {} Hz",
            path.display(),
            spec.sample_rate,
            DICTATION_SAMPLE_RATE.as_hz()
        );
    }

    let samples = match spec.sample_format {
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                bail!(
                    "audio file {} has unsupported {}-bit integer samples",
                    path.display(),
                    spec.bits_per_sample
                );
            }

            let max_amplitude = 2_f32.powi(i32::from(spec.bits_per_sample) - 1);
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|sample| sample as f32 / max_amplitude))
                .collect::<std::result::Result<Vec<_>, _>>()
        }
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                bail!(
                    "audio file {} has unsupported {}-bit float samples",
                    path.display(),
                    spec.bits_per_sample
                );
            }

            reader
                .samples::<f32>()
                .collect::<std::result::Result<Vec<_>, _>>()
        }
    }
    .with_context(|| format!("failed to read samples from {}", path.display()))?;

    CapturedUtterance::new(DICTATION_SAMPLE_RATE, samples)
        .ok_or_else(|| anyhow!("audio file {} had no samples", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    static NEXT_TEMP_FILE: AtomicUsize = AtomicUsize::new(0);

    fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transcription/cmu-arctic")
            .join(name)
    }

    fn temp_wav_path(name: &str) -> PathBuf {
        let id = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "dictate-audio-test-{}-{id}-{name}.wav",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    fn write_i16_wav(path: &Path, channels: u16, sample_rate: u32, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create test wav");
        for sample in samples {
            writer.write_sample(*sample).expect("write sample");
        }
        writer.finalize().expect("finalize test wav");
    }

    #[test]
    fn loads_committed_fixture_as_dictation_utterance() {
        let path = fixture_path("arctic_a0001.wav");

        let utterance = load_wav_utterance(&path).expect("load fixture");

        assert_eq!(utterance.sample_rate(), DICTATION_SAMPLE_RATE);
        assert_eq!(utterance.samples().len(), 51_761);
        assert!(utterance.samples().iter().any(|sample| *sample != 0.0));
        assert!(
            utterance
                .samples()
                .iter()
                .all(|sample| (-1.0..=1.0).contains(sample))
        );
    }

    #[test]
    fn rejects_empty_audio() {
        let path = temp_wav_path("empty");
        write_i16_wav(&path, 1, DICTATION_SAMPLE_RATE.as_hz(), &[]);

        let error = load_wav_utterance(&path).expect_err("empty audio is rejected");

        assert!(error.to_string().contains("had no samples"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_non_16khz_audio() {
        let path = temp_wav_path("wrong-rate");
        write_i16_wav(&path, 1, 8_000, &[0, 1]);

        let error = load_wav_utterance(&path).expect_err("wrong sample rate is rejected");

        assert!(error.to_string().contains("8000 Hz sample rate"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_non_mono_audio() {
        let path = temp_wav_path("stereo");
        write_i16_wav(&path, 2, DICTATION_SAMPLE_RATE.as_hz(), &[0, 0]);

        let error = load_wav_utterance(&path).expect_err("stereo audio is rejected");

        assert!(error.to_string().contains("2 channels"));
        let _ = fs::remove_file(path);
    }
}
