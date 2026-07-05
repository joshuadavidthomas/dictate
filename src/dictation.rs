use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

pub const DICTATION_SAMPLE_RATE: SampleRate = SampleRate(16_000);
pub const MAX_DICTATION_DURATION: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SampleRate(u32);

impl SampleRate {
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 { None } else { Some(Self(hz)) }
    }

    pub const fn as_hz(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictationPhase {
    Initializing,
    Idle,
    Recording,
    Transcribing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DictationCommand {
    Start,
    Stop,
    Toggle,
    Cancel,
}

impl FromStr for DictationCommand {
    type Err = ParseDictationCommandError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "toggle" => Ok(Self::Toggle),
            "cancel" => Ok(Self::Cancel),
            _ => Err(ParseDictationCommandError),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("expected start, stop, toggle, or cancel")]
pub struct ParseDictationCommandError;

impl DictationPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Initializing => "Transcription starting…",
            Self::Idle => "Ready",
            Self::Recording => "Recording…",
            Self::Transcribing => "Transcribing…",
            Self::Unavailable => "Transcription unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CapturedUtterance {
    sample_rate: SampleRate,
    samples: Vec<f32>,
}

impl CapturedUtterance {
    pub fn new(sample_rate: SampleRate, samples: Vec<f32>) -> Option<Self> {
        if samples.is_empty() {
            None
        } else {
            Some(Self {
                sample_rate,
                samples,
            })
        }
    }

    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f32(self.samples.len() as f32 / self.sample_rate.as_hz() as f32)
    }
}

#[derive(Clone)]
pub(crate) struct DictationControl {
    state: Arc<Mutex<DictationControlState>>,
}

impl DictationControl {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DictationControlState::Initializing)),
        }
    }

    pub(crate) fn apply(&self, command: DictationCommand) -> DictationUpdate {
        match command {
            DictationCommand::Start => self.start_recording(),
            DictationCommand::Stop => self.stop_recording(),
            DictationCommand::Toggle => match self.phase() {
                DictationPhase::Idle => self.start_recording(),
                DictationPhase::Recording => self.stop_recording(),
                DictationPhase::Initializing
                | DictationPhase::Transcribing
                | DictationPhase::Unavailable => DictationUpdate::Busy(self.phase()),
            },
            DictationCommand::Cancel => self.cancel_recording(),
        }
    }

    fn start_recording(&self) -> DictationUpdate {
        let mut state = self.state.lock().unwrap();

        match state.phase() {
            DictationPhase::Initializing => DictationUpdate::Busy(DictationPhase::Initializing),
            DictationPhase::Idle => {
                *state = DictationControlState::Recording {
                    sample_rate: DICTATION_SAMPLE_RATE,
                    samples: Vec::new(),
                };
                DictationUpdate::Started
            }
            DictationPhase::Recording => DictationUpdate::Ignored("already recording"),
            DictationPhase::Transcribing => DictationUpdate::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => DictationUpdate::Busy(DictationPhase::Unavailable),
        }
    }

    fn stop_recording(&self) -> DictationUpdate {
        let mut state = self.state.lock().unwrap();

        match std::mem::replace(&mut *state, DictationControlState::Idle) {
            DictationControlState::Initializing => {
                *state = DictationControlState::Initializing;
                DictationUpdate::Busy(DictationPhase::Initializing)
            }
            DictationControlState::Idle => DictationUpdate::Ignored("not recording"),
            DictationControlState::Recording {
                sample_rate,
                samples,
            } => {
                *state = stopped_recording_state(sample_rate, samples);
                DictationUpdate::Stopped
            }
            DictationControlState::Transcribing { ready_utterances } => {
                *state = DictationControlState::Transcribing { ready_utterances };
                DictationUpdate::Busy(DictationPhase::Transcribing)
            }
            DictationControlState::Unavailable => {
                *state = DictationControlState::Unavailable;
                DictationUpdate::Busy(DictationPhase::Unavailable)
            }
        }
    }

    fn cancel_recording(&self) -> DictationUpdate {
        let mut state = self.state.lock().unwrap();

        match state.phase() {
            DictationPhase::Initializing => DictationUpdate::Busy(DictationPhase::Initializing),
            DictationPhase::Idle => DictationUpdate::Ignored("not recording"),
            DictationPhase::Recording => {
                *state = DictationControlState::Idle;
                DictationUpdate::Cancelled
            }
            DictationPhase::Transcribing => DictationUpdate::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => DictationUpdate::Busy(DictationPhase::Unavailable),
        }
    }

    pub(crate) fn phase(&self) -> DictationPhase {
        self.state.lock().unwrap().phase()
    }

    pub(crate) fn mark_ready(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(&*state, DictationControlState::Initializing) {
            *state = DictationControlState::Idle;
        }
    }

    pub(crate) fn record_samples(&self, new_samples: &[f32]) -> RecordSamplesUpdate {
        let mut state = self.state.lock().unwrap();
        let reached_limit = match &mut *state {
            DictationControlState::Recording {
                sample_rate,
                samples,
            } => {
                let max_samples = max_dictation_samples(*sample_rate);
                let remaining = max_samples.saturating_sub(samples.len());
                let accepted = remaining.min(new_samples.len());
                samples.extend_from_slice(&new_samples[..accepted]);
                samples.len() >= max_samples
            }
            _ => return RecordSamplesUpdate::Ignored,
        };

        if reached_limit {
            let DictationControlState::Recording {
                sample_rate,
                samples,
            } = std::mem::replace(&mut *state, DictationControlState::Idle)
            else {
                unreachable!("recording state was checked before auto-stop")
            };
            *state = stopped_recording_state(sample_rate, samples);
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION,
            }
        } else {
            RecordSamplesUpdate::Recording
        }
    }

    pub(crate) fn take_utterance(&self) -> Option<CapturedUtterance> {
        let mut state = self.state.lock().unwrap();
        if let DictationControlState::Transcribing { ready_utterances } = &mut *state {
            ready_utterances.pop_front()
        } else {
            None
        }
    }

    pub(crate) fn finish_transcription(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            &*state,
            DictationControlState::Transcribing { ready_utterances } if ready_utterances.is_empty()
        ) {
            *state = DictationControlState::Idle;
        }
    }

    pub(crate) fn mark_unavailable(&self) {
        *self.state.lock().unwrap() = DictationControlState::Unavailable;
    }
}

