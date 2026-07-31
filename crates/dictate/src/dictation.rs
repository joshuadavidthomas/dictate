use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

pub const DICTATION_SAMPLE_RATE: SampleRate = SampleRate(16_000);
pub const MAX_DICTATION_DURATION: Duration = Duration::from_mins(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SampleRate(u32);

impl SampleRate {
    #[must_use]
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 { None } else { Some(Self(hz)) }
    }

    #[must_use]
    pub const fn as_hz(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecordingId(u64);

impl RecordingId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictationPhase {
    Initializing,
    Idle,
    Recording,
    Transcribing,
    /// Fatal transcription initialization failure; daemon restart required.
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
    #[must_use]
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
    #[must_use]
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

    #[must_use]
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[must_use]
    pub fn duration(&self) -> Duration {
        let sample_count = u32::try_from(self.samples.len()).map_or(u32::MAX, |count| count);
        Duration::from_secs_f64(f64::from(sample_count) / f64::from(self.sample_rate.as_hz()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReadyDictation<StopMetadata> {
    utterance: CapturedUtterance,
    stop_metadata: StopMetadata,
}

impl<StopMetadata> ReadyDictation<StopMetadata> {
    pub(crate) fn utterance(&self) -> &CapturedUtterance {
        &self.utterance
    }

    pub(crate) fn stop_metadata(&self) -> &StopMetadata {
        &self.stop_metadata
    }
}

pub(crate) struct DictationControl<StopMetadata> {
    state: Arc<Mutex<DictationControlState<StopMetadata>>>,
    next_recording_id: Arc<AtomicU64>,
}

impl<StopMetadata> Clone for DictationControl<StopMetadata> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            next_recording_id: Arc::clone(&self.next_recording_id),
        }
    }
}

impl<StopMetadata> DictationControl<StopMetadata> {
    fn state(&self) -> MutexGuard<'_, DictationControlState<StopMetadata>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DictationControlState::Initializing)),
            next_recording_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn apply(
        &self,
        command: DictationCommand,
        stop_metadata: StopMetadata,
    ) -> DictationUpdate {
        match command {
            DictationCommand::Start => self.start_recording(),
            DictationCommand::Stop => self.stop_recording(stop_metadata),
            DictationCommand::Toggle => match self.phase() {
                DictationPhase::Idle => self.start_recording(),
                DictationPhase::Recording => self.stop_recording(stop_metadata),
                DictationPhase::Initializing
                | DictationPhase::Transcribing
                | DictationPhase::Unavailable => DictationUpdate::Busy(self.phase()),
            },
            DictationCommand::Cancel => self.cancel_recording(),
        }
    }

    fn start_recording(&self) -> DictationUpdate {
        let mut state = self.state();

        match state.phase() {
            DictationPhase::Initializing => DictationUpdate::Busy(DictationPhase::Initializing),
            DictationPhase::Idle => {
                let recording_id =
                    RecordingId::new(self.next_recording_id.fetch_add(1, Ordering::Relaxed) + 1);
                *state = DictationControlState::PendingRecording {
                    recording_id,
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

    fn stop_recording(&self, stop_metadata: StopMetadata) -> DictationUpdate {
        let mut state = self.state();

        match std::mem::replace(&mut *state, DictationControlState::Idle) {
            DictationControlState::Initializing => {
                *state = DictationControlState::Initializing;
                DictationUpdate::Busy(DictationPhase::Initializing)
            }
            DictationControlState::Idle => DictationUpdate::Ignored("not recording"),
            DictationControlState::PendingRecording {
                recording_id,
                sample_rate,
                samples,
            }
            | DictationControlState::Recording {
                recording_id,
                sample_rate,
                samples,
            } => {
                *state = DictationControlState::PendingStop {
                    recording_id,
                    sample_rate,
                    samples,
                    stop_metadata,
                };
                DictationUpdate::Stopped
            }
            DictationControlState::PendingStop {
                recording_id,
                sample_rate,
                samples,
                stop_metadata,
            } => {
                *state = DictationControlState::PendingStop {
                    recording_id,
                    sample_rate,
                    samples,
                    stop_metadata,
                };
                DictationUpdate::Busy(DictationPhase::Transcribing)
            }
            DictationControlState::Stopping {
                recording_id,
                sample_rate,
                samples,
                stop_metadata,
            } => {
                *state = DictationControlState::Stopping {
                    recording_id,
                    sample_rate,
                    samples,
                    stop_metadata,
                };
                DictationUpdate::Busy(DictationPhase::Transcribing)
            }
            DictationControlState::AwaitingStopMetadata {
                recording_id,
                utterance,
            } => {
                *state = DictationControlState::AwaitingStopMetadata {
                    recording_id,
                    utterance,
                };
                DictationUpdate::Busy(DictationPhase::Transcribing)
            }
            DictationControlState::PendingTranscription { ready_dictations } => {
                *state = DictationControlState::PendingTranscription { ready_dictations };
                DictationUpdate::Busy(DictationPhase::Transcribing)
            }
            DictationControlState::Transcribing { ready_dictations } => {
                *state = DictationControlState::Transcribing { ready_dictations };
                DictationUpdate::Busy(DictationPhase::Transcribing)
            }
            DictationControlState::Unavailable => {
                *state = DictationControlState::Unavailable;
                DictationUpdate::Busy(DictationPhase::Unavailable)
            }
        }
    }

    fn cancel_recording(&self) -> DictationUpdate {
        let mut state = self.state();

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
        self.state().phase()
    }

    pub(crate) fn mark_ready(&self) {
        let mut state = self.state();
        if matches!(&*state, DictationControlState::Initializing) {
            *state = DictationControlState::Idle;
        }
    }

    pub(crate) fn begin_recording(&self) -> bool {
        let mut state = self.state();
        let previous = std::mem::replace(&mut *state, DictationControlState::Idle);
        let DictationControlState::PendingRecording {
            recording_id,
            sample_rate,
            samples,
        } = previous
        else {
            *state = previous;
            return false;
        };

        *state = DictationControlState::Recording {
            recording_id,
            sample_rate,
            samples,
        };
        true
    }

    pub(crate) fn begin_stopping(&self) -> bool {
        let mut state = self.state();
        let previous = std::mem::replace(&mut *state, DictationControlState::Idle);
        let DictationControlState::PendingStop {
            recording_id,
            sample_rate,
            samples,
            stop_metadata,
        } = previous
        else {
            *state = previous;
            return false;
        };

        *state = DictationControlState::Stopping {
            recording_id,
            sample_rate,
            samples,
            stop_metadata,
        };
        true
    }

    pub(crate) fn recording_id(&self) -> Option<RecordingId> {
        match &*self.state() {
            DictationControlState::Recording { recording_id, .. }
            | DictationControlState::PendingStop { recording_id, .. } => Some(*recording_id),
            DictationControlState::Initializing
            | DictationControlState::Idle
            | DictationControlState::PendingRecording { .. }
            | DictationControlState::Stopping { .. }
            | DictationControlState::AwaitingStopMetadata { .. }
            | DictationControlState::PendingTranscription { .. }
            | DictationControlState::Transcribing { .. }
            | DictationControlState::Unavailable => None,
        }
    }

    pub(crate) fn record_samples(
        &self,
        recording_id: RecordingId,
        new_samples: &[f32],
    ) -> RecordSamplesUpdate {
        let mut state = self.state();
        let reached_limit = match &mut *state {
            DictationControlState::Recording {
                recording_id: current_id,
                sample_rate,
                samples,
            } if *current_id == recording_id => {
                append_bounded_samples(*sample_rate, samples, new_samples)
            }
            DictationControlState::PendingStop {
                recording_id: current_id,
                sample_rate,
                samples,
                ..
            }
            | DictationControlState::Stopping {
                recording_id: current_id,
                sample_rate,
                samples,
                ..
            } if *current_id == recording_id => {
                append_bounded_samples(*sample_rate, samples, new_samples);
                return RecordSamplesUpdate::Recording;
            }
            DictationControlState::Initializing
            | DictationControlState::Idle
            | DictationControlState::PendingRecording { .. }
            | DictationControlState::Recording { .. }
            | DictationControlState::PendingStop { .. }
            | DictationControlState::Stopping { .. }
            | DictationControlState::AwaitingStopMetadata { .. }
            | DictationControlState::PendingTranscription { .. }
            | DictationControlState::Transcribing { .. }
            | DictationControlState::Unavailable => return RecordSamplesUpdate::Ignored,
        };

        if reached_limit {
            let previous_state = std::mem::replace(&mut *state, DictationControlState::Idle);
            let DictationControlState::Recording {
                recording_id,
                sample_rate,
                samples,
            } = previous_state
            else {
                return RecordSamplesUpdate::Ignored;
            };
            *state = stopped_recording_state(recording_id, sample_rate, samples, None);
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION,
            }
        } else {
            RecordSamplesUpdate::Recording
        }
    }

    pub(crate) fn finish_stopping(&self) -> FinishStopping {
        let mut state = self.state();
        let previous = std::mem::replace(&mut *state, DictationControlState::Idle);
        let DictationControlState::Stopping {
            recording_id,
            sample_rate,
            samples,
            stop_metadata,
        } = previous
        else {
            *state = previous;
            return FinishStopping::NotStopping;
        };

        *state = stopped_recording_state(recording_id, sample_rate, samples, Some(stop_metadata));
        if matches!(&*state, DictationControlState::PendingTranscription { .. }) {
            FinishStopping::Ready
        } else {
            FinishStopping::Empty
        }
    }

    pub(crate) fn begin_pending_transcription(&self) -> bool {
        let mut state = self.state();
        let previous = std::mem::replace(&mut *state, DictationControlState::Idle);
        let DictationControlState::PendingTranscription { ready_dictations } = previous else {
            *state = previous;
            return false;
        };

        *state = DictationControlState::Transcribing { ready_dictations };
        true
    }

    pub(crate) fn attach_pending_stop_metadata(
        &self,
        recording_id: RecordingId,
        stop_metadata: StopMetadata,
    ) -> bool {
        let mut state = self.state();
        let previous = std::mem::replace(&mut *state, DictationControlState::Idle);
        let DictationControlState::AwaitingStopMetadata {
            recording_id: pending_id,
            utterance,
        } = previous
        else {
            *state = previous;
            return false;
        };
        if pending_id != recording_id {
            *state = DictationControlState::AwaitingStopMetadata {
                recording_id: pending_id,
                utterance,
            };
            return false;
        }

        let mut ready_dictations = VecDeque::new();
        ready_dictations.push_back(ReadyDictation {
            utterance,
            stop_metadata,
        });
        *state = DictationControlState::PendingTranscription { ready_dictations };
        true
    }

    pub(crate) fn take_ready_dictation(&self) -> Option<ReadyDictation<StopMetadata>> {
        let mut state = self.state();
        if let DictationControlState::Transcribing { ready_dictations } = &mut *state {
            ready_dictations.pop_front()
        } else {
            None
        }
    }

    pub(crate) fn finish_transcription(&self) {
        let mut state = self.state();
        if matches!(
            &*state,
            DictationControlState::Transcribing { ready_dictations } if ready_dictations.is_empty()
        ) {
            *state = DictationControlState::Idle;
        }
    }

    /// Abort a retryable recording failure and return to idle when actively recording.
    pub(crate) fn abort_recording(&self, recording_id: RecordingId) -> bool {
        let mut state = self.state();
        if matches!(
            &*state,
            DictationControlState::Recording {
                recording_id: current_id,
                ..
            } if *current_id == recording_id
        ) {
            *state = DictationControlState::Idle;
            true
        } else {
            false
        }
    }

    /// Mark transcription unavailable after fatal worker initialization failure only; restart required.
    pub(crate) fn mark_unavailable(&self) {
        *self.state() = DictationControlState::Unavailable;
    }
}

fn append_bounded_samples(
    sample_rate: SampleRate,
    samples: &mut Vec<f32>,
    new_samples: &[f32],
) -> bool {
    let max_samples = max_dictation_samples(sample_rate);
    let remaining = max_samples.saturating_sub(samples.len());
    let accepted = remaining.min(new_samples.len());
    samples.extend_from_slice(&new_samples[..accepted]);
    samples.len() >= max_samples
}

fn max_dictation_samples(sample_rate: SampleRate) -> usize {
    let samples_per_second = usize::try_from(sample_rate.as_hz()).map_or(usize::MAX, |rate| rate);
    let max_seconds =
        usize::try_from(MAX_DICTATION_DURATION.as_secs()).map_or(usize::MAX, |seconds| seconds);
    samples_per_second.saturating_mul(max_seconds)
}

fn stopped_recording_state<StopMetadata>(
    recording_id: RecordingId,
    sample_rate: SampleRate,
    samples: Vec<f32>,
    stop_metadata: Option<StopMetadata>,
) -> DictationControlState<StopMetadata> {
    let Some(utterance) = CapturedUtterance::new(sample_rate, samples) else {
        return DictationControlState::Idle;
    };

    match stop_metadata {
        Some(stop_metadata) => {
            let mut ready_dictations = VecDeque::new();
            ready_dictations.push_back(ReadyDictation {
                utterance,
                stop_metadata,
            });
            DictationControlState::PendingTranscription { ready_dictations }
        }
        None => DictationControlState::AwaitingStopMetadata {
            recording_id,
            utterance,
        },
    }
}

#[derive(Debug)]
enum DictationControlState<StopMetadata> {
    Initializing,
    Idle,
    PendingRecording {
        recording_id: RecordingId,
        sample_rate: SampleRate,
        samples: Vec<f32>,
    },
    Recording {
        recording_id: RecordingId,
        sample_rate: SampleRate,
        samples: Vec<f32>,
    },
    PendingStop {
        recording_id: RecordingId,
        sample_rate: SampleRate,
        samples: Vec<f32>,
        stop_metadata: StopMetadata,
    },
    Stopping {
        recording_id: RecordingId,
        sample_rate: SampleRate,
        samples: Vec<f32>,
        stop_metadata: StopMetadata,
    },
    AwaitingStopMetadata {
        recording_id: RecordingId,
        utterance: CapturedUtterance,
    },
    PendingTranscription {
        ready_dictations: VecDeque<ReadyDictation<StopMetadata>>,
    },
    Transcribing {
        ready_dictations: VecDeque<ReadyDictation<StopMetadata>>,
    },
    Unavailable,
}

impl<StopMetadata> DictationControlState<StopMetadata> {
    const fn phase(&self) -> DictationPhase {
        match self {
            Self::Initializing => DictationPhase::Initializing,
            Self::Idle => DictationPhase::Idle,
            Self::PendingRecording { .. } | Self::Recording { .. } => DictationPhase::Recording,
            Self::PendingStop { .. }
            | Self::Stopping { .. }
            | Self::AwaitingStopMetadata { .. }
            | Self::PendingTranscription { .. }
            | Self::Transcribing { .. } => DictationPhase::Transcribing,
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
pub(crate) enum FinishStopping {
    NotStopping,
    Empty,
    Ready,
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

    const TEST_RECORDING_ID: RecordingId = RecordingId::new(1);

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum TestStopMetadata {
        Unavailable,
        Focused {
            window_id: u64,
            app_id: String,
            title: String,
        },
    }

    type TestDictation = DictationControl<TestStopMetadata>;

    fn focused_metadata(window_id: u64, app_id: &str, title: &str) -> TestStopMetadata {
        TestStopMetadata::Focused {
            window_id,
            app_id: app_id.to_owned(),
            title: title.to_owned(),
        }
    }

    fn test_dictation() -> TestDictation {
        TestDictation::new()
    }

    fn start_test_recording(dictation: &TestDictation, sample_rate: SampleRate) {
        *dictation.state() = DictationControlState::Recording {
            recording_id: TEST_RECORDING_ID,
            sample_rate,
            samples: Vec::new(),
        };
    }

    fn test_sample_rate() -> SampleRate {
        SampleRate::new(4).expect("non-zero sample rate")
    }

    fn apply_test(dictation: &TestDictation, command: DictationCommand) -> DictationUpdate {
        dictation.apply(command, TestStopMetadata::Unavailable)
    }

    fn activate_test_recording(dictation: &TestDictation) -> RecordingId {
        assert!(dictation.begin_recording());
        dictation
            .recording_id()
            .expect("recording should have an id")
    }

    fn record_test_samples(dictation: &TestDictation, samples: &[f32]) -> RecordSamplesUpdate {
        dictation.record_samples(TEST_RECORDING_ID, samples)
    }

    fn attach_test_stop_metadata(dictation: &TestDictation) {
        assert!(
            dictation
                .attach_pending_stop_metadata(TEST_RECORDING_ID, TestStopMetadata::Unavailable)
        );
        assert!(dictation.begin_pending_transcription());
    }

    fn take_test_utterance(dictation: &TestDictation) -> Option<CapturedUtterance> {
        dictation
            .take_ready_dictation()
            .map(|ready| ready.utterance)
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
            apply_test(&dictation, DictationCommand::Start),
            DictationUpdate::Started
        );
        let recording_id = activate_test_recording(&dictation);
        dictation.record_samples(recording_id, &[0.1, 0.2]);
        dictation.record_samples(recording_id, &[0.3]);
        let stop_metadata = focused_metadata(7, "dev.editor", "README.md");
        assert_eq!(
            dictation.apply(DictationCommand::Stop, stop_metadata.clone()),
            DictationUpdate::Stopped
        );
        assert_eq!(
            dictation.record_samples(recording_id, &[0.4, 0.5]),
            RecordSamplesUpdate::Recording
        );
        assert_eq!(dictation.recording_id(), Some(recording_id));
        assert!(dictation.begin_stopping());
        assert_eq!(dictation.recording_id(), None);
        assert!(dictation.take_ready_dictation().is_none());
        assert_eq!(dictation.finish_stopping(), FinishStopping::Ready);
        assert!(dictation.begin_pending_transcription());

        let ready = dictation
            .take_ready_dictation()
            .expect("recording has samples");
        assert_eq!(ready.stop_metadata(), &stop_metadata);
        assert_eq!(ready.utterance().sample_rate(), DICTATION_SAMPLE_RATE);
        assert_eq!(ready.utterance().samples(), &[0.1, 0.2, 0.3, 0.4, 0.5]);
    }

    #[test]
    fn empty_recording_returns_to_idle() {
        let dictation = DictationControl::new();
        dictation.mark_ready();

        assert_eq!(
            apply_test(&dictation, DictationCommand::Start),
            DictationUpdate::Started
        );
        activate_test_recording(&dictation);
        assert_eq!(
            apply_test(&dictation, DictationCommand::Stop),
            DictationUpdate::Stopped
        );

        assert_eq!(dictation.phase(), DictationPhase::Transcribing);
        assert!(dictation.begin_stopping());
        assert_eq!(dictation.finish_stopping(), FinishStopping::Empty);
        assert_eq!(dictation.phase(), DictationPhase::Idle);
        assert!(!dictation.begin_pending_transcription());
        assert!(take_test_utterance(&dictation).is_none());
    }

    #[test]
    fn initializing_blocks_recording_until_microphone_is_ready() {
        let dictation = DictationControl::new();

        assert_eq!(dictation.phase(), DictationPhase::Initializing);
        assert_eq!(
            apply_test(&dictation, DictationCommand::Start),
            DictationUpdate::Busy(DictationPhase::Initializing)
        );
        assert_eq!(dictation.phase(), DictationPhase::Initializing);

        dictation.mark_ready();
        assert_eq!(
            apply_test(&dictation, DictationCommand::Start),
            DictationUpdate::Started
        );
        assert!(dictation.recording_id().is_none());
        assert!(dictation.begin_recording());
        assert!(dictation.recording_id().is_some());
    }

    #[test]
    fn cap_samples_auto_stop_to_transcribing() {
        let sample_rate = test_sample_rate();
        let cap_samples = max_dictation_samples(sample_rate);
        let dictation = DictationControl::new();
        start_test_recording(&dictation, sample_rate);

        assert_eq!(
            record_test_samples(&dictation, &vec![0.1; cap_samples]),
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION
            }
        );

        assert!(take_test_utterance(&dictation).is_none());
        attach_test_stop_metadata(&dictation);
        let utterance = take_test_utterance(&dictation).expect("auto-stop queues utterance");
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
            record_test_samples(&dictation, &vec![1.0; cap_samples - 2]),
            RecordSamplesUpdate::Recording
        );
        assert_eq!(
            record_test_samples(&dictation, &[2.0, 3.0, 4.0, 5.0]),
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION
            }
        );

        attach_test_stop_metadata(&dictation);
        let utterance = take_test_utterance(&dictation).expect("auto-stop queues utterance");
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
            record_test_samples(&dictation, &vec![0.1; cap_samples]),
            RecordSamplesUpdate::AutoStopped {
                duration: MAX_DICTATION_DURATION
            }
        );
        assert_eq!(
            record_test_samples(&dictation, &[0.2, 0.3]),
            RecordSamplesUpdate::Ignored
        );

        attach_test_stop_metadata(&dictation);
        let utterance = take_test_utterance(&dictation).expect("auto-stop queues utterance");
        assert_eq!(utterance.samples().len(), cap_samples);
        dictation.finish_transcription();
        assert_eq!(dictation.phase(), DictationPhase::Idle);
    }

    #[test]
    fn stale_recording_work_cannot_mutate_a_new_recording() {
        let dictation = DictationControl::new();
        dictation.mark_ready();
        assert_eq!(
            apply_test(&dictation, DictationCommand::Start),
            DictationUpdate::Started
        );
        let first_id = activate_test_recording(&dictation);
        assert_eq!(
            apply_test(&dictation, DictationCommand::Cancel),
            DictationUpdate::Cancelled
        );
        assert_eq!(
            apply_test(&dictation, DictationCommand::Start),
            DictationUpdate::Started
        );
        let second_id = activate_test_recording(&dictation);

        assert_ne!(first_id, second_id);
        assert_eq!(
            dictation.record_samples(first_id, &[0.8]),
            RecordSamplesUpdate::Ignored
        );
        assert!(!dictation.abort_recording(first_id));
        assert_eq!(dictation.recording_id(), Some(second_id));
        assert_eq!(
            dictation.record_samples(second_id, &[0.2]),
            RecordSamplesUpdate::Recording
        );
    }

    #[test]
    fn abort_recording_returns_recording_to_idle() {
        let dictation = DictationControl::new();
        start_test_recording(&dictation, test_sample_rate());

        assert!(dictation.abort_recording(TEST_RECORDING_ID));

        assert_eq!(dictation.phase(), DictationPhase::Idle);
        assert!(take_test_utterance(&dictation).is_none());
    }

    #[test]
    fn abort_recording_ignores_initializing_idle_and_unavailable() {
        let dictation = test_dictation();
        assert!(!dictation.abort_recording(TEST_RECORDING_ID));
        assert_eq!(dictation.phase(), DictationPhase::Initializing);

        dictation.mark_ready();
        assert!(!dictation.abort_recording(TEST_RECORDING_ID));
        assert_eq!(dictation.phase(), DictationPhase::Idle);

        *dictation.state() = DictationControlState::Unavailable;
        assert!(!dictation.abort_recording(TEST_RECORDING_ID));
        assert_eq!(dictation.phase(), DictationPhase::Unavailable);
    }

    #[test]
    fn abort_recording_preserves_transcribing_utterance() {
        let dictation = DictationControl::new();
        start_test_recording(&dictation, test_sample_rate());
        assert_eq!(
            record_test_samples(&dictation, &[0.1, 0.2]),
            RecordSamplesUpdate::Recording
        );
        assert_eq!(
            apply_test(&dictation, DictationCommand::Stop),
            DictationUpdate::Stopped
        );

        assert!(!dictation.abort_recording(TEST_RECORDING_ID));
        assert_eq!(
            record_test_samples(&dictation, &[0.3]),
            RecordSamplesUpdate::Recording
        );

        assert_eq!(dictation.phase(), DictationPhase::Transcribing);
        assert!(dictation.begin_stopping());
        assert_eq!(dictation.finish_stopping(), FinishStopping::Ready);
        assert!(dictation.take_ready_dictation().is_none());
        assert!(dictation.begin_pending_transcription());
        let utterance = take_test_utterance(&dictation).expect("queued utterance survives");
        assert_eq!(utterance.samples(), &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn utterance_duration_uses_sample_rate() {
        let sample_rate = SampleRate::new(4).expect("non-zero sample rate");
        let utterance = CapturedUtterance::new(sample_rate, vec![0.0; 6])
            .expect("samples produce an utterance");

        assert_eq!(utterance.duration(), Duration::from_secs_f32(1.5));
    }
}
