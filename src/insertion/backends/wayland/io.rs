use std::fmt;
use std::io;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::event::poll;
use rustix::io::Errno;
use wayland_client::Connection;
use wayland_client::DispatchError;
use wayland_client::EventQueue;
use wayland_client::backend::WaylandError;

use super::commits::BufferedCommit;
use super::protocol_state::State;
use super::protocol_state::interrupted_commit_failure;
use crate::insertion::InsertionBackendFailure;
use crate::insertion::InsertionBackendKind;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionIoOperation;
use crate::insertion::InsertionProtocolFailureKind;

pub(super) fn dispatch_until_event_or_timeout<QueueState>(
    connection: &Connection,
    event_queue: &mut EventQueue<QueueState>,
    state: &mut QueueState,
    deadline: Instant,
) -> Result<(), InsertionFailure> {
    loop {
        ensure_deadline_active(deadline, InsertionIoOperation::ReadEvents)?;
        let dispatched = event_queue
            .dispatch_pending(state)
            .map_err(dispatch_failed)?;
        if dispatched > 0 {
            return Ok(());
        }

        if matches!(
            flush_event_queue(connection, event_queue, state, deadline)?,
            EventQueueFlushProgress::EventDispatched
        ) {
            return Ok(());
        }
        let Some(read_guard) = event_queue.prepare_read() else {
            continue;
        };

        poll_wayland_fd(
            read_guard.connection_fd(),
            PollFlags::IN,
            PollDeadline::Until(deadline),
            InsertionIoOperation::WaitReadable,
        )?;
        read_events_or_retry(read_guard.read())?;
    }
}

