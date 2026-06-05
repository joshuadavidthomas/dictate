use std::time::Duration;

pub const DICTATION_SAMPLE_RATE: AudioSampleRate = AudioSampleRate { hz: 16_000 };

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioSampleRate {
    hz: u32,
}

impl AudioSampleRate {
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 { None } else { Some(Self { hz }) }
    }

    pub const fn as_hz(self) -> u32 {
        self.hz
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictationPhase {
    Idle,
    Recording,
    Transcribing,
    Unavailable,
}

impl DictationPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Recording => "Recording…",
            Self::Transcribing => "Transcribing…",
            Self::Unavailable => "Transcription unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CapturedUtterance {
    sample_rate: AudioSampleRate,
    samples: Vec<f32>,
}

impl CapturedUtterance {
    pub fn new(sample_rate: AudioSampleRate, samples: Vec<f32>) -> Option<Self> {
        if samples.is_empty() {
            None
        } else {
            Some(Self {
                sample_rate,
                samples,
            })
        }
    }

    pub fn sample_rate(&self) -> AudioSampleRate {
        self.sample_rate
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f32(self.samples.len() as f32 / self.sample_rate.as_hz() as f32)
    }
}

#[derive(Clone, Debug)]
pub struct DictationSession {
    sample_rate: AudioSampleRate,
    samples: Vec<f32>,
}

impl DictationSession {
    pub fn new(sample_rate: AudioSampleRate) -> Self {
        Self {
            sample_rate,
            samples: Vec::new(),
        }
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
    }

    pub fn finish(self) -> Option<CapturedUtterance> {
        CapturedUtterance::new(self.sample_rate, self.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_sample_rate_is_rejected() {
        assert!(AudioSampleRate::new(0).is_none());
    }

    #[test]
    fn phase_labels_describe_user_action_or_state() {
        assert_eq!(DictationPhase::Idle.label(), "Ready");
        assert_eq!(DictationPhase::Recording.label(), "Recording…");
        assert_eq!(DictationPhase::Transcribing.label(), "Transcribing…");
        assert_eq!(
            DictationPhase::Unavailable.label(),
            "Transcription unavailable"
        );
    }

    #[test]
    fn session_finishes_to_captured_utterance() {
        let mut session = DictationSession::new(DICTATION_SAMPLE_RATE);

        session.push_samples(&[0.1, 0.2]);
        session.push_samples(&[0.3]);

        let utterance = session.finish().expect("session has samples");
        assert_eq!(utterance.sample_rate(), DICTATION_SAMPLE_RATE);
        assert_eq!(utterance.samples(), &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn empty_session_has_no_utterance() {
        let session = DictationSession::new(DICTATION_SAMPLE_RATE);

        assert!(session.finish().is_none());
    }

    #[test]
    fn utterance_duration_uses_sample_rate() {
        let sample_rate = AudioSampleRate::new(4).expect("non-zero sample rate");
        let utterance = CapturedUtterance::new(sample_rate, vec![0.0; 6])
            .expect("samples produce an utterance");

        assert_eq!(utterance.duration(), Duration::from_secs_f32(1.5));
    }
}
