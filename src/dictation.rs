use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseDictationCommandError;

impl fmt::Display for ParseDictationCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected start, stop, toggle, or cancel")
    }
}

impl Error for ParseDictationCommandError {}

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

#[derive(Clone)]
pub(crate) struct DictationControl {
    state: Arc<Mutex<DictationControlState>>,
}

impl DictationControl {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DictationControlState::new())),
        }
    }

    pub(crate) fn start_recording(&self) -> ControlOutcome {
        let mut state = self.state.lock().unwrap();

        match state.phase {
            DictationPhase::Idle => {
                state.active_session = Some(DictationSession::new(DICTATION_SAMPLE_RATE));
                state.phase = DictationPhase::Recording;
                ControlOutcome::Started
            }
            DictationPhase::Recording => ControlOutcome::Ignored("already recording"),
            DictationPhase::Transcribing => ControlOutcome::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => ControlOutcome::Busy(DictationPhase::Unavailable),
        }
    }

    pub(crate) fn stop_recording(&self) -> ControlOutcome {
        let mut state = self.state.lock().unwrap();

        match state.phase {
            DictationPhase::Idle => ControlOutcome::Ignored("not recording"),
            DictationPhase::Recording => {
                let utterance = state
                    .active_session
                    .take()
                    .and_then(DictationSession::finish);
                if let Some(utterance) = utterance {
                    state.ready_utterances.push_back(utterance);
                    state.phase = DictationPhase::Transcribing;
                } else {
                    state.phase = DictationPhase::Idle;
                }
                ControlOutcome::Stopped
            }
            DictationPhase::Transcribing => ControlOutcome::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => ControlOutcome::Busy(DictationPhase::Unavailable),
        }
    }

    pub(crate) fn cancel_recording(&self) -> ControlOutcome {
        let mut state = self.state.lock().unwrap();

        match state.phase {
            DictationPhase::Idle => ControlOutcome::Ignored("not recording"),
            DictationPhase::Recording => {
                state.active_session = None;
                state.ready_utterances.clear();
                state.phase = DictationPhase::Idle;
                ControlOutcome::Cancelled
            }
            DictationPhase::Transcribing => ControlOutcome::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => ControlOutcome::Busy(DictationPhase::Unavailable),
        }
    }

    pub(crate) fn phase(&self) -> DictationPhase {
        self.state.lock().unwrap().phase
    }

    pub(crate) fn record_samples(&self, samples: &[f32]) {
        let mut state = self.state.lock().unwrap();
        if let Some(session) = state.active_session.as_mut() {
            session.push_samples(samples);
        }
    }

    pub(crate) fn take_utterance(&self) -> Option<CapturedUtterance> {
        self.state.lock().unwrap().ready_utterances.pop_front()
    }

    pub(crate) fn finish_transcription(&self) {
        let mut state = self.state.lock().unwrap();
        if state.ready_utterances.is_empty() && state.active_session.is_none() {
            state.phase = DictationPhase::Idle;
        }
    }

    pub(crate) fn mark_unavailable(&self) {
        let mut state = self.state.lock().unwrap();
        state.phase = DictationPhase::Unavailable;
        state.active_session = None;
        state.ready_utterances.clear();
    }
}

#[derive(Debug)]
struct DictationControlState {
    phase: DictationPhase,
    active_session: Option<DictationSession>,
    ready_utterances: VecDeque<CapturedUtterance>,
}

impl DictationControlState {
    fn new() -> Self {
        Self {
            phase: DictationPhase::Idle,
            active_session: None,
            ready_utterances: VecDeque::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControlOutcome {
    Started,
    Stopped,
    Cancelled,
    Ignored(&'static str),
    Busy(DictationPhase),
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