pub(super) fn dispatch_ready_events(
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertionFailure> {
    loop {
        ensure_deadline_active(deadline, InsertionIoOperation::ReadEvents)?;
        let dispatched = event_queue
            .dispatch_pending(state)
            .map_err(dispatch_failed)?;
        if dispatched > 0 {
            return Ok(());
        }

        let Some(read_guard) = event_queue.prepare_read() else {
            continue;
        };
        let ready = poll_wayland_fd(
            read_guard.connection_fd(),
            PollFlags::IN,
            PollDeadline::Now,
            InsertionIoOperation::WaitReadable,
        )?;
        if !ready.readable {
            drop(read_guard);
            return Ok(());
        }
        read_events_or_retry(read_guard.read())?;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaylandReadAttempt {
    Completed,
    Interrupted,
    WouldBlock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FlushWaitContext {
    writable_observed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlushWaitReadAction {
    DispatchPending,
    RetryFlush,
    ContinueWaiting,
}

fn classify_wayland_read(
    result: Result<usize, WaylandError>,
) -> Result<WaylandReadAttempt, InsertionFailure> {
    match result {
        Ok(_) => Ok(WaylandReadAttempt::Completed),
        Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
            Ok(WaylandReadAttempt::Interrupted)
        }
        Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
            Ok(WaylandReadAttempt::WouldBlock)
        }
        Err(error) => Err(wayland_error_failed(
            InsertionIoOperation::ReadEvents,
            error,
        )),
    }
}

fn read_events_or_retry(result: Result<usize, WaylandError>) -> Result<bool, InsertionFailure> {
    match classify_wayland_read(result)? {
        WaylandReadAttempt::Completed => Ok(true),
        WaylandReadAttempt::Interrupted | WaylandReadAttempt::WouldBlock => Ok(false),
    }
}

fn flush_wait_read_action(
    result: Result<usize, WaylandError>,
    context: FlushWaitContext,
) -> Result<FlushWaitReadAction, InsertionFailure> {
    match classify_wayland_read(result)? {
        WaylandReadAttempt::Completed => Ok(FlushWaitReadAction::DispatchPending),
        WaylandReadAttempt::WouldBlock if context.writable_observed => {
            Ok(FlushWaitReadAction::RetryFlush)
        }
        WaylandReadAttempt::Interrupted | WaylandReadAttempt::WouldBlock => {
            Ok(FlushWaitReadAction::ContinueWaiting)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventQueueFlushProgress {
    Flushed,
    Writable,
    EventDispatched,
}

fn flush_event_queue<QueueState>(
    connection: &Connection,
    event_queue: &mut EventQueue<QueueState>,
    state: &mut QueueState,
    deadline: Instant,
) -> Result<EventQueueFlushProgress, InsertionFailure> {
    loop {
        ensure_deadline_active(deadline, InsertionIoOperation::FlushRequests)?;
        match event_queue.flush() {
            Ok(()) => return Ok(EventQueueFlushProgress::Flushed),
            Err(error) => match flush_retry(&error) {
                FlushRetry::RetryNow => {
                    ensure_deadline_active(deadline, InsertionIoOperation::FlushRequests)?;
                }
                FlushRetry::WaitWritable => {
                    if matches!(
                        wait_for_event_queue_flush_progress(
                            connection,
                            event_queue,
                            state,
                            deadline
                        )?,
                        EventQueueFlushProgress::EventDispatched
                    ) {
                        return Ok(EventQueueFlushProgress::EventDispatched);
                    }
                }
                FlushRetry::Fail => {
                    return Err(wayland_error_failed(
                        InsertionIoOperation::FlushRequests,
                        error,
                    ));
                }
            },
        }
    }
}

fn wait_for_event_queue_flush_progress<QueueState>(
    connection: &Connection,
    event_queue: &mut EventQueue<QueueState>,
    state: &mut QueueState,
    deadline: Instant,
) -> Result<EventQueueFlushProgress, InsertionFailure> {
    loop {
        ensure_deadline_active(deadline, InsertionIoOperation::WaitWritable)?;
        let Some(read_guard) = event_queue.prepare_read() else {
            let dispatched = event_queue
                .dispatch_pending(state)
                .map_err(dispatch_failed)?;
            if dispatched > 0 {
                return Ok(EventQueueFlushProgress::EventDispatched);
            }
            continue;
        };

        let backend = connection.backend();
        let ready = poll_wayland_fd(
            backend.poll_fd(),
            PollFlags::IN | PollFlags::OUT,
            PollDeadline::Until(deadline),
            InsertionIoOperation::WaitWritable,
        )?;
        if ready.readable {
            match flush_wait_read_action(
                read_guard.read(),
                FlushWaitContext {
                    writable_observed: ready.writable,
                },
            )? {
                FlushWaitReadAction::DispatchPending => {
                    let dispatched = event_queue
                        .dispatch_pending(state)
                        .map_err(dispatch_failed)?;
                    if dispatched > 0 {
                        return Ok(EventQueueFlushProgress::EventDispatched);
                    }
                    if ready.writable {
                        return Ok(EventQueueFlushProgress::Writable);
                    }
                }
                FlushWaitReadAction::RetryFlush => {
                    return Ok(EventQueueFlushProgress::Writable);
                }
                FlushWaitReadAction::ContinueWaiting => {}
            }
        } else {
            drop(read_guard);
            if ready.writable {
                return Ok(EventQueueFlushProgress::Writable);
            }
        }
    }
}

pub(super) fn wait_for_commit_flush_progress(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertionFailure> {
    loop {
        ensure_deadline_active(deadline, InsertionIoOperation::WaitWritable)?;
        let Some(read_guard) = event_queue.prepare_read() else {
            event_queue
                .dispatch_pending(state)
                .map_err(dispatch_failed)?;
            if let Some(failure) = interrupted_commit_failure(state) {
                return Err(failure);
            }
            continue;
        };

        let backend = connection.backend();
        let ready = poll_wayland_fd(
            backend.poll_fd(),
            PollFlags::IN | PollFlags::OUT,
            PollDeadline::Until(deadline),
            InsertionIoOperation::WaitWritable,
        )?;
        match commit_flush_wait_action(ready) {
            CommitFlushWaitAction::RetryFlush => {
                drop(read_guard);
                return Ok(());
            }
            CommitFlushWaitAction::ReadEvents => {
                match flush_wait_read_action(
                    read_guard.read(),
                    FlushWaitContext {
                        writable_observed: ready.writable,
                    },
                )? {
                    FlushWaitReadAction::DispatchPending => {
                        event_queue
                            .dispatch_pending(state)
                            .map_err(dispatch_failed)?;
                        if let Some(failure) = interrupted_commit_failure(state) {
                            return Err(failure);
                        }
                        if matches!(
                            commit_flush_dispatched_action(ready),
                            CommitFlushDispatchedAction::RetryFlush
                        ) {
                            return Ok(());
                        }
                    }
                    FlushWaitReadAction::RetryFlush => {
                        return Ok(());
                    }
                    FlushWaitReadAction::ContinueWaiting => {}
                }
            }
            CommitFlushWaitAction::Continue => {
                drop(read_guard);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitFlushWaitAction {
    RetryFlush,
    ReadEvents,
    Continue,
}

fn commit_flush_wait_action(ready: FdReady) -> CommitFlushWaitAction {
    if ready.readable {
        CommitFlushWaitAction::ReadEvents
    } else if ready.writable {
        CommitFlushWaitAction::RetryFlush
    } else {
        CommitFlushWaitAction::Continue
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitFlushDispatchedAction {
    RetryFlush,
    ContinueWaiting,
}

fn commit_flush_dispatched_action(ready: FdReady) -> CommitFlushDispatchedAction {
    if ready.writable {
        CommitFlushDispatchedAction::RetryFlush
    } else {
        CommitFlushDispatchedAction::ContinueWaiting
    }
}

#[derive(Debug)]
pub(super) enum BufferedCommitFlushFailure {
    FlushFailed(InsertionBackendFailure),
    Interrupted(InsertionFailure),
}

pub(super) fn flush_buffered_commit_request(
    buffered: BufferedCommit,
    deadline: Instant,
    mut flush: impl FnMut() -> Result<(), WaylandError>,
    mut wait_writable: impl FnMut(Instant) -> Result<(), InsertionFailure>,
) -> Result<BufferedCommit, BufferedCommitFlushFailure> {
    loop {
        ensure_deadline_active(deadline, InsertionIoOperation::FlushRequests)
            .map_err(BufferedCommitFlushFailure::Interrupted)?;
        match flush() {
            Ok(()) => return Ok(buffered),
            Err(error) => match flush_retry(&error) {
                FlushRetry::RetryNow => {
                    ensure_deadline_active(deadline, InsertionIoOperation::FlushRequests)
                        .map_err(BufferedCommitFlushFailure::Interrupted)?;
                }
                FlushRetry::WaitWritable => {
                    wait_writable(deadline).map_err(BufferedCommitFlushFailure::Interrupted)?;
                }
                FlushRetry::Fail => {
                    return Err(BufferedCommitFlushFailure::FlushFailed(
                        backend_failure_from_wayland_error(
                            InsertionIoOperation::FlushRequests,
                            error,
                        ),
                    ));
                }
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlushRetry {
    RetryNow,
    WaitWritable,
    Fail,
}

fn ensure_deadline_active(
    deadline: Instant,
    operation: InsertionIoOperation,
) -> Result<(), InsertionFailure> {
    if Instant::now() >= deadline {
        Err(InsertionFailure::IdleTimedOut { operation })
    } else {
        Ok(())
    }
}

fn flush_retry(error: &WaylandError) -> FlushRetry {
    let WaylandError::Io(error) = error else {
        return FlushRetry::Fail;
    };

    let kind = error.kind();
    if kind == std::io::ErrorKind::Interrupted {
        FlushRetry::RetryNow
    } else if kind == std::io::ErrorKind::WouldBlock {
        FlushRetry::WaitWritable
    } else {
        FlushRetry::Fail
    }
}

fn insertion_io_failed(
    operation: InsertionIoOperation,
    error: impl fmt::Display,
) -> InsertionFailure {
    InsertionFailure::BackendFailed {
        backend: InsertionBackendKind::WaylandInputMethod,
        failure: InsertionBackendFailure::Io {
            operation,
            kind: io::ErrorKind::Other,
            message: error.to_string(),
        },
    }
}

pub(super) fn wayland_protocol_failed(
    operation: InsertionIoOperation,
    kind: InsertionProtocolFailureKind,
    error: impl fmt::Display,
) -> InsertionFailure {
    InsertionFailure::BackendFailed {
        backend: InsertionBackendKind::WaylandInputMethod,
        failure: InsertionBackendFailure::Protocol {
            operation,
            kind,
            message: error.to_string(),
        },
    }
}

fn wayland_error_failed(operation: InsertionIoOperation, error: WaylandError) -> InsertionFailure {
    InsertionFailure::BackendFailed {
        backend: InsertionBackendKind::WaylandInputMethod,
        failure: backend_failure_from_wayland_error(operation, error),
    }
}

fn backend_failure_from_wayland_error(
    operation: InsertionIoOperation,
    error: WaylandError,
) -> InsertionBackendFailure {
    match error {
        WaylandError::Io(error) => InsertionBackendFailure::Io {
            operation,
            kind: error.kind(),
            message: error.to_string(),
        },
        WaylandError::Protocol(error) => InsertionBackendFailure::Protocol {
            operation,
            kind: InsertionProtocolFailureKind::WaylandProtocol,
            message: error.to_string(),
        },
    }
}

fn dispatch_failed(error: DispatchError) -> InsertionFailure {
    match error {
        DispatchError::Backend(error) => {
            wayland_error_failed(InsertionIoOperation::ReadEvents, error)
        }
        error @ DispatchError::BadMessage { .. } => wayland_protocol_failed(
            InsertionIoOperation::ReadEvents,
            InsertionProtocolFailureKind::BadMessage,
            error,
        ),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollDeadline {
    Now,
    Until(Instant),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FdReady {
    readable: bool,
    writable: bool,
}

fn poll_wayland_fd(
    fd: std::os::fd::BorrowedFd<'_>,
    interest: PollFlags,
    deadline: PollDeadline,
    operation: InsertionIoOperation,
) -> Result<FdReady, InsertionFailure> {
    loop {
        let timeout = match deadline {
            PollDeadline::Now => Timespec::default(),
            PollDeadline::Until(deadline) => {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return Err(InsertionFailure::IdleTimedOut { operation });
                };
                if remaining.is_zero() {
                    return Err(InsertionFailure::IdleTimedOut { operation });
                }
                Timespec::try_from(remaining)
                    .map_err(|error| insertion_io_failed(operation, error))?
            }
        };

        let mut poll_fds = [PollFd::from_borrowed_fd(fd, interest)];
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => {
                return match deadline {
                    PollDeadline::Now => Ok(FdReady {
                        readable: false,
                        writable: false,
                    }),
                    PollDeadline::Until(_) => Err(InsertionFailure::IdleTimedOut { operation }),
                };
            }
            Ok(_) => {
                let ready = poll_fds[0].revents();
                if ready.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
                    return Err(insertion_io_failed(
                        operation,
                        format_args!("Wayland fd reported {ready:?}"),
                    ));
                }
                return Ok(FdReady {
                    readable: ready.intersects(PollFlags::IN),
                    writable: ready.intersects(PollFlags::OUT),
                });
            }
            Err(Errno::INTR) => {}
            Err(error) => {
                return Err(insertion_io_failed(operation, error));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::super::commits::BufferedCommit;
    use super::super::commits::CommitRequest;
    use super::super::protocol_state::InputMethodEvent;
    use super::super::protocol_state::InputMethodFailure;
    use super::super::protocol_state::InputMethodSession;
    use super::super::protocol_state::State;
    use super::super::text_chunks::CommitChunks;
    use super::*;
    use crate::insertion::InsertionAuthorityLoss;
    use crate::insertion::InsertionOutcome;

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
    fn flush_wait_policy_reads_events_before_retrying_when_also_writable() {
        assert_eq!(
            commit_flush_wait_action(FdReady {
                readable: true,
                writable: true,
            }),
            CommitFlushWaitAction::ReadEvents
        );
        assert_eq!(
            commit_flush_wait_action(FdReady {
                readable: true,
                writable: false,
            }),
            CommitFlushWaitAction::ReadEvents
        );
        assert_eq!(
            commit_flush_wait_action(FdReady {
                readable: false,
                writable: true,
            }),
            CommitFlushWaitAction::RetryFlush
        );
    }

    #[test]
    fn readable_and_writable_commit_flush_retries_after_dispatching_events() {
        assert_eq!(
            commit_flush_dispatched_action(FdReady {
                readable: true,
                writable: true,
            }),
            CommitFlushDispatchedAction::RetryFlush
        );
        assert_eq!(
            commit_flush_dispatched_action(FdReady {
                readable: true,
                writable: false,
            }),
            CommitFlushDispatchedAction::ContinueWaiting
        );
    }

    #[test]
    fn read_wouldblock_preserves_writable_flush_progress() {
        let context = FlushWaitContext {
            writable_observed: true,
        };
        assert_eq!(
            flush_wait_read_action(
                Err(WaylandError::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "readiness race",
                ))),
                context,
            )
            .expect("wouldblock should be retryable"),
            FlushWaitReadAction::RetryFlush
        );
        assert_eq!(
            flush_wait_read_action(
                Err(WaylandError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "interrupted",
                ))),
                context,
            )
            .expect("interrupted should retry the read path"),
            FlushWaitReadAction::ContinueWaiting
        );
    }

    #[test]
    fn flush_errors_map_to_specific_retry_actions() {
        assert_eq!(
            flush_retry(&WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "socket backpressure",
            ))),
            FlushRetry::WaitWritable
        );
        assert_eq!(
            flush_retry(&WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "interrupted",
            ))),
            FlushRetry::RetryNow
        );
        assert_eq!(
            flush_retry(&WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed",
            ))),
            FlushRetry::Fail
        );
    }

    #[test]
    fn flush_loop_waits_on_wouldblock_and_marks_request_once() {
        let mut state = state_with_chunks(&["hello"]);
        let mut flushes = VecDeque::from([
            Err(WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "socket backpressure",
            ))),
            Ok(()),
        ]);
        let mut wait_calls = 0;
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        let flushed_request = flush_buffered_commit_request(
            buffered,
            Instant::now() + Duration::from_secs(1),
            || flushes.pop_front().expect("flush action should exist"),
            |_| {
                wait_calls += 1;
                Ok(())
            },
        )
        .expect("flush should succeed after writable wait");
        state
            .commit_request_flushed(flushed_request)
            .expect("flushed request should update state");

        assert_eq!(wait_calls, 1);
        assert_eq!(state.sent_bytes(), "hello".len());
    }

    #[test]
    fn flush_loop_retries_interrupted_without_waiting() {
        let mut state = state_with_chunks(&["hello"]);
        let mut flushes = VecDeque::from([
            Err(WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "interrupted",
            ))),
            Ok(()),
        ]);
        let mut wait_calls = 0;
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        let flushed_request = flush_buffered_commit_request(
            buffered,
            Instant::now() + Duration::from_secs(1),
            || flushes.pop_front().expect("flush action should exist"),
            |_| {
                wait_calls += 1;
                Ok(())
            },
        )
        .expect("interrupted flush should retry immediately");
        state
            .commit_request_flushed(flushed_request)
            .expect("flushed request should update state");

        assert_eq!(wait_calls, 0);
        assert_eq!(state.sent_bytes(), "hello".len());
    }

    #[test]
    fn flush_loop_deactivate_after_buffer_marks_delivery_uncertain() {
        let mut state = state_with_chunks(&["hello"]);
        let (_request, buffered) = begin_and_buffer_commit(&mut state);
        let mut flushes = VecDeque::from([
            Err(WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "socket backpressure",
            ))),
            Ok(()),
        ]);

        let failure = flush_buffered_commit_request(
            buffered,
            Instant::now() + Duration::from_secs(1),
            || flushes.pop_front().expect("flush action should exist"),
            |_| {
                state.handle_input_method_event(InputMethodEvent::Deactivate);
                if let Some(failure) = interrupted_commit_failure(&state) {
                    Err(failure)
                } else {
                    panic!("expected deactivate to interrupt commit flush")
                }
            },
        )
        .expect_err("deactivation while buffered should interrupt flush");
        let failure = state.handle_commit_flush_failure(failure);

        assert!(matches!(
            failure,
            InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodDeactivated,
            }
        ));
        assert!(matches!(
            state.failure_outcome(failure),
            InsertionOutcome::DeliveryUncertain {
                maybe_sent_bytes,
                failure: InsertionFailure::InsertionAuthorityLost {
                    reason: InsertionAuthorityLoss::InputMethodDeactivated,
                },
            } if maybe_sent_bytes == "hello".len()
        ));
        assert_eq!(
            state.session(),
            &InputMethodSession::Failed(InputMethodFailure::Deactivated)
        );
    }

    #[test]
    fn flush_loop_timeout_after_queue_is_uncertain() {
        let mut state = state_with_chunks(&["hello"]);
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        let failure = flush_buffered_commit_request(
            buffered,
            Instant::now() + Duration::from_secs(1),
            || {
                Err(WaylandError::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "socket backpressure",
                )))
            },
            |_| {
                Err(InsertionFailure::IdleTimedOut {
                    operation: InsertionIoOperation::WaitWritable,
                })
            },
        )
        .expect_err("timeout after queued request should fail");
        let failure = state.handle_commit_flush_failure(failure);

        assert!(matches!(
            failure,
            InsertionFailure::IdleTimedOut {
                operation: InsertionIoOperation::WaitWritable,
            }
        ));
        assert!(matches!(
            state.failure_outcome(failure),
            InsertionOutcome::DeliveryUncertain {
                maybe_sent_bytes,
                failure: InsertionFailure::IdleTimedOut {
                    operation: InsertionIoOperation::WaitWritable,
                },
            } if maybe_sent_bytes == "hello".len()
        ));
        assert_eq!(state.sent_bytes(), 0);
    }

    #[test]
    fn flush_loop_expired_deadline_after_queue_is_uncertain() {
        let mut state = state_with_chunks(&["hello"]);
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        let failure = flush_buffered_commit_request(
            buffered,
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            || {
                Err(WaylandError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "interrupted",
                )))
            },
            |_| Ok(()),
        )
        .expect_err("expired deadline should fail instead of retrying forever");
        let failure = state.handle_commit_flush_failure(failure);

        assert!(matches!(
            failure,
            InsertionFailure::IdleTimedOut {
                operation: InsertionIoOperation::FlushRequests,
            }
        ));
        assert!(matches!(
            state.failure_outcome(failure),
            InsertionOutcome::DeliveryUncertain {
                maybe_sent_bytes,
                failure: InsertionFailure::IdleTimedOut {
                    operation: InsertionIoOperation::FlushRequests,
                },
            } if maybe_sent_bytes == "hello".len()
        ));
        assert_eq!(state.sent_bytes(), 0);
    }
}
