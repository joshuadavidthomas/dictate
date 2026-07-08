mod commits;
mod discovery;
mod io;
mod protocol_state;
mod text_chunks;

use std::time::Duration;
use std::time::Instant;

use wayland_client::ConnectError;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::EventQueue;
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;

use self::commits::BufferedCommit;
use self::discovery::discover_registry;
use self::io::dispatch_ready_events;
use self::io::dispatch_until_event_or_timeout;
use self::io::flush_buffered_commit_request;
use self::io::wait_for_commit_flush_progress;
use self::io::wayland_protocol_failed;
use self::protocol_state::InputMethodEvent;
use self::protocol_state::InputMethodSession;
use self::protocol_state::State;
use self::text_chunks::CommitChunks;
use crate::insertion::InsertionAuthorityLoss;
use crate::insertion::InsertionBackend;
use crate::insertion::InsertionBackendKind;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionIoOperation;
use crate::insertion::InsertionOutcome;
use crate::insertion::InsertionProtocolFailureKind;
use crate::insertion::InsertionText;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WaylandInputMethodBackend {
    protocol_idle_timeout: Duration,
}

impl Default for WaylandInputMethodBackend {
    fn default() -> Self {
        Self::new(Self::DEFAULT_PROTOCOL_IDLE_TIMEOUT)
    }
}

impl WaylandInputMethodBackend {
    pub(crate) const DEFAULT_PROTOCOL_IDLE_TIMEOUT: Duration = Duration::from_millis(900);
    const PROTOCOL_ATTEMPT_BASE_IDLE_WINDOWS: u32 = 4;
    const PROTOCOL_ATTEMPT_IDLE_WINDOWS_PER_CHUNK: u32 = 1;

    pub(crate) const fn new(protocol_idle_timeout: Duration) -> Self {
        Self {
            protocol_idle_timeout,
        }
    }
}

impl InsertionBackend for WaylandInputMethodBackend {
    fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome {
        insert_with_input_method(text, self.protocol_idle_timeout)
    }
}

fn wayland_connect_failure(error: &ConnectError) -> InsertionFailure {
    match error {
        ConnectError::NoCompositor => InsertionFailure::BackendUnavailable {
            backend: InsertionBackendKind::WaylandInputMethod,
        },
        ConnectError::NoWaylandLib => wayland_protocol_failed(
            InsertionIoOperation::Connect,
            InsertionProtocolFailureKind::WaylandProtocol,
            "Wayland library could not be loaded",
        ),
        ConnectError::InvalidFd => wayland_protocol_failed(
            InsertionIoOperation::Connect,
            InsertionProtocolFailureKind::SessionState,
            "WAYLAND_SOCKET is invalid",
        ),
    }
}

