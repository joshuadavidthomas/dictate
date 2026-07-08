use std::fmt;
use std::io;

pub(crate) trait InsertionBackend {
    fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InsertionText<'a> {
    text: &'a str,
}

impl<'a> InsertionText<'a> {
    pub(crate) fn new(text: &'a str) -> Option<Self> {
        if text.is_empty() {
            None
        } else {
            Some(Self { text })
        }
    }

    pub(crate) fn as_str(self) -> &'a str {
        self.text
    }
}

#[derive(Debug)]
pub(crate) enum InsertionOutcome {
    // The backend submitted a semantic insertion request. This does not prove the focused
    // application inserted the text.
    Submitted {
        sent_bytes: usize,
    },
    NotInserted(InsertionFailure),
    DeliveryUncertain {
        maybe_sent_bytes: usize,
        failure: InsertionFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InsertionFailure {
    #[error("text insertion backend is unavailable: {backend}")]
    BackendUnavailable { backend: InsertionBackendKind },
    #[error("text insertion target is unavailable: {target}")]
    TargetUnavailable { target: InsertionTargetKind },
    #[error("text insertion target is ambiguous: {target} matched {count} candidates")]
    AmbiguousTarget {
        target: InsertionTargetKind,
        count: u32,
    },
    #[error("text insertion authority was lost: {reason}")]
    InsertionAuthorityLost { reason: InsertionAuthorityLoss },
    #[error("text insertion made no progress before the idle timeout while {operation}")]
    IdleTimedOut { operation: InsertionIoOperation },
    #[error("text insertion exceeded the attempt timeout while {operation}")]
    AttemptTimedOut { operation: InsertionIoOperation },
    #[error("text insertion backend failed: {backend}: {failure}")]
    BackendFailed {
        backend: InsertionBackendKind,
        failure: InsertionBackendFailure,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertionBackendKind {
    WaylandInputMethod,
}

impl fmt::Display for InsertionBackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WaylandInputMethod => formatter.write_str("Wayland input-method"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertionTargetKind {
    Seat,
}

impl fmt::Display for InsertionTargetKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Seat => formatter.write_str("Wayland seat"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertionAuthorityLoss {
    InputMethodDeactivated,
    InputMethodUnavailable,
}

impl fmt::Display for InsertionAuthorityLoss {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputMethodDeactivated => {
                formatter.write_str("Wayland input method deactivated during the insertion request")
            }
            Self::InputMethodUnavailable => formatter
                .write_str("Wayland input method became unavailable during the insertion request"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InsertionBackendFailure {
    Io {
        operation: InsertionIoOperation,
        kind: io::ErrorKind,
        message: String,
    },
    Protocol {
        operation: InsertionIoOperation,
        kind: InsertionProtocolFailureKind,
        message: String,
    },
}

impl fmt::Display for InsertionBackendFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                kind,
                message,
            } => write!(formatter, "IO while {operation}: {message} ({kind:?})"),
            Self::Protocol {
                operation,
                kind,
                message,
            } => write!(formatter, "protocol while {operation}: {message} ({kind})"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertionProtocolFailureKind {
    BadMessage,
    WaylandProtocol,
    SessionState,
}

impl fmt::Display for InsertionProtocolFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMessage => formatter.write_str("bad message"),
            Self::WaylandProtocol => formatter.write_str("Wayland protocol"),
            Self::SessionState => formatter.write_str("input-method session state"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertionIoOperation {
    Connect,
    WaitReadable,
    WaitWritable,
    ReadEvents,
    FlushRequests,
}

impl fmt::Display for InsertionIoOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect => formatter.write_str("connecting to backend"),
            Self::WaitReadable => formatter.write_str("waiting for readability"),
            Self::WaitWritable => formatter.write_str("waiting for writability"),
            Self::ReadEvents => formatter.write_str("reading events"),
            Self::FlushRequests => formatter.write_str("flushing requests"),
        }
    }
}
