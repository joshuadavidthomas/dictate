use std::collections::VecDeque;

use super::commits::BufferedCommit;
use super::commits::CommitRequest;
use super::io::BufferedCommitFlushFailure;
use super::text_chunks::ChunkQueue;
use super::text_chunks::CommitChunk;
use super::text_chunks::CommitChunks;
use crate::insertion::InsertionAuthorityLoss;
use crate::insertion::InsertionBackendFailure;
use crate::insertion::InsertionBackendKind;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionIoOperation;
use crate::insertion::InsertionOutcome;
use crate::insertion::InsertionProtocolFailureKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InputMethodSerial(u32);

impl InputMethodSerial {
    const INITIAL: Self = Self(0);

    fn advance(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    fn request_serial(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum InputMethodSession {
    WaitingInactive {
        serial: InputMethodSerial,
    },
    ReadyToCommit {
        serial: InputMethodSerial,
    },
    CommitQueued {
        serial: InputMethodSerial,
        current: CommitChunk,
        remaining: VecDeque<CommitChunk>,
    },
    CommitInFlight {
        next_serial: InputMethodSerial,
        remaining: VecDeque<CommitChunk>,
        buffered_sent_bytes: usize,
    },
    SentToInputMethod,
    Unavailable,
    Failed(InputMethodFailure),
}

impl Default for InputMethodSession {
    fn default() -> Self {
        Self::WaitingInactive {
            serial: InputMethodSerial::INITIAL,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum InputMethodFailure {
    Deactivated,
    Unavailable,
    SerialAdvancedAfterBuffer,
    Protocol {
        operation: InsertionIoOperation,
        message: String,
    },
    FlushFailed(InsertionBackendFailure),
}

impl InputMethodFailure {
    pub(super) fn into_insert_failure(self) -> InsertionFailure {
        match self {
            Self::Deactivated => InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodDeactivated,
            },
            Self::Unavailable => InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodUnavailable,
            },
            Self::SerialAdvancedAfterBuffer => InsertionFailure::BackendFailed {
                backend: InsertionBackendKind::WaylandInputMethod,
                failure: InsertionBackendFailure::Protocol {
                    operation: InsertionIoOperation::ReadEvents,
                    kind: InsertionProtocolFailureKind::SessionState,
                    message: "input-method serial advanced before buffered commit flushed"
                        .to_owned(),
                },
            },
            Self::Protocol { operation, message } => InsertionFailure::BackendFailed {
                backend: InsertionBackendKind::WaylandInputMethod,
                failure: InsertionBackendFailure::Protocol {
                    operation,
                    kind: InsertionProtocolFailureKind::SessionState,
                    message,
                },
            },
            Self::FlushFailed(failure) => InsertionFailure::BackendFailed {
                backend: InsertionBackendKind::WaylandInputMethod,
                failure,
            },
        }
    }

    pub(super) fn protocol(operation: InsertionIoOperation, message: impl Into<String>) -> Self {
        Self::Protocol {
            operation,
            message: message.into(),
        }
    }
}

impl InputMethodSession {
    fn has_queued_commit(&self) -> bool {
        matches!(self, Self::CommitQueued { .. })
    }

    fn next_commit_request(&self) -> Option<CommitRequest> {
        let Self::CommitQueued {
            serial, current, ..
        } = self
        else {
            return None;
        };
        Some(CommitRequest {
            serial: serial.request_serial(),
            chunk: current.chunk.clone(),
        })
    }

    fn validate_commit_request(&self, request: &CommitRequest) -> Result<(), InputMethodFailure> {
        let Self::CommitQueued {
            serial, current, ..
        } = self
        else {
            return Err(InputMethodFailure::protocol(
                InsertionIoOperation::FlushRequests,
                "commit requested outside queued input-method session",
            ));
        };
        if request.serial != serial.request_serial() || request.chunk != current.chunk {
            return Err(InputMethodFailure::protocol(
                InsertionIoOperation::FlushRequests,
                "commit request did not match the queued input-method commit",
            ));
        }
        Ok(())
    }

    fn commit_request_buffered(
        &mut self,
        request: &CommitRequest,
    ) -> Result<usize, InputMethodFailure> {
        self.validate_commit_request(request)?;
        let Self::CommitQueued {
            serial, remaining, ..
        } = self
        else {
            return Err(InputMethodFailure::protocol(
                InsertionIoOperation::FlushRequests,
                "validated queued commit was no longer queued",
            ));
        };
        let sent_bytes = request.sent_bytes();
        *self = Self::CommitInFlight {
            next_serial: *serial,
            remaining: std::mem::take(remaining),
            buffered_sent_bytes: sent_bytes,
        };
        Ok(sent_bytes)
    }

    fn commit_request_flushed(&mut self) -> Result<usize, InputMethodFailure> {
        let Self::CommitInFlight {
            next_serial,
            remaining,
            buffered_sent_bytes,
        } = self
        else {
            return Err(InputMethodFailure::protocol(
                InsertionIoOperation::FlushRequests,
                "commit flush completed outside in-flight input-method session",
            ));
        };
        let sent_bytes = *buffered_sent_bytes;
        if let Some(next) = remaining.pop_front() {
            *self = Self::CommitQueued {
                serial: *next_serial,
                current: next,
                remaining: std::mem::take(remaining),
            };
        } else {
            *self = Self::SentToInputMethod;
        }
        Ok(sent_bytes)
    }

    fn is_finished(&self) -> bool {
        !matches!(
            self,
            Self::WaitingInactive { .. }
                | Self::ReadyToCommit { .. }
                | Self::CommitQueued { .. }
                | Self::CommitInFlight { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InputMethodEvent {
    Activate,
    Deactivate,
    Done,
    Unavailable,
}

pub(super) struct State {
    chunks: ChunkQueue,
    session: InputMethodSession,
    protocol_progress: u64,
}

impl State {
    pub(super) fn new(chunks: CommitChunks) -> Self {
        Self {
            chunks: ChunkQueue::new(chunks),
            session: InputMethodSession::default(),
            protocol_progress: 0,
        }
    }

    pub(super) fn session(&self) -> &InputMethodSession {
        &self.session
    }

    pub(super) fn is_finished(&self) -> bool {
        self.session.is_finished()
    }

    pub(super) fn has_queued_commit(&self) -> bool {
        self.session.has_queued_commit()
    }

    pub(super) fn protocol_progress(&self) -> u64 {
        self.protocol_progress
    }

    pub(super) fn sent_bytes(&self) -> usize {
        self.chunks.sent_bytes()
    }

    fn take_commit_batch(&mut self) -> Option<super::text_chunks::CommitBatch> {
        self.chunks.take_commit_batch()
    }

    pub(super) fn next_commit_request(
        &mut self,
    ) -> Result<Option<CommitRequest>, InsertionFailure> {
        let Some(request) = self.session.next_commit_request() else {
            return Ok(None);
        };
        self.session
            .validate_commit_request(&request)
            .map_err(|failure| {
                self.session = InputMethodSession::Failed(failure.clone());
                failure.into_insert_failure()
            })?;
        Ok(Some(request))
    }

    pub(super) fn mark_commit_buffered(
        &mut self,
        request: &CommitRequest,
    ) -> Result<BufferedCommit, InsertionFailure> {
        let bytes = self
            .session
            .commit_request_buffered(request)
            .map_err(|failure| {
                self.session = InputMethodSession::Failed(failure.clone());
                failure.into_insert_failure()
            })?;
        self.chunks.record_commit_buffered(bytes);
        Ok(BufferedCommit::new(bytes))
    }

    pub(super) fn commit_request_flushed(
        &mut self,
        buffered: BufferedCommit,
    ) -> Result<(), InsertionFailure> {
        let buffered_sent_bytes = buffered.into_sent_bytes();
        let bytes = self.session.commit_request_flushed().map_err(|failure| {
            self.session = InputMethodSession::Failed(failure.clone());
            failure.into_insert_failure()
        })?;
        if bytes != buffered_sent_bytes {
            let failure = InputMethodFailure::protocol(
                InsertionIoOperation::FlushRequests,
                "flushed commit bytes did not match buffered commit",
            );
            self.session = InputMethodSession::Failed(failure.clone());
            return Err(failure.into_insert_failure());
        }
        self.chunks.record_commit_flushed(bytes);
        Ok(())
    }

    pub(super) fn handle_commit_flush_failure(
        &mut self,
        failure: BufferedCommitFlushFailure,
    ) -> InsertionFailure {
        match failure {
            BufferedCommitFlushFailure::FlushFailed(failure) => {
                self.session =
                    InputMethodSession::Failed(InputMethodFailure::FlushFailed(failure.clone()));
                InsertionFailure::BackendFailed {
                    backend: InsertionBackendKind::WaylandInputMethod,
                    failure,
                }
            }
            BufferedCommitFlushFailure::Interrupted(failure) => failure,
        }
    }

    fn record_protocol_progress(&mut self) {
        self.protocol_progress += 1;
    }

    fn handle_activate(&mut self) {
        if let InputMethodSession::WaitingInactive { serial } = &self.session {
            self.session = InputMethodSession::ReadyToCommit { serial: *serial };
        }
    }

    fn handle_deactivate(&mut self) {
        match &self.session {
            InputMethodSession::ReadyToCommit { serial } => {
                self.session = InputMethodSession::WaitingInactive { serial: *serial };
            }
            InputMethodSession::CommitQueued { .. } | InputMethodSession::CommitInFlight { .. } => {
                self.session = InputMethodSession::Failed(InputMethodFailure::Deactivated);
            }
            InputMethodSession::WaitingInactive { .. }
            | InputMethodSession::SentToInputMethod
            | InputMethodSession::Unavailable
            | InputMethodSession::Failed(_) => {}
        }
    }

    fn handle_unavailable(&mut self) {
        match &self.session {
            InputMethodSession::WaitingInactive { .. }
            | InputMethodSession::ReadyToCommit { .. } => {
                self.session = InputMethodSession::Unavailable;
            }
            InputMethodSession::CommitQueued { .. } | InputMethodSession::CommitInFlight { .. } => {
                self.session = InputMethodSession::Failed(InputMethodFailure::Unavailable);
            }
            InputMethodSession::SentToInputMethod
            | InputMethodSession::Unavailable
            | InputMethodSession::Failed(_) => {}
        }
    }

    fn handle_done(&mut self) {
        if let InputMethodSession::ReadyToCommit { serial } = &self.session {
            let serial = serial.advance();
            let Some(batch) = self.take_commit_batch() else {
                self.session = InputMethodSession::Failed(InputMethodFailure::protocol(
                    InsertionIoOperation::ReadEvents,
                    "missing queued insertion chunk",
                ));
                return;
            };
            self.session = InputMethodSession::CommitQueued {
                serial,
                current: batch.current,
                remaining: batch.remaining,
            };
            return;
        }

        match &mut self.session {
            InputMethodSession::WaitingInactive { serial }
            | InputMethodSession::CommitQueued { serial, .. } => {
                *serial = serial.advance();
            }
            InputMethodSession::CommitInFlight { .. } => {
                self.session =
                    InputMethodSession::Failed(InputMethodFailure::SerialAdvancedAfterBuffer);
            }
            InputMethodSession::ReadyToCommit { .. }
            | InputMethodSession::SentToInputMethod
            | InputMethodSession::Unavailable
            | InputMethodSession::Failed(_) => {}
        }
    }

    pub(super) fn handle_input_method_event(&mut self, event: InputMethodEvent) {
        self.record_protocol_progress();
        match event {
            InputMethodEvent::Activate => {
                self.handle_activate();
            }
            InputMethodEvent::Deactivate => {
                self.handle_deactivate();
            }
            InputMethodEvent::Done => {
                self.handle_done();
            }
            InputMethodEvent::Unavailable => {
                self.handle_unavailable();
            }
        }
    }

    pub(super) fn failure_outcome(&self, failure: InsertionFailure) -> InsertionOutcome {
        if self.chunks.maybe_sent_bytes() > 0 {
            InsertionOutcome::DeliveryUncertain {
                maybe_sent_bytes: self.chunks.maybe_sent_bytes(),
                failure,
            }
        } else {
            InsertionOutcome::NotInserted(failure)
        }
    }
}

pub(super) fn interrupted_commit_failure(state: &State) -> Option<InsertionFailure> {
    match state.session() {
        InputMethodSession::CommitInFlight { .. } => None,
        InputMethodSession::Failed(failure) => Some(failure.clone().into_insert_failure()),
        InputMethodSession::WaitingInactive { .. }
        | InputMethodSession::ReadyToCommit { .. }
        | InputMethodSession::CommitQueued { .. }
        | InputMethodSession::SentToInputMethod
        | InputMethodSession::Unavailable => Some(
            InputMethodFailure::protocol(
                InsertionIoOperation::FlushRequests,
                "input-method commit was interrupted before flush completed",
            )
            .into_insert_failure(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insertion::InsertionTargetKind;

    fn state_with_chunks(chunks: &[&str]) -> State {
        State::new(CommitChunks::from_test_chunks(
            chunks.iter().map(|chunk| (*chunk).to_owned()),
        ))
    }

    fn begin_and_buffer_commit(state: &mut State) -> (CommitRequest, BufferedCommit) {
        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        let request = state
            .next_commit_request()
            .expect("commit request should be valid")
            .expect("done should queue a commit request");
        let buffered = state
            .mark_commit_buffered(&request)
            .expect("commit request should be buffered");
        (request, buffered)
    }

    #[test]
    fn input_method_serial_wraps_explicitly() {
        assert_eq!(InputMethodSerial(u32::MAX).advance(), InputMethodSerial(0));
    }

    #[test]
    fn final_chunk_request_is_sent_after_flush() {
        let mut state = state_with_chunks(&["hello"]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");
        assert_eq!(state.session, InputMethodSession::SentToInputMethod);
        assert!(state.session.is_finished());
    }

    #[test]
    fn one_done_serial_can_send_all_chunks() {
        let mut state = state_with_chunks(&["hello", " world"]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);

        let InputMethodSession::CommitQueued {
            serial,
            current,
            remaining,
        } = &state.session
        else {
            panic!("done should queue all chunks for commit");
        };
        assert_eq!(serial.request_serial(), 1);
        assert_eq!(current.chunk, "hello");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].chunk, " world");
    }

    #[test]
    fn queued_commit_flush_advances_to_remaining_then_sent() {
        let mut state = state_with_chunks(&["hello", " world"]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");
        let InputMethodSession::CommitQueued {
            current, remaining, ..
        } = &state.session
        else {
            panic!("first flush should advance to the second queued commit");
        };
        assert_eq!(current.chunk, " world");
        assert!(remaining.is_empty());

        let request = state
            .next_commit_request()
            .expect("remaining commit request should be valid")
            .expect("remaining queued commit should become in-flight");
        let buffered = state
            .mark_commit_buffered(&request)
            .expect("remaining commit request should be buffered");
        assert_eq!(request.chunk, " world");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");
        assert_eq!(state.session, InputMethodSession::SentToInputMethod);
    }

    #[test]
    fn done_while_queued_refreshes_remaining_commit_serials() {
        let mut state = state_with_chunks(&["hello", " world"]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        state.handle_input_method_event(InputMethodEvent::Done);

        let InputMethodSession::CommitQueued {
            serial,
            current,
            remaining,
        } = &state.session
        else {
            panic!("second done should preserve the queued commit state");
        };
        assert_eq!(serial.request_serial(), 2);
        assert_eq!(current.chunk, "hello");
        assert_eq!(remaining[0].chunk, " world");
    }

    #[test]
    fn done_while_in_flight_marks_buffered_commit_stale() {
        let mut state = state_with_chunks(&["hello", " world"]);
        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        let in_flight = state
            .next_commit_request()
            .expect("commit request should be valid")
            .expect("queued commit should become in-flight");
        let _buffered = state
            .mark_commit_buffered(&in_flight)
            .expect("commit request should be buffered");
        assert_eq!(in_flight.serial, 1);

        state.handle_input_method_event(InputMethodEvent::Done);

        assert_eq!(
            state.session,
            InputMethodSession::Failed(InputMethodFailure::SerialAdvancedAfterBuffer)
        );
    }

    #[test]
    fn queued_commits_are_rejected_when_deactivated_before_flush() {
        let mut state = state_with_chunks(&["hello"]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        state.handle_input_method_event(InputMethodEvent::Deactivate);

        assert_eq!(
            state.session,
            InputMethodSession::Failed(InputMethodFailure::Deactivated)
        );
        assert!(matches!(
            state.failure_outcome(InputMethodFailure::Deactivated.into_insert_failure()),
            InsertionOutcome::NotInserted(_)
        ));
    }

    #[test]
    fn unavailable_finishes_without_commit() {
        let mut state = state_with_chunks(&["hello"]);

        state.handle_input_method_event(InputMethodEvent::Unavailable);

        assert_eq!(state.session, InputMethodSession::Unavailable);
        assert!(state.session.is_finished());
    }

    #[test]
    fn event_driver_reports_unavailable_before_bytes_as_not_inserted() {
        let mut state = state_with_chunks(&["hello"]);

        state.handle_input_method_event(InputMethodEvent::Unavailable);

        assert!(matches!(
            state.failure_outcome(InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodUnavailable,
            }),
            InsertionOutcome::NotInserted(InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodUnavailable,
            })
        ));
    }

    #[test]
    fn event_driver_reports_first_flush_failure_as_uncertain_after_request_is_queued() {
        let mut state = state_with_chunks(&["hello"]);

        let (request, _buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");

        let InsertionOutcome::DeliveryUncertain {
            maybe_sent_bytes,
            failure,
        } = state.failure_outcome(InsertionFailure::BackendFailed {
            backend: InsertionBackendKind::WaylandInputMethod,
            failure: InsertionBackendFailure::Io {
                operation: InsertionIoOperation::FlushRequests,
                kind: std::io::ErrorKind::Other,
                message: "flush failed".to_owned(),
            },
        })
        else {
            panic!("expected uncertain insertion after queueing commit request");
        };
        assert_eq!(maybe_sent_bytes, "hello".len());
        assert_eq!(
            failure.to_string(),
            "text insertion backend failed: Wayland input-method: IO while flushing requests: flush failed (Other)"
        );
    }

    #[test]
    fn event_driver_reports_failure_after_flushed_chunk_as_uncertain() {
        let mut state = state_with_chunks(&["hello", " world"]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.serial, 1);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");

        let InsertionOutcome::DeliveryUncertain {
            maybe_sent_bytes,
            failure,
        } = state.failure_outcome(InsertionFailure::BackendFailed {
            backend: InsertionBackendKind::WaylandInputMethod,
            failure: InsertionBackendFailure::Protocol {
                operation: InsertionIoOperation::ReadEvents,
                kind: InsertionProtocolFailureKind::WaylandProtocol,
                message: "lost compositor".to_owned(),
            },
        })
        else {
            panic!("expected uncertain insertion outcome");
        };
        assert_eq!(maybe_sent_bytes, "hello".len());
        assert_eq!(
            failure.to_string(),
            "text insertion backend failed: Wayland input-method: protocol while reading events: lost compositor (Wayland protocol)"
        );
    }

    #[test]
    fn event_driver_reports_timeout_before_bytes_as_not_inserted() {
        let state = state_with_chunks(&["hello"]);

        assert!(matches!(
            state.failure_outcome(InsertionFailure::IdleTimedOut {
                operation: InsertionIoOperation::WaitReadable,
            }),
            InsertionOutcome::NotInserted(InsertionFailure::IdleTimedOut {
                operation: InsertionIoOperation::WaitReadable,
            })
        ));
    }

    #[test]
    fn missing_registry_parts_are_fallback_safe_before_bytes() {
        let state = state_with_chunks(&["hello"]);

        assert!(matches!(
            state.failure_outcome(InsertionFailure::BackendUnavailable {
                backend: InsertionBackendKind::WaylandInputMethod,
            }),
            InsertionOutcome::NotInserted(InsertionFailure::BackendUnavailable {
                backend: InsertionBackendKind::WaylandInputMethod,
            })
        ));
        assert!(matches!(
            state.failure_outcome(InsertionFailure::TargetUnavailable {
                target: InsertionTargetKind::Seat,
            }),
            InsertionOutcome::NotInserted(InsertionFailure::TargetUnavailable {
                target: InsertionTargetKind::Seat,
            })
        ));
    }

    #[test]
    fn deactivate_before_done_does_not_commit() {
        let mut state = state_with_chunks(&["hello"]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Deactivate);
        state.handle_input_method_event(InputMethodEvent::Done);

        assert_eq!(
            state.session,
            InputMethodSession::WaitingInactive {
                serial: InputMethodSerial(1),
            }
        );
        assert!(!state.session.is_finished());
    }

    #[test]
    fn unavailable_after_sent_does_not_reclassify_success() {
        let mut state = state_with_chunks(&["hello"]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");
        state.handle_input_method_event(InputMethodEvent::Unavailable);

        assert_eq!(state.session, InputMethodSession::SentToInputMethod);
    }
}