fn max_dictation_samples(sample_rate: SampleRate) -> usize {
    sample_rate.as_hz() as usize * MAX_DICTATION_DURATION.as_secs() as usize
}

fn stopped_recording_state(sample_rate: SampleRate, samples: Vec<f32>) -> DictationControlState {
    if let Some(utterance) = CapturedUtterance::new(sample_rate, samples) {
        let mut ready_utterances = VecDeque::new();
        ready_utterances.push_back(utterance);
        DictationControlState::Transcribing { ready_utterances }
    } else {
        DictationControlState::Idle
    }
}

#[derive(Debug)]
enum DictationControlState {
    Initializing,
    Idle,
    Recording {
        sample_rate: SampleRate,
        samples: Vec<f32>,
    },
    Transcribing {
        ready_utterances: VecDeque<CapturedUtterance>,
    },
    Unavailable,
}

impl DictationControlState {
    const fn phase(&self) -> DictationPhase {
        match self {
            Self::Initializing => DictationPhase::Initializing,
            Self::Idle => DictationPhase::Idle,
            Self::Recording { .. } => DictationPhase::Recording,
            Self::Transcribing { .. } => DictationPhase::Transcribing,
            Self::Unavailable => DictationPhase::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DictationUpdate {
    Started,
    Stopped,
    Cancelled,
    Ignored(&'static str),
    Busy(DictationPhase),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordSamplesUpdate {
    Recording,
    AutoStopped { duration: Duration },
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_test_recording(dictation: &DictationControl, sample_rate: SampleRate) {
        *dictation.state.lock().unwrap() = DictationControlState::Recording {
            sample_rate,
            samples: Vec::new(),
        };
    }

    fn test_sample_rate() -> SampleRate {
        SampleRate::new(4).expect("non-zero sample rate")
    }

    #[test]
    fn zero_sample_rate_is_rejected() {
        assert!(SampleRate::new(0).is_none());
    }

    #[test]
    fn phase_labels_describe_user_action_or_state() {
        assert_eq!(
            DictationPhase::Initializing.label(),
            "Transcription starting…"
        );
        assert_eq!(DictationPhase::Idle.label(), "Ready");
        assert_eq!(DictationPhase::Recording.label(), "Recording…");
        assert_eq!(DictationPhase::Transcribing.label(), "Transcribing…");
        assert_eq!(
            DictationPhase::Unavailable.label(),
            "Transcription unavailable"
        );
    }

    #[test]
    fn recording_stops_to_captured_utterance() {
        let dictation = DictationControl::new();
        dictation.mark_ready();

        assert_eq!(
            dictation.apply(DictationCommand::Start),
            DictationUpdate::Started
        );
        dictation.record_samples(&[0.1, 0.2]);
        dictation.record_samples(&[0.3]);
        assert_eq!(
            dictation.apply(DictationCommand::Stop),
            DictationUpdate::Stopped
        );

        let utterance = dictation.take_utterance().expect("recording has samples");
        assert_eq!(utterance.sample_rate(), DICTATION_SAMPLE_RATE);
        assert_eq!(utterance.samples(), &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn empty_recording_returns_to_idle() {
        let dictation = DictationControl::new();
        dictation.mark_ready();

        assert_eq!(
            dictation.apply(DictationCommand::Start),
            DictationUpdate::Started
        );
        assert_eq!(
            dictation.apply(DictationCommand::Stop),
            DictationUpdate::Stopped
        );

        assert_eq!(dictation.phase(), DictationPhase::Idle);
        assert!(dictation.take_utterance().is_none());
    }

    #[test]
    fn initializing_blocks_recording_until_microphone_is_ready() {
        let dictation = DictationControl::new();

        assert_eq!(dictation.phase(), DictationPhase::Initializing);
        assert_eq!(
            dictation.apply(DictationCommand::Start),
            DictationUpdate::Busy(DictationPhase::Initializing)
        );
        assert_eq!(dictation.phase(), DictationPhase::Initializing);

        dictation.mark_ready();
        assert_eq!(
            dictation.apply(DictationCommand::Start),
            DictationUpdate::Started
        );
    }

    #[test]
    fn cap_samples_auto_stop_to_transcribing() {
        let sample_rate = test_sample_rate();
        let cap_samples = max_dictation_samples(sample_rate);
        let dictation = DictationControl::new();
        start_test_recording(&dictation, sample_rate);

        assert_eq!(
            dictation.record_samples(&vec![0.1; cap_samples]),
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION
            }
        );

        let utterance = dictation
            .take_utterance()
            .expect("auto-stop queues utterance");
        assert_eq!(utterance.sample_rate(), sample_rate);
        assert_eq!(utterance.samples().len(), cap_samples);
    }

    #[test]
    fn cap_samples_truncate_final_batch_at_limit() {
        let sample_rate = test_sample_rate();
        let cap_samples = max_dictation_samples(sample_rate);
        let dictation = DictationControl::new();
        start_test_recording(&dictation, sample_rate);

        assert_eq!(
            dictation.record_samples(&vec![1.0; cap_samples - 2]),
            RecordSamplesUpdate::Recording
        );
        assert_eq!(
            dictation.record_samples(&[2.0, 3.0, 4.0, 5.0]),
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION
            }
        );

        let utterance = dictation
            .take_utterance()
            .expect("auto-stop queues utterance");
        assert_eq!(utterance.samples().len(), cap_samples);
        assert_eq!(&utterance.samples()[cap_samples - 2..], &[2.0, 3.0]);
    }

    #[test]
    fn record_samples_ignored_after_auto_stop_until_transcription_finishes() {
        let sample_rate = test_sample_rate();
        let cap_samples = max_dictation_samples(sample_rate);
        let dictation = DictationControl::new();
        start_test_recording(&dictation, sample_rate);

        assert_eq!(
            dictation.record_samples(&vec![0.1; cap_samples]),
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION
            }
        );
        assert_eq!(
            dictation.record_samples(&[0.2, 0.3]),
            RecordSamplesUpdate::Ignored
        );

        let utterance = dictation
            .take_utterance()
            .expect("auto-stop queues utterance");
        assert_eq!(utterance.samples().len(), cap_samples);
        dictation.finish_transcription();
        assert_eq!(dictation.phase(), DictationPhase::Idle);
    }

    #[test]
    fn utterance_duration_uses_sample_rate() {
        let sample_rate = SampleRate::new(4).expect("non-zero sample rate");
        let utterance = CapturedUtterance::new(sample_rate, vec![0.0; 6])
            .expect("samples produce an utterance");

        assert_eq!(utterance.duration(), Duration::from_secs_f32(1.5));
    }
}