fn insert_with_input_method(
    text: InsertionText<'_>,
    protocol_idle_timeout: Duration,
) -> InsertionOutcome {
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(error) => {
            return InsertionOutcome::NotInserted(wayland_connect_failure(&error));
        }
    };
    let discovered = match discover_registry(&connection, protocol_idle_timeout) {
        Ok(discovered) => discovered,
        Err(failure) => return InsertionOutcome::NotInserted(failure),
    };

    let mut event_queue = connection.new_event_queue();
    let qh = event_queue.handle();
    let chunks = CommitChunks::from_text(text);
    let protocol_attempt_timeout = protocol_attempt_timeout(protocol_idle_timeout, chunks.len());
    let mut state = State::new(chunks);

    // Retain the proxy for the insertion attempt; dropping it unregisters the event/request target.
    let input_method = discovered
        .manager
        .get_input_method(&discovered.seat, &qh, ());
    let mut progress_deadline = ProgressDeadline::new(
        protocol_idle_timeout,
        protocol_attempt_timeout,
        state.protocol_progress(),
    );
    while !state.is_finished() {
        if state.has_queued_commit() {
            let dispatch_deadline = progress_deadline.current();
            if let Err(failure) =
                dispatch_ready_events(&mut event_queue, &mut state, dispatch_deadline.instant)
            {
                return state.failure_outcome(dispatch_deadline.classify_timeout(failure));
            }
            progress_deadline.reset_after_progress(state.protocol_progress());
            if let Err(failure) = flush_next_pending_commit(
                &connection,
                &input_method,
                &mut event_queue,
                &mut state,
                &mut progress_deadline,
            ) {
                return state.failure_outcome(failure);
            }
            continue;
        }

        let dispatch_deadline = progress_deadline.current();
        if let Err(failure) = dispatch_until_event_or_timeout(
            &connection,
            &mut event_queue,
            &mut state,
            dispatch_deadline.instant,
        ) {
            return state.failure_outcome(dispatch_deadline.classify_timeout(failure));
        }
        progress_deadline.reset_after_progress(state.protocol_progress());
    }

    match state.session() {
        InputMethodSession::Failed(failure) => state.failure_outcome(
            progress_deadline
                .current()
                .classify_timeout(failure.clone().into_insert_failure()),
        ),
        InputMethodSession::SentToInputMethod => InsertionOutcome::Submitted {
            sent_bytes: state.sent_bytes(),
        },
        InputMethodSession::Unavailable => {
            state.failure_outcome(InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodUnavailable,
            })
        }
        InputMethodSession::WaitingInactive { .. }
        | InputMethodSession::ReadyToCommit { .. }
        | InputMethodSession::CommitQueued { .. }
        | InputMethodSession::CommitInFlight { .. } => {
            state.failure_outcome(InsertionFailure::IdleTimedOut {
                operation: InsertionIoOperation::ReadEvents,
            })
        }
    }
}

fn protocol_attempt_timeout(idle_timeout: Duration, chunk_count: usize) -> Duration {
    let chunk_count = u32::try_from(chunk_count).unwrap_or(u32::MAX);
    let chunk_windows = WaylandInputMethodBackend::PROTOCOL_ATTEMPT_IDLE_WINDOWS_PER_CHUNK
        .saturating_mul(chunk_count);
    idle_timeout.saturating_mul(
        WaylandInputMethodBackend::PROTOCOL_ATTEMPT_BASE_IDLE_WINDOWS.saturating_add(chunk_windows),
    )
}

