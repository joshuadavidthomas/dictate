use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::event::poll;
use rustix::io::Errno;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::EventQueue;
use wayland_client::QueueHandle;
use wayland_client::backend::WaylandError;
use wayland_client::delegate_noop;
use wayland_client::protocol::wl_callback;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;

const MAX_COMMIT_STRING_BYTES: usize = 4000;

pub(crate) trait TextInsertionBackend {
    fn insert(&mut self, text: &str) -> InsertOutcome;
}

#[derive(Debug)]
pub(crate) enum InsertOutcome {
    SentToInputMethod {
        sent_bytes: usize,
    },
    NotInserted(InsertFailure),
    DeliveryUncertain {
        maybe_sent_bytes: usize,
        failure: InsertFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InsertFailure {
    #[error("Wayland text insertion is unavailable: {message}")]
    NoWaylandDisplay { message: String },
    #[error("Wayland input-method manager is unavailable")]
    InputMethodManagerUnavailable,
    #[error("Wayland seat is unavailable")]
    SeatUnavailable,
    #[error("Wayland input method rejected the insertion request")]
    InputMethodRejected,
    #[error("Wayland input method deactivated during the insertion request")]
    InputMethodDeactivated,
    #[error("Wayland input method became unavailable during the insertion request")]
    InputMethodUnavailable,
    #[error("Wayland text insertion made no protocol progress before the idle timeout")]
    ProtocolIdleTimedOut,
    #[error("Wayland text insertion exceeded the protocol attempt timeout")]
    ProtocolAttemptTimedOut,
    #[error("Wayland text insertion failed: {message}")]
    ProtocolFailed { message: String },
}

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
    const PROTOCOL_ATTEMPT_IDLE_WINDOWS: u32 = 4;

    pub(crate) const fn new(protocol_idle_timeout: Duration) -> Self {
        Self {
            protocol_idle_timeout,
        }
    }
}

impl TextInsertionBackend for WaylandInputMethodBackend {
    fn insert(&mut self, text: &str) -> InsertOutcome {
        insert_with_input_method(text, self.protocol_idle_timeout)
    }
}

fn insert_with_input_method(text: &str, protocol_idle_timeout: Duration) -> InsertOutcome {
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(error) => {
            return InsertOutcome::NotInserted(InsertFailure::NoWaylandDisplay {
                message: error.to_string(),
            });
        }
    };
    let mut event_queue = connection.new_event_queue();
    let qh = event_queue.handle();

    let discovery_deadline = Instant::now() + protocol_idle_timeout;
    connection.display().get_registry(&qh, ());

    let mut state = State::new(commit_string_chunks(text));
    if let Err(failure) = bounded_roundtrip(
        &connection,
        &mut event_queue,
        &qh,
        &mut state,
        discovery_deadline,
    ) {
        return state.failure_outcome(failure);
    }

    let Some(manager) = state.input_method_manager.clone() else {
        return state.failure_outcome(InsertFailure::InputMethodManagerUnavailable);
    };
    let Some(seat) = state.seat.clone() else {
        return state.failure_outcome(InsertFailure::SeatUnavailable);
    };

    // Retain the proxy for the insertion attempt; dropping it unregisters the event/request target.
    let input_method = manager.get_input_method(&seat, &qh, ());
    let protocol_attempt_timeout = protocol_idle_timeout
        .saturating_mul(WaylandInputMethodBackend::PROTOCOL_ATTEMPT_IDLE_WINDOWS);
    let mut progress_deadline = ProgressDeadline::new(
        protocol_idle_timeout,
        protocol_attempt_timeout,
        state.protocol_progress,
    );
    while !state.session.is_finished() {
        if state.session.has_queued_commit() {
            if let Err(failure) =
                dispatch_ready_events(&mut event_queue, &mut state, progress_deadline.current())
            {
                return state.failure_outcome(progress_deadline.classify_timeout(failure));
            }
            progress_deadline.reset_after_progress(state.protocol_progress);
            if let Err(failure) = flush_next_pending_commit(
                &connection,
                &input_method,
                &mut event_queue,
                &mut state,
                &mut progress_deadline,
            ) {
                return state.failure_outcome(progress_deadline.classify_timeout(failure));
            }
            continue;
        }

        let dispatch_deadline = progress_deadline.current();
        if let Err(failure) = dispatch_until_event_or_timeout(
            &connection,
            &mut event_queue,
            &mut state,
            dispatch_deadline,
        ) {
            return state.failure_outcome(progress_deadline.classify_timeout(failure));
        }
        progress_deadline.reset_after_progress(state.protocol_progress);
    }

