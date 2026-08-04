use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;

use crate::dictation::CapturedUtterance;
use crate::dictation::DICTATION_SAMPLE_RATE;

#[allow(clippy::cast_possible_truncation)]
pub fn save_wav_utterance(path: &Path, utterance: &CapturedUtterance) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: utterance.sample_rate().as_hz(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("failed to create WAV audio {}", path.display()))?;
    for &sample in utterance.samples() {
        let scaled = (sample.clamp(-1.0, 1.0) * 32768.0).round();
        let sample = scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        writer
            .write_sample(sample)
            .with_context(|| format!("failed to write samples to {}", path.display()))?;
    }
    writer
        .finalize()
        .with_context(|| format!("failed to finalize WAV audio {}", path.display()))?;
    Ok(())
}

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

            if spec.bits_per_sample <= 16 {
                let max_amplitude = 2_f32.powi(i32::from(spec.bits_per_sample) - 1);
                reader
                    .samples::<i16>()
                    .map(|sample| sample.map(|sample| f32::from(sample) / max_amplitude))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(anyhow::Error::from)
            } else {
                let shift = u32::from(spec.bits_per_sample - 16);
                let mut samples = Vec::new();
                for sample in reader.samples::<i32>() {
                    let sample = sample.map_err(anyhow::Error::from)?;
                    let shifted = sample >> shift;
                    let sample = i16::try_from(shifted).with_context(|| {
                        format!("sample {shifted} does not fit after 16-bit downshift")
                    })?;
                    samples.push(f32::from(sample) / 32768.0);
                }
                Ok(samples)
            }
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
                .map_err(anyhow::Error::from)
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
            .join("tests/fixtures/cmu-arctic")
            .join(name)
    }

    fn temp_wav_path(name: &str) -> PathBuf {
        let id = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "dictate-audio-test-{}-{id}-{name}.wav",
            std::process::id()
        ));
        drop(fs::remove_file(&path));
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
    fn saves_and_loads_utterance_with_i16_precision() {
        let path = temp_wav_path("round-trip");
        let samples = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        let utterance = CapturedUtterance::new(DICTATION_SAMPLE_RATE, samples.clone())
            .expect("non-empty utterance");

        save_wav_utterance(&path, &utterance).expect("save utterance");
        let loaded = load_wav_utterance(&path).expect("load saved utterance");

        assert_eq!(loaded.sample_rate(), utterance.sample_rate());
        assert_eq!(loaded.samples().len(), samples.len());
        for (&actual, expected) in loaded.samples().iter().zip(samples) {
            assert!((actual - expected).abs() <= 1.0 / 32768.0);
        }
        drop(fs::remove_file(path));
    }

    #[test]
    fn clamps_samples_when_saving_utterance() {
        let path = temp_wav_path("clamped");
        let utterance = CapturedUtterance::new(DICTATION_SAMPLE_RATE, vec![-2.0, 2.0])
            .expect("non-empty utterance");

        save_wav_utterance(&path, &utterance).expect("save utterance");
        let loaded = load_wav_utterance(&path).expect("load saved utterance");

        assert_eq!(loaded.samples(), &[-1.0, f32::from(i16::MAX) / 32768.0]);
        drop(fs::remove_file(path));
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
        drop(fs::remove_file(path));
    }

    #[test]
    fn rejects_non_16khz_audio() {
        let path = temp_wav_path("wrong-rate");
        write_i16_wav(&path, 1, 8_000, &[0, 1]);

        let error = load_wav_utterance(&path).expect_err("wrong sample rate is rejected");

        assert!(error.to_string().contains("8000 Hz sample rate"));
        drop(fs::remove_file(path));
    }

    #[test]
    fn rejects_non_mono_audio() {
        let path = temp_wav_path("stereo");
        write_i16_wav(&path, 2, DICTATION_SAMPLE_RATE.as_hz(), &[0, 0]);

        let error = load_wav_utterance(&path).expect_err("stereo audio is rejected");

        assert!(error.to_string().contains("2 channels"));
        drop(fs::remove_file(path));
    }
}
