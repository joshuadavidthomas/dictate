use std::fmt;
use std::io;
use std::io::Write;

use wl_clipboard_rs::copy;

use crate::insertion::ClipboardPasteBackend;
use crate::insertion::CompletedInsertion;
use crate::insertion::InsertionBackend;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionOutcome;
use crate::insertion::InsertionText;
use crate::insertion::UncertainInsertion;

const TEXT_MIME: &str = "text/plain;charset=utf-8";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeliveryTarget {
    #[default]
    Stdout,
    Clipboard,
    Insert,
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
#[derive(Debug, Eq, PartialEq)]
pub enum DeliveryReport {
    Noop,
    Delivered {
        target: ConfirmedDeliveryTarget,
        preceding_failures: Vec<DeliveryAttemptFailure>,
    },
    InsertCompleted(CompletedInsertion),
    InsertUncertain(UncertainInsertion),
    NotDelivered {
        failures: DeliveryFailures,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmedDeliveryTarget {
    Stdout,
    Clipboard,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DeliveryFailures {
    first: DeliveryAttemptFailure,
    rest: Vec<DeliveryAttemptFailure>,
}

impl DeliveryFailures {
    fn one(first: DeliveryAttemptFailure) -> Self {
        Self {
            first,
            rest: Vec::new(),
        }
    }

    fn new(first: DeliveryAttemptFailure, rest: Vec<DeliveryAttemptFailure>) -> Self {
        Self { first, rest }
    }

    pub fn iter(&self) -> impl Iterator<Item = &DeliveryAttemptFailure> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum DeliveryAttemptFailure {
    Insert(InsertionFailure),
    Clipboard(ClipboardFailure),
    Stdout(TextOutputFailure),
}

impl fmt::Display for DeliveryAttemptFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Insert(failure) => write!(formatter, "insert failed: {failure}"),
            Self::Clipboard(failure) => write!(formatter, "clipboard failed: {failure}"),
            Self::Stdout(failure) => write!(formatter, "stdout failed: {failure}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("failed to copy text to the clipboard: {kind}")]
pub struct ClipboardFailure {
    kind: ClipboardFailureKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ClipboardFailureKind {
    NoSeats,
    SocketOpen(io::ErrorKind),
    WaylandConnection,
    WaylandCommunication,
    MissingProtocol { name: String, version: u32 },
    PrimarySelectionUnsupported,
    SeatNotFound,
    TemporaryStorage(io::ErrorKind),
    DataTransfer(io::ErrorKind),
}

impl ClipboardFailureKind {
    fn from_copy_error(error: &copy::Error) -> Self {
        match error {
            copy::Error::NoSeats => Self::NoSeats,
            copy::Error::SocketOpenError(error) => Self::SocketOpen(error.kind()),
            copy::Error::WaylandConnection(_) => Self::WaylandConnection,
            copy::Error::WaylandCommunication(_) => Self::WaylandCommunication,
            copy::Error::MissingProtocol { name, version } => Self::MissingProtocol {
                name: (*name).to_owned(),
                version: *version,
            },
            copy::Error::PrimarySelectionUnsupported => Self::PrimarySelectionUnsupported,
            copy::Error::SeatNotFound => Self::SeatNotFound,
            copy::Error::TempCopy(error) => {
                Self::TemporaryStorage(source_creation_error_kind(error))
            }
            copy::Error::TempFileRemove(error) | copy::Error::TempDirRemove(error) => {
                Self::TemporaryStorage(error.kind())
            }
            copy::Error::Paste(
                copy::DataSourceError::FileOpen(error) | copy::DataSourceError::Copy(error),
            ) => Self::DataTransfer(error.kind()),
        }
    }
}

impl fmt::Display for ClipboardFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSeats => formatter.write_str("no Wayland seats"),
            Self::SocketOpen(kind) => write!(formatter, "Wayland socket open failed ({kind:?})"),
            Self::WaylandConnection => formatter.write_str("Wayland connection failed"),
            Self::WaylandCommunication => formatter.write_str("Wayland communication failed"),
            Self::MissingProtocol { name, version } => {
                write!(formatter, "missing Wayland protocol {name} v{version}")
            }
            Self::PrimarySelectionUnsupported => {
                formatter.write_str("primary selection unsupported")
            }
            Self::SeatNotFound => formatter.write_str("Wayland seat not found"),
            Self::TemporaryStorage(kind) => {
                write!(formatter, "temporary storage failed ({kind:?})")
            }
            Self::DataTransfer(kind) => write!(formatter, "data transfer failed ({kind:?})"),
        }
    }
}

fn source_creation_error_kind(error: &copy::SourceCreationError) -> io::ErrorKind {
    match error {
        copy::SourceCreationError::TempDirCreate(error)
        | copy::SourceCreationError::TempFileCreate(error)
        | copy::SourceCreationError::DataCopy(error)
        | copy::SourceCreationError::TempFileWrite(error)
        | copy::SourceCreationError::TempFileOpen(error)
        | copy::SourceCreationError::TempFileMetadata(error)
        | copy::SourceCreationError::TempFileSeek(error)
        | copy::SourceCreationError::TempFileRead(error)
        | copy::SourceCreationError::TempFileTruncate(error) => error.kind(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextOutputFailure {
    kind: io::ErrorKind,
    message: String,
}

impl TextOutputFailure {
    fn from_io(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for TextOutputFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({:?})", self.message, self.kind)
    }
}

pub(crate) trait ClipboardSink {
    fn copy(&mut self, text: &str) -> Result<(), ClipboardFailure>;
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
pub fn deliver(target: DeliveryTarget, text: &str) -> DeliveryReport {
    let mut insertion = ClipboardPasteBackend::default();
    let mut clipboard = WaylandClipboardSink;
    deliver_with_effects(target, text, &mut insertion, &mut clipboard, || {
        io::stdout().lock()
    })
}

pub(crate) fn deliver_with_effects<W: Write>(
    target: DeliveryTarget,
    text: &str,
    insertion: &mut impl InsertionBackend,
    clipboard: &mut impl ClipboardSink,
    stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    match target {
        DeliveryTarget::Stdout => {
            let mut stdout = stdout();
            deliver_stdout(&mut stdout, text)
        }
        DeliveryTarget::Clipboard => deliver_clipboard(clipboard, text, stdout),
        DeliveryTarget::Insert => deliver_insert(insertion, clipboard, text, stdout),
    }
}

fn deliver_insert<W: Write>(
    insertion: &mut impl InsertionBackend,
    _clipboard: &mut impl ClipboardSink,
    text: &str,
    _stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    let Some(insertion_text) = InsertionText::new(text) else {
        return DeliveryReport::Noop;
    };

    match insertion.insert(insertion_text) {
        InsertionOutcome::Completed(completed) => DeliveryReport::InsertCompleted(completed),
        InsertionOutcome::DeliveryUncertain(uncertain) => {
            DeliveryReport::InsertUncertain(uncertain)
        }
        InsertionOutcome::NotInserted(insert_failure) => DeliveryReport::NotDelivered {
            failures: DeliveryFailures::one(DeliveryAttemptFailure::Insert(insert_failure)),
        },
    }
}

fn deliver_clipboard<W: Write>(
    clipboard: &mut impl ClipboardSink,
    text: &str,
    stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    match clipboard.copy(text) {
        Ok(()) => DeliveryReport::Delivered {
            target: ConfirmedDeliveryTarget::Clipboard,
            preceding_failures: Vec::new(),
        },
        Err(clipboard_failure) => {
            let clipboard_failure = DeliveryAttemptFailure::Clipboard(clipboard_failure);
            let mut stdout = stdout();
            match write_stdout(&mut stdout, text) {
                Ok(()) => DeliveryReport::Delivered {
                    target: ConfirmedDeliveryTarget::Stdout,
                    preceding_failures: vec![clipboard_failure],
                },
                Err(stdout_failure) => DeliveryReport::NotDelivered {
                    failures: DeliveryFailures::new(
                        clipboard_failure,
                        vec![DeliveryAttemptFailure::Stdout(stdout_failure)],
                    ),
                },
            }
        }
    }
}

fn deliver_stdout(stdout: &mut impl Write, text: &str) -> DeliveryReport {
    match write_stdout(stdout, text) {
        Ok(()) => DeliveryReport::Delivered {
            target: ConfirmedDeliveryTarget::Stdout,
            preceding_failures: Vec::new(),
        },
        Err(stdout_failure) => DeliveryReport::NotDelivered {
            failures: DeliveryFailures::one(DeliveryAttemptFailure::Stdout(stdout_failure)),
        },
    }
}

fn write_stdout(stdout: &mut impl Write, text: &str) -> Result<(), TextOutputFailure> {
    write_text(stdout, text).map_err(|error| TextOutputFailure::from_io(&error))
}

struct WaylandClipboardSink;

impl ClipboardSink for WaylandClipboardSink {
    fn copy(&mut self, text: &str) -> Result<(), ClipboardFailure> {
        let mut options = copy::Options::new();
        options.clipboard(copy::ClipboardType::Regular);
        options
            .copy(
                copy::Source::Bytes(text.as_bytes().to_vec().into_boxed_slice()),
                copy::MimeType::Specific(TEXT_MIME.to_owned()),
            )
            .map_err(|error| ClipboardFailure {
                kind: ClipboardFailureKind::from_copy_error(&error),
            })
    }
}

fn write_text(mut out: impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insertion::ClipboardRestoration;
    use crate::insertion::DirectTypingClipboard;
    use crate::insertion::PrePasteFailure;
    use crate::insertion::WtypeFailure;

    fn completed() -> CompletedInsertion {
        CompletedInsertion::ClipboardPaste {
            transcript_bytes: 5,
            restoration: ClipboardRestoration::Restored,
        }
    }

    fn insertion_failure() -> InsertionFailure {
        InsertionFailure::DirectFallbackUnavailable {
            fallback_reason: PrePasteFailure::ClipboardChanged,
            failure: WtypeFailure::Spawn {
                kind: io::ErrorKind::NotFound,
                message: "wtype not found".to_owned(),
            },
        }
    }

    struct FakeInsertion {
        outcome: Option<InsertionOutcome>,
        attempts: Vec<String>,
    }

    impl FakeInsertion {
        fn new(outcome: InsertionOutcome) -> Self {
            Self {
                outcome: Some(outcome),
                attempts: Vec::new(),
            }
        }
    }

    impl InsertionBackend for FakeInsertion {
        fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome {
            self.attempts.push(text.as_str().to_owned());
            self.outcome
                .take()
                .expect("fixture has one insertion outcome")
        }
    }

    struct FakeClipboard {
        fails: bool,
        copies: Vec<String>,
    }

    impl ClipboardSink for FakeClipboard {
        fn copy(&mut self, text: &str) -> Result<(), ClipboardFailure> {
            self.copies.push(text.to_owned());
            if self.fails {
                Err(ClipboardFailure {
                    kind: ClipboardFailureKind::NoSeats,
                })
            } else {
                Ok(())
            }
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn insert_completion_does_not_touch_delivery_fallbacks() {
        let insertion_result = completed();
        let mut insertion = FakeInsertion::new(InsertionOutcome::Completed(insertion_result));
        let mut clipboard = FakeClipboard {
            fails: false,
            copies: Vec::new(),
        };
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

        assert_eq!(report, DeliveryReport::InsertCompleted(completed()));
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    #[test]
    fn uncertain_insert_never_uses_another_delivery_route() {
        let uncertain = UncertainInsertion::DirectTyping {
            maybe_input_bytes: 3,
            fallback_reason: PrePasteFailure::ClipboardChanged,
            failure: WtypeFailure::TimedOut,
            clipboard: DirectTypingClipboard::Published {
                restoration: ClipboardRestoration::SkippedNewerClipboard,
            },
        };
        let mut insertion = FakeInsertion::new(InsertionOutcome::DeliveryUncertain(uncertain));
        let mut clipboard = FakeClipboard {
            fails: false,
            copies: Vec::new(),
        };
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

        assert!(matches!(report, DeliveryReport::InsertUncertain(_)));
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    #[test]
    fn insertion_failure_is_terminal_without_clipboard_or_stdout_fallback() {
        let failure = insertion_failure();
        let mut insertion = FakeInsertion::new(InsertionOutcome::NotInserted(failure.clone()));
        let mut clipboard = FakeClipboard {
            fails: false,
            copies: Vec::new(),
        };
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

        assert_eq!(
            report,
            DeliveryReport::NotDelivered {
                failures: DeliveryFailures::one(DeliveryAttemptFailure::Insert(failure)),
            }
        );
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    #[test]
    fn empty_insert_is_a_noop() {
        let mut insertion = FakeInsertion::new(InsertionOutcome::Completed(completed()));
        let mut clipboard = FakeClipboard {
            fails: false,
            copies: Vec::new(),
        };
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "", || &mut stdout);

        assert_eq!(report, DeliveryReport::Noop);
        assert!(insertion.attempts.is_empty());
    }

    #[test]
    fn clipboard_failure_uses_stdout() {
        let mut clipboard = FakeClipboard {
            fails: true,
            copies: Vec::new(),
        };
        let mut stdout = Vec::new();

        let report = deliver_clipboard(&mut clipboard, "hello", || &mut stdout);

        assert!(matches!(
            report,
            DeliveryReport::Delivered {
                target: ConfirmedDeliveryTarget::Stdout,
                ..
            }
        ));
        assert_eq!(stdout, b"hello\n");
    }

    #[test]
    fn stdout_failure_is_reported() {
        let report = deliver_stdout(&mut FailingWriter, "hello");

        assert!(matches!(report, DeliveryReport::NotDelivered { .. }));
    }
}
