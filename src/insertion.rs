mod backends;
mod outcome;

pub(crate) use backends::wayland::WaylandInputMethodBackend;
pub(crate) use outcome::InsertionAuthorityLoss;
pub(crate) use outcome::InsertionBackend;
pub(crate) use outcome::InsertionBackendFailure;
pub(crate) use outcome::InsertionBackendKind;
pub(crate) use outcome::InsertionFailure;
pub(crate) use outcome::InsertionIoOperation;
pub(crate) use outcome::InsertionOutcome;
pub(crate) use outcome::InsertionProtocolFailureKind;
pub(crate) use outcome::InsertionTargetKind;
pub(crate) use outcome::InsertionText;