    match &state.session {
        InputMethodSession::Failed(failure) => state.failure_outcome(
            progress_deadline.classify_timeout(failure.clone().into_insert_failure()),
        ),
        InputMethodSession::SentToInputMethod => InsertOutcome::SentToInputMethod {
            sent_bytes: state.chunks.sent_bytes(),
        },
        InputMethodSession::Unavailable => {
            state.failure_outcome(InsertFailure::InputMethodRejected)
        }
        InputMethodSession::WaitingInactive { .. }
        | InputMethodSession::ReadyToCommit { .. }
        | InputMethodSession::CommitQueued { .. }
        | InputMethodSession::CommitInFlight { .. } => {
            state.failure_outcome(InsertFailure::ProtocolIdleTimedOut)
        }
    }
}

impl InsertFailure {
    fn protocol(error: impl fmt::Display) -> Self {
        Self::ProtocolFailed {
            message: error.to_string(),
        }
    }
}

fn bounded_roundtrip(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    qh: &QueueHandle<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertFailure> {
    state.roundtrip_done = false;
    connection.display().sync(qh, ());

    while !state.roundtrip_done && Instant::now() < deadline {
        dispatch_until_event_or_timeout(connection, event_queue, state, deadline)?;
    }

    if state.roundtrip_done {
        Ok(())
    } else {
        Err(InsertFailure::ProtocolIdleTimedOut)
    }
}

#[derive(Debug)]
struct ProgressDeadline {
    idle_timeout: Duration,
    idle_deadline: Instant,
    attempt_deadline: Instant,
    observed_progress: u64,
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

    fn current(&self) -> Instant {
        self.idle_deadline.min(self.attempt_deadline)
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

    fn classify_timeout(&self, failure: InsertFailure) -> InsertFailure {
        if matches!(failure, InsertFailure::ProtocolIdleTimedOut)
            && Instant::now() >= self.attempt_deadline
        {
            InsertFailure::ProtocolAttemptTimedOut
        } else {
            failure
        }
    }
}

fn dispatch_until_event_or_timeout(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertFailure> {
    loop {
        ensure_deadline_active(deadline)?;
        let dispatched = event_queue
            .dispatch_pending(state)
            .map_err(InsertFailure::protocol)?;
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
            WaylandIoOperation::WaitReadable,
        )?;
        if read_events_or_retry(read_guard.read())? {
            return Ok(());
        }
    }
}

fn dispatch_ready_events(
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertFailure> {
    loop {
        ensure_deadline_active(deadline)?;
        let dispatched = event_queue
            .dispatch_pending(state)
            .map_err(InsertFailure::protocol)?;
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
            WaylandIoOperation::WaitReadable,
        )?;
        if !ready.readable {
            drop(read_guard);
            return Ok(());
        }
        if !read_events_or_retry(read_guard.read())? {
            continue;
        }
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
) -> Result<WaylandReadAttempt, InsertFailure> {
    match result {
        Ok(_) => Ok(WaylandReadAttempt::Completed),
        Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
            Ok(WaylandReadAttempt::Interrupted)
        }
        Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
            Ok(WaylandReadAttempt::WouldBlock)
        }
        Err(error) => Err(InsertFailure::protocol(error)),
    }
}

fn read_events_or_retry(result: Result<usize, WaylandError>) -> Result<bool, InsertFailure> {
    match classify_wayland_read(result)? {
        WaylandReadAttempt::Completed => Ok(true),
        WaylandReadAttempt::Interrupted | WaylandReadAttempt::WouldBlock => Ok(false),
    }
}

fn flush_wait_read_action(
    result: Result<usize, WaylandError>,
    context: FlushWaitContext,
) -> Result<FlushWaitReadAction, InsertFailure> {
    match classify_wayland_read(result)? {
        WaylandReadAttempt::Completed => Ok(FlushWaitReadAction::DispatchPending),
        WaylandReadAttempt::Interrupted => Ok(FlushWaitReadAction::ContinueWaiting),
        WaylandReadAttempt::WouldBlock if context.writable_observed => {
            Ok(FlushWaitReadAction::RetryFlush)
        }
        WaylandReadAttempt::WouldBlock => Ok(FlushWaitReadAction::ContinueWaiting),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventQueueFlushProgress {
    Flushed,
    Writable,
    EventDispatched,
}

fn flush_event_queue(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<EventQueueFlushProgress, InsertFailure> {
    loop {
        ensure_deadline_active(deadline)?;
        match event_queue.flush() {
            Ok(()) => return Ok(EventQueueFlushProgress::Flushed),
            Err(error) => match flush_retry(&error) {
                FlushRetry::RetryNow => {
                    ensure_deadline_active(deadline)?;
                    continue;
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
                FlushRetry::Fail => return Err(InsertFailure::protocol(error)),
            },
        }
    }
}

fn wait_for_event_queue_flush_progress(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<EventQueueFlushProgress, InsertFailure> {
    loop {
        ensure_deadline_active(deadline)?;
        let Some(read_guard) = event_queue.prepare_read() else {
            let dispatched = event_queue
                .dispatch_pending(state)
                .map_err(InsertFailure::protocol)?;
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
            WaylandIoOperation::WaitWritable,
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
                        .map_err(InsertFailure::protocol)?;
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
                FlushWaitReadAction::ContinueWaiting => continue,
            }
        } else {
            drop(read_guard);
            if ready.writable {
                return Ok(EventQueueFlushProgress::Writable);
            }
        }
    }
}

fn flush_next_pending_commit(
    connection: &Connection,
    input_method: &ZwpInputMethodV2,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    progress_deadline: &mut ProgressDeadline,
) -> Result<(), InsertFailure> {
    let Some((_request, buffered)) = state.buffer_next_commit_with(|request| {
        input_method.commit_string(request.chunk.clone());
        input_method.commit(request.serial);
    })?
    else {
        return Ok(());
    };
    flush_commit_request(
        connection,
        event_queue,
        state,
        buffered,
        progress_deadline.current(),
    )?;
    progress_deadline.reset_after_write_progress();
    Ok(())
}

fn flush_commit_request(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    buffered: BufferedCommit,
    deadline: Instant,
) -> Result<(), InsertFailure> {
    flush_buffered_commit_request(
        state,
        buffered,
        deadline,
        || connection.flush(),
        |state, deadline| wait_for_commit_flush_progress(connection, event_queue, state, deadline),
    )
}

fn wait_for_commit_flush_progress(
    connection: &Connection,
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertFailure> {
    loop {
        ensure_deadline_active(deadline)?;
        let Some(read_guard) = event_queue.prepare_read() else {
            event_queue
                .dispatch_pending(state)
                .map_err(InsertFailure::protocol)?;
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
            WaylandIoOperation::WaitWritable,
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
                            .map_err(InsertFailure::protocol)?;
                        if let Some(failure) = interrupted_commit_failure(state) {
                            return Err(failure);
                        }
                    }
                    FlushWaitReadAction::RetryFlush => {
                        return Ok(());
                    }
                    FlushWaitReadAction::ContinueWaiting => continue,
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

fn interrupted_commit_failure(state: &State) -> Option<InsertFailure> {
    match &state.session {
        InputMethodSession::CommitInFlight { .. } => None,
        InputMethodSession::Failed(failure) => Some(failure.clone().into_insert_failure()),
        _ => Some(InsertFailure::ProtocolFailed {
            message: "input-method commit was interrupted before flush completed".to_owned(),
        }),
    }
}

fn flush_buffered_commit_request(
    state: &mut State,
    buffered: BufferedCommit,
    deadline: Instant,
    mut flush: impl FnMut() -> Result<(), WaylandError>,
    mut wait_writable: impl FnMut(&mut State, Instant) -> Result<(), InsertFailure>,
) -> Result<(), InsertFailure> {
    loop {
        ensure_deadline_active(deadline)?;
        match flush() {
            Ok(()) => {
                state.commit_request_flushed(buffered)?;
                return Ok(());
            }
            Err(error) => match flush_retry(&error) {
                FlushRetry::RetryNow => {
                    ensure_deadline_active(deadline)?;
                    continue;
                }
                FlushRetry::WaitWritable => {
                    wait_writable(state, deadline)?;
                }
                FlushRetry::Fail => {
                    let failure = InputMethodFailure::FlushFailed {
                        message: error.to_string(),
                    };
                    state.session = InputMethodSession::Failed(failure.clone());
                    return Err(failure.into_insert_failure());
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

fn ensure_deadline_active(deadline: Instant) -> Result<(), InsertFailure> {
    if Instant::now() >= deadline {
        Err(InsertFailure::ProtocolIdleTimedOut)
    } else {
        Ok(())
    }
}

fn flush_retry(error: &WaylandError) -> FlushRetry {
    let WaylandError::Io(error) = error else {
        return FlushRetry::Fail;
    };

    match error.kind() {
        std::io::ErrorKind::Interrupted => FlushRetry::RetryNow,
        std::io::ErrorKind::WouldBlock => FlushRetry::WaitWritable,
        _ => FlushRetry::Fail,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaylandIoOperation {
    WaitReadable,
    WaitWritable,
}

impl WaylandIoOperation {
    fn label(self) -> &'static str {
        match self {
            Self::WaitReadable => "waiting for Wayland readability",
            Self::WaitWritable => "waiting for Wayland writability",
        }
    }
}

fn wayland_io_failed(operation: WaylandIoOperation, error: impl fmt::Display) -> InsertFailure {
    InsertFailure::ProtocolFailed {
        message: format!("{} failed: {error}", operation.label()),
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
    operation: WaylandIoOperation,
) -> Result<FdReady, InsertFailure> {
    loop {
        let timeout = match deadline {
            PollDeadline::Now => Timespec::default(),
            PollDeadline::Until(deadline) => {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return Err(InsertFailure::ProtocolIdleTimedOut);
                };
                if remaining.is_zero() {
                    return Err(InsertFailure::ProtocolIdleTimedOut);
                }
                Timespec::try_from(remaining)
                    .map_err(|error| wayland_io_failed(operation, error))?
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
                    PollDeadline::Until(_) => Err(InsertFailure::ProtocolIdleTimedOut),
                };
            }
            Ok(_) => {
                let ready = poll_fds[0].revents();
                if ready.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
                    return Err(InsertFailure::ProtocolFailed {
                        message: format!(
                            "{} failed: Wayland fd reported {ready:?}",
                            operation.label()
                        ),
                    });
                }
                return Ok(FdReady {
                    readable: ready.intersects(PollFlags::IN),
                    writable: ready.intersects(PollFlags::OUT),
                });
            }
            Err(Errno::INTR) => continue,
            Err(error) => {
                return Err(wayland_io_failed(operation, error));
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommitChunk {
    chunk: String,
}

#[derive(Debug, Eq, PartialEq)]
struct CommitBatch {
    current: CommitChunk,
    remaining: VecDeque<CommitChunk>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommitRequest {
    serial: u32,
    chunk: String,
}

impl CommitRequest {
    fn sent_bytes(&self) -> usize {
        self.chunk.len()
    }
}

#[derive(Debug, Eq, PartialEq)]
struct BufferedCommit {
    sent_bytes: usize,
}

#[derive(Debug, Eq, PartialEq)]
enum InputMethodSession {
    WaitingInactive {
        serial: u32,
    },
    ReadyToCommit {
        serial: u32,
    },
    CommitQueued {
        serial: u32,
        current: CommitChunk,
        remaining: VecDeque<CommitChunk>,
    },
    CommitInFlight {
        next_serial: u32,
        remaining: VecDeque<CommitChunk>,
        buffered_sent_bytes: usize,
    },
    SentToInputMethod,
    Unavailable,
    Failed(InputMethodFailure),
}

impl Default for InputMethodSession {
    fn default() -> Self {
        Self::WaitingInactive { serial: 0 }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum InputMethodFailure {
    Deactivated,
    Unavailable,
    SerialAdvancedAfterBuffer,
    MissingCommitChunk,
    InvalidCommitTransition { message: String },
    FlushFailed { message: String },
}

impl InputMethodFailure {
    fn into_insert_failure(self) -> InsertFailure {
        match self {
            Self::Deactivated => InsertFailure::InputMethodDeactivated,
            Self::Unavailable => InsertFailure::InputMethodUnavailable,
            Self::SerialAdvancedAfterBuffer => InsertFailure::ProtocolFailed {
                message: "input-method serial advanced before buffered commit flushed".to_owned(),
            },
            Self::MissingCommitChunk => InsertFailure::ProtocolFailed {
                message: "missing queued insertion chunk".to_owned(),
            },
            Self::InvalidCommitTransition { message } | Self::FlushFailed { message } => {
                InsertFailure::ProtocolFailed { message }
            }
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
            serial: *serial,
            chunk: current.chunk.clone(),
        })
    }

    fn validate_commit_request(&self, request: &CommitRequest) -> Result<(), InputMethodFailure> {
        let Self::CommitQueued {
            serial, current, ..
        } = self
        else {
            return Err(InputMethodFailure::InvalidCommitTransition {
                message: "commit requested outside queued input-method session".to_owned(),
            });
        };
        if request.serial != *serial || request.chunk != current.chunk {
            return Err(InputMethodFailure::InvalidCommitTransition {
                message: "commit request did not match the queued input-method commit".to_owned(),
            });
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
            return Err(InputMethodFailure::InvalidCommitTransition {
                message: "validated queued commit was no longer queued".to_owned(),
            });
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
            return Err(InputMethodFailure::InvalidCommitTransition {
                message: "commit flush completed outside in-flight input-method session".to_owned(),
            });
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

#[derive(Debug, Eq, PartialEq)]
enum InputMethodEvent {
    Activate,
    Deactivate,
    Done,
    Unavailable,
}

#[derive(Debug)]
struct ChunkQueue {
    chunks: VecDeque<String>,
    maybe_sent_bytes: usize,
    sent_bytes: usize,
}

impl ChunkQueue {
    fn new(chunks: Vec<String>) -> Self {
        Self {
            chunks: chunks.into(),
            maybe_sent_bytes: 0,
            sent_bytes: 0,
        }
    }

    fn take_next(&mut self) -> Option<String> {
        self.chunks.pop_front()
    }

    fn take_commit_batch(&mut self) -> Option<CommitBatch> {
        let current = self.take_next().map(|chunk| CommitChunk { chunk })?;
        let mut remaining = VecDeque::new();
        while let Some(chunk) = self.take_next() {
            remaining.push_back(CommitChunk { chunk });
        }
        Some(CommitBatch { current, remaining })
    }

    fn record_commit_buffered(&mut self, bytes: usize) {
        self.maybe_sent_bytes += bytes;
    }

    fn record_commit_flushed(&mut self, bytes: usize) {
        let flushed_bytes = self.sent_bytes + bytes;
        assert!(
            flushed_bytes <= self.maybe_sent_bytes,
            "flushed commit bytes cannot exceed buffered commit bytes"
        );
        self.sent_bytes = flushed_bytes;
    }

    fn maybe_sent_bytes(&self) -> usize {
        self.maybe_sent_bytes
    }

    fn sent_bytes(&self) -> usize {
        self.sent_bytes
    }
}

struct State {
    input_method_manager: Option<ZwpInputMethodManagerV2>,
    seat: Option<wl_seat::WlSeat>,
    chunks: ChunkQueue,
    session: InputMethodSession,
    roundtrip_done: bool,
    protocol_progress: u64,
}

impl State {
    fn new(text_chunks: Vec<String>) -> Self {
        Self {
            input_method_manager: None,
            seat: None,
            chunks: ChunkQueue::new(text_chunks),
            session: InputMethodSession::default(),
            roundtrip_done: false,
            protocol_progress: 0,
        }
    }
}

impl State {
    fn take_commit_batch(&mut self) -> Option<CommitBatch> {
        self.chunks.take_commit_batch()
    }

    fn buffer_next_commit_with(
        &mut self,
        write_commit: impl FnOnce(&CommitRequest),
    ) -> Result<Option<(CommitRequest, BufferedCommit)>, InsertFailure> {
        let Some(request) = self.session.next_commit_request() else {
            return Ok(None);
        };
        self.session
            .validate_commit_request(&request)
            .map_err(|failure| {
                self.session = InputMethodSession::Failed(failure.clone());
                failure.into_insert_failure()
            })?;
        write_commit(&request);
        let buffered = self.commit_request_buffered(&request)?;
        Ok(Some((request, buffered)))
    }

    fn commit_request_buffered(
        &mut self,
        request: &CommitRequest,
    ) -> Result<BufferedCommit, InsertFailure> {
        let bytes = self
            .session
            .commit_request_buffered(request)
            .map_err(|failure| {
                self.session = InputMethodSession::Failed(failure.clone());
                failure.into_insert_failure()
            })?;
        self.chunks.record_commit_buffered(bytes);
        Ok(BufferedCommit { sent_bytes: bytes })
    }

    fn commit_request_flushed(&mut self, buffered: BufferedCommit) -> Result<(), InsertFailure> {
        let bytes = self.session.commit_request_flushed().map_err(|failure| {
            self.session = InputMethodSession::Failed(failure.clone());
            failure.into_insert_failure()
        })?;
        if bytes != buffered.sent_bytes {
            let failure = InputMethodFailure::InvalidCommitTransition {
                message: "flushed commit bytes did not match buffered commit".to_owned(),
            };
            self.session = InputMethodSession::Failed(failure.clone());
            return Err(failure.into_insert_failure());
        }
        self.chunks.record_commit_flushed(bytes);
        Ok(())
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
            let serial = *serial + 1;
            let Some(batch) = self.take_commit_batch() else {
                self.session = InputMethodSession::Failed(InputMethodFailure::MissingCommitChunk);
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
                *serial += 1;
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

    fn handle_input_method_event(&mut self, event: InputMethodEvent) {
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

    fn failure_outcome(&self, failure: InsertFailure) -> InsertOutcome {
        if self.chunks.maybe_sent_bytes() == 0 {
            InsertOutcome::NotInserted(failure)
        } else {
            InsertOutcome::DeliveryUncertain {
                maybe_sent_bytes: self.chunks.maybe_sent_bytes(),
                failure,
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version: _,
        } = event
        else {
            return;
        };

        match interface.as_str() {
            "zwp_input_method_manager_v2" => {
                state.input_method_manager =
                    Some(registry.bind::<ZwpInputMethodManagerV2, _, _>(name, 1, qh, ()));
            }
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputMethodManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.roundtrip_done = true;
        }
    }
}

impl Dispatch<ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        _input_method: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
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
            | zwp_input_method_v2::Event::ContentType { .. } => {}
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);

fn commit_string_chunks(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = 0;

    for (index, character) in text.char_indices() {
        let character_bytes = character.len_utf8();
        if bytes + character_bytes > MAX_COMMIT_STRING_BYTES {
            chunks.push(text[start..index].to_owned());
            start = index;
            bytes = 0;
        }
        bytes += character_bytes;
    }

    if start < text.len() {
        chunks.push(text[start..].to_owned());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begin_and_buffer_commit(state: &mut State) -> (CommitRequest, BufferedCommit) {
        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        state
            .buffer_next_commit_with(|_| {})
            .expect("commit request should be buffered")
            .expect("done should queue a commit request")
    }

    #[test]
    fn commit_chunks_stay_under_wayland_limit() {
        let text = "x".repeat(MAX_COMMIT_STRING_BYTES + 1);
        let chunks = commit_string_chunks(&text);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks.concat(), text);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= MAX_COMMIT_STRING_BYTES)
        );
    }

    #[test]
    fn commit_chunks_preserve_utf8_boundaries() {
        let text = format!("{}é", "x".repeat(MAX_COMMIT_STRING_BYTES - 1));
        let chunks = commit_string_chunks(&text);

        assert_eq!(chunks.concat(), text);
        assert_eq!(chunks[0].len(), MAX_COMMIT_STRING_BYTES - 1);
        assert_eq!(chunks[1], "é");
    }

    #[test]
    fn final_chunk_request_is_sent_after_flush() {
        let mut state = State::new(vec!["hello".to_owned()]);

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
        let mut state = State::new(vec!["hello".to_owned(), " world".to_owned()]);

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
        assert_eq!(*serial, 1);
        assert_eq!(current.chunk, "hello");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].chunk, " world");
    }

    #[test]
    fn queued_commit_flush_advances_to_remaining_then_sent() {
        let mut state = State::new(vec!["hello".to_owned(), " world".to_owned()]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("first commit request should flush");
        let InputMethodSession::CommitQueued {
            current, remaining, ..
        } = &state.session
        else {
            panic!("first flush should advance to the second queued commit");
        };
        assert_eq!(current.chunk, " world");
        assert!(remaining.is_empty());

        let (request, buffered) = state
            .buffer_next_commit_with(|_| {})
            .expect("remaining commit request should be buffered")
            .expect("remaining queued commit should become in-flight");
        assert_eq!(request.chunk, " world");
        state
            .commit_request_flushed(buffered)
            .expect("remaining commit request should flush");
        assert_eq!(state.session, InputMethodSession::SentToInputMethod);
    }

    #[test]
    fn done_while_queued_refreshes_remaining_commit_serials() {
        let mut state = State::new(vec!["hello".to_owned(), " world".to_owned()]);

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
        assert_eq!(*serial, 2);
        assert_eq!(current.chunk, "hello");
        assert_eq!(remaining[0].chunk, " world");
    }

    #[test]
    fn done_while_in_flight_marks_buffered_commit_stale() {
        let mut state = State::new(vec!["hello".to_owned(), " world".to_owned()]);
        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        let (in_flight, _buffered) = state
            .buffer_next_commit_with(|_| {})
            .expect("commit request should be buffered")
            .expect("queued commit should become in-flight");
        assert_eq!(in_flight.serial, 1);

        state.handle_input_method_event(InputMethodEvent::Done);

        assert_eq!(
            state.session,
            InputMethodSession::Failed(InputMethodFailure::SerialAdvancedAfterBuffer)
        );
    }

    #[test]
    fn queued_commits_are_rejected_when_deactivated_before_flush() {
        let mut state = State::new(vec!["hello".to_owned()]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Done);
        state.handle_input_method_event(InputMethodEvent::Deactivate);

        assert_eq!(
            state.session,
            InputMethodSession::Failed(InputMethodFailure::Deactivated)
        );
        assert!(matches!(
            state.failure_outcome(InputMethodFailure::Deactivated.into_insert_failure()),
            InsertOutcome::NotInserted(_)
        ));
    }

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
        assert!(deadline.current() >= initial);
        assert!(deadline.current() <= now + attempt_timeout);
    }

    #[test]
    fn progress_deadline_reports_overall_attempt_timeout() {
        let timeout = Duration::from_millis(1);
        let now = Instant::now() - Duration::from_secs(1);
        let deadline = ProgressDeadline::new_at(timeout, timeout, 0, now);

        assert!(matches!(
            deadline.classify_timeout(InsertFailure::ProtocolIdleTimedOut),
            InsertFailure::ProtocolAttemptTimedOut
        ));
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
        let mut state = State::new(vec!["hello".to_owned()]);
        let mut flushes = VecDeque::from([
            Err(WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "socket backpressure",
            ))),
            Ok(()),
        ]);
        let mut wait_calls = 0;
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        flush_buffered_commit_request(
            &mut state,
            buffered,
            Instant::now() + Duration::from_secs(1),
            || flushes.pop_front().expect("flush action should exist"),
            |_, _| {
                wait_calls += 1;
                Ok(())
            },
        )
        .expect("flush should succeed after writable wait");

        assert_eq!(wait_calls, 1);
        assert_eq!(state.chunks.maybe_sent_bytes(), "hello".len());
        assert_eq!(state.chunks.sent_bytes(), "hello".len());
    }

    #[test]
    fn flush_loop_retries_interrupted_without_waiting() {
        let mut state = State::new(vec!["hello".to_owned()]);
        let mut flushes = VecDeque::from([
            Err(WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "interrupted",
            ))),
            Ok(()),
        ]);
        let mut wait_calls = 0;
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        flush_buffered_commit_request(
            &mut state,
            buffered,
            Instant::now() + Duration::from_secs(1),
            || flushes.pop_front().expect("flush action should exist"),
            |_, _| {
                wait_calls += 1;
                Ok(())
            },
        )
        .expect("interrupted flush should retry immediately");

        assert_eq!(wait_calls, 0);
        assert_eq!(state.chunks.maybe_sent_bytes(), "hello".len());
        assert_eq!(state.chunks.sent_bytes(), "hello".len());
    }

    #[test]
    fn flush_loop_deactivate_after_buffer_marks_delivery_uncertain() {
        let mut state = State::new(vec!["hello".to_owned()]);
        let (_request, buffered) = begin_and_buffer_commit(&mut state);
        let mut flushes = VecDeque::from([
            Err(WaylandError::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "socket backpressure",
            ))),
            Ok(()),
        ]);

        let failure = flush_buffered_commit_request(
            &mut state,
            buffered,
            Instant::now() + Duration::from_secs(1),
            || flushes.pop_front().expect("flush action should exist"),
            |state, _| {
                state.handle_input_method_event(InputMethodEvent::Deactivate);
                if let Some(failure) = interrupted_commit_failure(state) {
                    Err(failure)
                } else {
                    Err(InsertFailure::ProtocolFailed {
                        message: "expected deactivate to interrupt commit flush".to_owned(),
                    })
                }
            },
        )
        .expect_err("deactivation while buffered should interrupt flush");

        assert!(matches!(failure, InsertFailure::InputMethodDeactivated));
        assert!(matches!(
            state.failure_outcome(failure),
            InsertOutcome::DeliveryUncertain {
                maybe_sent_bytes,
                failure: InsertFailure::InputMethodDeactivated,
            } if maybe_sent_bytes == "hello".len()
        ));
        assert_eq!(
            state.session,
            InputMethodSession::Failed(InputMethodFailure::Deactivated)
        );
    }

    #[test]
    fn flush_loop_timeout_after_queue_is_uncertain() {
        let mut state = State::new(vec!["hello".to_owned()]);
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        let failure = flush_buffered_commit_request(
            &mut state,
            buffered,
            Instant::now() + Duration::from_secs(1),
            || {
                Err(WaylandError::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "socket backpressure",
                )))
            },
            |_, _| Err(InsertFailure::ProtocolIdleTimedOut),
        )
        .expect_err("timeout after queued request should fail");

        assert!(matches!(failure, InsertFailure::ProtocolIdleTimedOut));
        assert!(matches!(
            state.failure_outcome(failure),
            InsertOutcome::DeliveryUncertain {
                maybe_sent_bytes,
                failure: InsertFailure::ProtocolIdleTimedOut,
            } if maybe_sent_bytes == "hello".len()
        ));
        assert_eq!(state.chunks.sent_bytes(), 0);
    }

    #[test]
    fn flush_loop_expired_deadline_after_queue_is_uncertain() {
        let mut state = State::new(vec!["hello".to_owned()]);
        let (_request, buffered) = begin_and_buffer_commit(&mut state);

        let failure = flush_buffered_commit_request(
            &mut state,
            buffered,
            Instant::now() - Duration::from_secs(1),
            || {
                Err(WaylandError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "interrupted",
                )))
            },
            |_, _| Ok(()),
        )
        .expect_err("expired deadline should fail instead of retrying forever");

        assert!(matches!(failure, InsertFailure::ProtocolIdleTimedOut));
        assert!(matches!(
            state.failure_outcome(failure),
            InsertOutcome::DeliveryUncertain {
                maybe_sent_bytes,
                failure: InsertFailure::ProtocolIdleTimedOut,
            } if maybe_sent_bytes == "hello".len()
        ));
        assert_eq!(state.chunks.sent_bytes(), 0);
    }

    #[test]
    fn unavailable_finishes_without_commit() {
        let mut state = State::new(vec!["hello".to_owned()]);

        state.handle_input_method_event(InputMethodEvent::Unavailable);

        assert_eq!(state.session, InputMethodSession::Unavailable);
        assert!(state.session.is_finished());
    }

    #[test]
    fn event_driver_reports_unavailable_before_bytes_as_not_inserted() {
        let mut state = State::new(vec!["hello".to_owned()]);

        state.handle_input_method_event(InputMethodEvent::Unavailable);

        assert!(matches!(
            state.failure_outcome(InsertFailure::InputMethodRejected),
            InsertOutcome::NotInserted(InsertFailure::InputMethodRejected)
        ));
    }

    #[test]
    fn event_driver_reports_first_flush_failure_as_uncertain_after_request_is_queued() {
        let mut state = State::new(vec!["hello".to_owned()]);

        let (request, _buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");

        let InsertOutcome::DeliveryUncertain {
            maybe_sent_bytes,
            failure,
        } = state.failure_outcome(InsertFailure::ProtocolFailed {
            message: "flush failed".to_owned(),
        })
        else {
            panic!("expected uncertain insertion after queueing commit request");
        };
        assert_eq!(maybe_sent_bytes, "hello".len());
        assert_eq!(
            failure.to_string(),
            "Wayland text insertion failed: flush failed"
        );
    }

    #[test]
    fn event_driver_reports_failure_after_flushed_chunk_as_uncertain() {
        let mut state = State::new(vec!["hello".to_owned(), " world".to_owned()]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.serial, 1);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");

        let InsertOutcome::DeliveryUncertain {
            maybe_sent_bytes,
            failure,
        } = state.failure_outcome(InsertFailure::ProtocolFailed {
            message: "lost compositor".to_owned(),
        })
        else {
            panic!("expected uncertain insertion outcome");
        };
        assert_eq!(maybe_sent_bytes, "hello".len());
        assert_eq!(
            failure.to_string(),
            "Wayland text insertion failed: lost compositor"
        );
    }

    #[test]
    fn event_driver_reports_timeout_before_bytes_as_not_inserted() {
        let state = State::new(vec!["hello".to_owned()]);

        assert!(matches!(
            state.failure_outcome(InsertFailure::ProtocolIdleTimedOut),
            InsertOutcome::NotInserted(InsertFailure::ProtocolIdleTimedOut)
        ));
    }

    #[test]
    fn missing_registry_parts_are_fallback_safe_before_bytes() {
        let state = State::new(vec!["hello".to_owned()]);

        assert!(matches!(
            state.failure_outcome(InsertFailure::InputMethodManagerUnavailable),
            InsertOutcome::NotInserted(InsertFailure::InputMethodManagerUnavailable)
        ));
        assert!(matches!(
            state.failure_outcome(InsertFailure::SeatUnavailable),
            InsertOutcome::NotInserted(InsertFailure::SeatUnavailable)
        ));
    }

    #[test]
    fn deactivate_before_done_does_not_commit() {
        let mut state = State::new(vec!["hello".to_owned()]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        state.handle_input_method_event(InputMethodEvent::Deactivate);
        state.handle_input_method_event(InputMethodEvent::Done);

        assert_eq!(
            state.session,
            InputMethodSession::WaitingInactive { serial: 1 }
        );
        assert!(!state.session.is_finished());
    }

    #[test]
    fn unavailable_after_sent_does_not_reclassify_success() {
        let mut state = State::new(vec!["hello".to_owned()]);

        let (request, buffered) = begin_and_buffer_commit(&mut state);
        assert_eq!(request.chunk, "hello");
        state
            .commit_request_flushed(buffered)
            .expect("commit request should flush");
        state.handle_input_method_event(InputMethodEvent::Unavailable);

        assert_eq!(state.session, InputMethodSession::SentToInputMethod);
    }
}