#[derive(Debug)]
struct ProgressDeadline {
    idle_timeout: Duration,
    idle_deadline: Instant,
    attempt_deadline: Instant,
    observed_progress: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProgressTimeout {
    instant: Instant,
    kind: ProgressTimeoutKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProgressTimeoutKind {
    Idle,
    Attempt,
}

impl ProgressTimeout {
    fn classify_timeout(self, failure: InsertionFailure) -> InsertionFailure {
        if let InsertionFailure::IdleTimedOut { operation } = failure
            && matches!(self.kind, ProgressTimeoutKind::Attempt)
        {
            InsertionFailure::AttemptTimedOut { operation }
        } else {
            failure
        }
    }
}

impl ProgressDeadline {
    fn new(idle_timeout: Duration, attempt_timeout: Duration, observed_progress: u64) -> Self {
        Self::new_at(
            idle_timeout,
            attempt_timeout,
            observed_progress,
            Instant::now(),
        )
    }

    fn new_at(
        idle_timeout: Duration,
        attempt_timeout: Duration,
        observed_progress: u64,
        now: Instant,
    ) -> Self {
        Self {
            idle_timeout,
            idle_deadline: now + idle_timeout,
            attempt_deadline: now + attempt_timeout,
            observed_progress,
        }
    }

    fn current(&self) -> ProgressTimeout {
        if self.attempt_deadline <= self.idle_deadline {
            ProgressTimeout {
                instant: self.attempt_deadline,
                kind: ProgressTimeoutKind::Attempt,
            }
        } else {
            ProgressTimeout {
                instant: self.idle_deadline,
                kind: ProgressTimeoutKind::Idle,
            }
        }
    }

    fn reset_after_progress(&mut self, progress: u64) {
        if progress != self.observed_progress {
            self.observed_progress = progress;
            self.idle_deadline = Instant::now() + self.idle_timeout;
        }
    }

    fn reset_after_write_progress(&mut self) {
        self.idle_deadline = Instant::now() + self.idle_timeout;
    }
}

fn flush_next_pending_commit(
    connection: &Connection,
    input_method: &ZwpInputMethodV2,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    progress_deadline: &mut ProgressDeadline,
) -> Result<(), InsertionFailure> {
    let Some(request) = state.next_commit_request()? else {
        return Ok(());
    };
    input_method.commit_string(request.chunk.clone());
    input_method.commit(request.serial);
    let buffered = state.mark_commit_buffered(&request)?;
    let flush_deadline = progress_deadline.current();
    flush_commit_request(
        connection,
        event_queue,
        state,
        buffered,
        flush_deadline.instant,
    )
    .map_err(|failure| flush_deadline.classify_timeout(failure))?;
    progress_deadline.reset_after_write_progress();
    Ok(())
}

fn flush_commit_request(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    buffered: BufferedCommit,
    deadline: Instant,
) -> Result<(), InsertionFailure> {
    let flush_result = flush_buffered_commit_request(
        buffered,
        deadline,
        || connection.flush(),
        |deadline| wait_for_commit_flush_progress(connection, event_queue, state, deadline),
    );
    let flushed = flush_result.map_err(|failure| state.handle_commit_flush_failure(failure))?;
    state.commit_request_flushed(flushed)
}

impl Dispatch<ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        _input_method: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        (): &(),
        _connection: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                state.handle_input_method_event(InputMethodEvent::Activate);
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.handle_input_method_event(InputMethodEvent::Deactivate);
            }
            zwp_input_method_v2::Event::Done => {
                state.handle_input_method_event(InputMethodEvent::Done);
            }
            zwp_input_method_v2::Event::Unavailable => {
                state.handle_input_method_event(InputMethodEvent::Unavailable);
            }
            zwp_input_method_v2::Event::SurroundingText { .. }
            | zwp_input_method_v2::Event::TextChangeCause { .. }
            | zwp_input_method_v2::Event::ContentType { .. }
            | _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_deadline_resets_after_each_protocol_step_without_extending_attempt() {
        let timeout = Duration::from_secs(10);
        let attempt_timeout = Duration::from_secs(30);
        let now = Instant::now();
        let mut deadline = ProgressDeadline::new_at(timeout, attempt_timeout, 0, now);
        let initial = deadline.current();

        deadline.reset_after_progress(0);
        assert_eq!(deadline.current(), initial);

        deadline.reset_after_progress(1);
        assert!(deadline.current().instant >= initial.instant);
        assert!(deadline.current().instant <= now + attempt_timeout);
    }

    #[test]
    fn progress_deadline_reports_overall_attempt_timeout() {
        let timeout = Duration::from_millis(1);
        let now = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let deadline = ProgressDeadline::new_at(timeout, timeout, 0, now);

        assert!(matches!(
            deadline
                .current()
                .classify_timeout(InsertionFailure::IdleTimedOut {
                    operation: InsertionIoOperation::WaitReadable,
                }),
            InsertionFailure::AttemptTimedOut {
                operation: InsertionIoOperation::WaitReadable,
            }
        ));
    }

    #[test]
    fn protocol_attempt_timeout_scales_with_chunk_count() {
        let idle_timeout = Duration::from_secs(1);

        assert_eq!(
            protocol_attempt_timeout(idle_timeout, 1),
            idle_timeout.saturating_mul(5)
        );
        assert_eq!(
            protocol_attempt_timeout(idle_timeout, 3),
            idle_timeout.saturating_mul(7)
        );
    }
}
