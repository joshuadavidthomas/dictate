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

#[derive(Debug, thiserror::Error)]
pub(crate) enum InsertFailure {
    #[error("Wayland text insertion is unavailable: {message}")]
    NoWaylandDisplay { message: String },
    #[error("Wayland input-method manager is unavailable")]
    InputMethodManagerUnavailable,
    #[error("Wayland seat is unavailable")]
    SeatUnavailable,
    #[error("Wayland input method rejected the insertion request")]
    InputMethodRejected,
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

    state.input_method_handle = Some(manager.get_input_method(&seat, &qh, ()));
    let protocol_attempt_timeout = protocol_idle_timeout
        .saturating_mul(WaylandInputMethodBackend::PROTOCOL_ATTEMPT_IDLE_WINDOWS);
    let mut progress_deadline = ProgressDeadline::new(
        protocol_idle_timeout,
        protocol_attempt_timeout,
        state.protocol_progress,
    );
    while !state.session.is_finished() {
        if let Err(failure) = dispatch_until_event_or_timeout(
            &mut event_queue,
            &mut state,
            progress_deadline.current(),
        ) {
            return state.failure_outcome(progress_deadline.classify_timeout(failure));
        }
        progress_deadline.reset_after_progress(state.protocol_progress);
    }

    match &state.session {
        InputMethodSession::Failed(message) => {
            state.failure_outcome(InsertFailure::ProtocolFailed {
                message: message.clone(),
            })
        }
        InputMethodSession::SentToInputMethod => InsertOutcome::SentToInputMethod {
            sent_bytes: state.chunks.sent_bytes(),
        },
        InputMethodSession::Unavailable => {
            state.failure_outcome(InsertFailure::InputMethodRejected)
        }
        InputMethodSession::WaitingInactive { .. } | InputMethodSession::ReadyToCommit { .. } => {
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
        dispatch_until_event_or_timeout(event_queue, state, deadline)?;
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
    event_queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), InsertFailure> {
    loop {
        let dispatched = event_queue
            .dispatch_pending(state)
            .map_err(InsertFailure::protocol)?;
        if dispatched > 0 {
            return Ok(());
        }

        event_queue.flush().map_err(InsertFailure::protocol)?;
        let Some(read_guard) = event_queue.prepare_read() else {
            continue;
        };

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(InsertFailure::ProtocolIdleTimedOut);
        };
        if remaining.is_zero() {
            return Err(InsertFailure::ProtocolIdleTimedOut);
        }

        let mut poll_fds = [PollFd::from_borrowed_fd(
            read_guard.connection_fd(),
            PollFlags::IN,
        )];
        let timeout = Timespec::try_from(remaining).map_err(InsertFailure::protocol)?;
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => return Err(InsertFailure::ProtocolIdleTimedOut),
            Ok(_) => {
                read_guard.read().map_err(InsertFailure::protocol)?;
                return Ok(());
            }
            Err(Errno::INTR) => continue,
            Err(error) => return Err(InsertFailure::protocol(error)),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum InputMethodSession {
    WaitingInactive { serial: u32 },
    ReadyToCommit { serial: u32 },
    SentToInputMethod,
    Unavailable,
    Failed(String),
}

impl Default for InputMethodSession {
    fn default() -> Self {
        Self::WaitingInactive { serial: 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DoneAction {
    CommitChunks { serial: u32 },
    Ignore,
}

impl InputMethodSession {
    fn activate(&mut self) {
        if let Self::WaitingInactive { serial } = self {
            *self = Self::ReadyToCommit { serial: *serial };
        }
    }

    fn deactivate(&mut self) {
        match self {
            Self::ReadyToCommit { serial } => {
                *self = Self::WaitingInactive { serial: *serial };
            }
            Self::WaitingInactive { .. }
            | Self::SentToInputMethod
            | Self::Unavailable
            | Self::Failed(_) => {}
        }
    }

    fn done(&mut self) -> DoneAction {
        match self {
            Self::WaitingInactive { serial } => {
                *serial += 1;
                DoneAction::Ignore
            }
            Self::ReadyToCommit { serial } => {
                *serial += 1;
                DoneAction::CommitChunks { serial: *serial }
            }
            Self::SentToInputMethod | Self::Unavailable | Self::Failed(_) => DoneAction::Ignore,
        }
    }

    fn commit_sent(&mut self) {
        if matches!(self, Self::ReadyToCommit { .. }) {
            *self = Self::SentToInputMethod;
        }
    }

    fn unavailable(&mut self) {
        if matches!(
            self,
            Self::WaitingInactive { .. } | Self::ReadyToCommit { .. }
        ) {
            *self = Self::Unavailable;
        }
    }

    fn protocol_failed(&mut self, message: String) {
        if matches!(
            self,
            Self::WaitingInactive { .. } | Self::ReadyToCommit { .. }
        ) {
            *self = Self::Failed(message);
        }
    }

    fn is_finished(&self) -> bool {
        !matches!(
            self,
            Self::WaitingInactive { .. } | Self::ReadyToCommit { .. }
        )
    }
}

#[derive(Debug, Eq, PartialEq)]
enum InputMethodEvent {
    Activate,
    Deactivate,
    Done,
    Unavailable,
    ProtocolFailed(String),
}

#[derive(Debug, Eq, PartialEq)]
struct CommitRequest {
    serial: u32,
    chunk: String,
}

impl CommitRequest {
    fn sent_bytes(&self) -> usize {
        self.chunk.len()
    }
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

    fn mark_maybe_sent(&mut self, bytes: usize) {
        self.maybe_sent_bytes += bytes;
    }

    fn mark_sent(&mut self, bytes: usize) {
        self.sent_bytes += bytes;
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
    // Retain the proxy for the insertion attempt; dropping it unregisters the event/request target.
    input_method_handle: Option<ZwpInputMethodV2>,
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
            input_method_handle: None,
            seat: None,
            chunks: ChunkQueue::new(text_chunks),
            session: InputMethodSession::default(),
            roundtrip_done: false,
            protocol_progress: 0,
        }
    }
}

impl State {
    fn take_next_chunk(&mut self) -> Option<String> {
        self.chunks.take_next()
    }

    fn mark_chunk_maybe_sent(&mut self, bytes: usize) {
        self.chunks.mark_maybe_sent(bytes);
    }

    fn mark_chunk_sent(&mut self, bytes: usize) {
        self.chunks.mark_sent(bytes);
    }

    fn record_protocol_progress(&mut self) {
        self.protocol_progress += 1;
    }

    fn handle_input_method_event(&mut self, event: InputMethodEvent) -> Vec<CommitRequest> {
        self.record_protocol_progress();
        match event {
            InputMethodEvent::Activate => {
                self.session.activate();
                Vec::new()
            }
            InputMethodEvent::Deactivate => {
                self.session.deactivate();
                Vec::new()
            }
            InputMethodEvent::Done => {
                let DoneAction::CommitChunks { serial } = self.session.done() else {
                    return Vec::new();
                };
                let mut requests = Vec::new();
                while let Some(chunk) = self.take_next_chunk() {
                    requests.push(CommitRequest { serial, chunk });
                }
                if requests.is_empty() {
                    self.session
                        .protocol_failed("missing queued insertion chunk".to_owned());
                }
                requests
            }
            InputMethodEvent::Unavailable => {
                self.session.unavailable();
                Vec::new()
            }
            InputMethodEvent::ProtocolFailed(message) => {
                self.session.protocol_failed(message);
                Vec::new()
            }
        }
    }

    fn commit_flushed(&mut self, sent_bytes: usize) {
        self.mark_chunk_sent(sent_bytes);
    }

    fn commit_sequence_flushed(&mut self) {
        self.session.commit_sent();
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
        input_method: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        connection: &Connection,
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
                let requests = state.handle_input_method_event(InputMethodEvent::Done);
                for request in requests {
                    let sent_bytes = request.sent_bytes();
                    input_method.commit_string(request.chunk);
                    input_method.commit(request.serial);
                    // A failed flush may still have partially written this non-idempotent request.
                    // Treat fallback as unsafe once the request enters the Wayland client buffer.
                    state.mark_chunk_maybe_sent(sent_bytes);
                    if let Err(error) = connection.flush() {
                        state.handle_input_method_event(InputMethodEvent::ProtocolFailed(
                            error.to_string(),
                        ));
                        return;
                    }
                    state.commit_flushed(sent_bytes);
                }
                state.commit_sequence_flushed();
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
        let mut session = InputMethodSession::default();

        session.activate();
        let first = session.done();
        session.commit_sent();

        assert_eq!(first, DoneAction::CommitChunks { serial: 1 });
        assert_eq!(session, InputMethodSession::SentToInputMethod);
        assert!(session.is_finished());
    }

    #[test]
    fn one_done_serial_can_send_all_chunks() {
        let mut state = State::new(vec!["hello".to_owned(), " world".to_owned()]);

        state.handle_input_method_event(InputMethodEvent::Activate);
        let requests = state.handle_input_method_event(InputMethodEvent::Done);

        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| request.serial == 1));
        assert_eq!(requests[0].chunk, "hello");
        assert_eq!(requests[1].chunk, " world");
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
    fn unavailable_finishes_without_commit() {
        let mut session = InputMethodSession::default();

        session.unavailable();

        assert_eq!(session, InputMethodSession::Unavailable);
        assert!(session.is_finished());
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

        state.handle_input_method_event(InputMethodEvent::Activate);
        let requests = state.handle_input_method_event(InputMethodEvent::Done);
        assert_eq!(requests.len(), 1);
        state.mark_chunk_maybe_sent(requests[0].sent_bytes());
        state
            .handle_input_method_event(InputMethodEvent::ProtocolFailed("flush failed".to_owned()));

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

        state.handle_input_method_event(InputMethodEvent::Activate);
        let mut requests = state.handle_input_method_event(InputMethodEvent::Done);
        let request = requests.remove(0);
        assert_eq!(request.serial, 1);
        assert_eq!(request.chunk, "hello");
        state.mark_chunk_maybe_sent(request.sent_bytes());
        state.commit_flushed(request.sent_bytes());
        state.handle_input_method_event(InputMethodEvent::ProtocolFailed(
            "lost compositor".to_owned(),
        ));

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
        let mut session = InputMethodSession::default();

        session.activate();
        session.deactivate();
        let action = session.done();

        assert_eq!(action, DoneAction::Ignore);
        assert_eq!(session, InputMethodSession::WaitingInactive { serial: 1 });
        assert!(!session.is_finished());
    }

    #[test]
    fn protocol_failure_wins_before_commit_sequence_is_sent() {
        let mut session = InputMethodSession::default();

        session.activate();
        assert_eq!(session.done(), DoneAction::CommitChunks { serial: 1 });
        session.protocol_failed("flush failed".to_owned());

        assert_eq!(
            session,
            InputMethodSession::Failed("flush failed".to_owned())
        );
        assert!(session.is_finished());
    }

    #[test]
    fn unavailable_after_sent_does_not_reclassify_success() {
        let mut session = InputMethodSession::default();

        session.activate();
        assert_eq!(session.done(), DoneAction::CommitChunks { serial: 1 });
        session.commit_sent();
        session.unavailable();

        assert_eq!(session, InputMethodSession::SentToInputMethod);
    }

    #[test]
    fn unavailable_after_failed_does_not_hide_protocol_failure() {
        let mut session = InputMethodSession::default();

        session.protocol_failed("dispatch failed".to_owned());
        session.unavailable();

        assert_eq!(
            session,
            InputMethodSession::Failed("dispatch failed".to_owned())
        );
    }
}
