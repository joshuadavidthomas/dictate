use std::fmt;
use std::io;
use std::io::Write;

use clap::ValueEnum;
use wl_clipboard_rs::copy::ClipboardType;
use wl_clipboard_rs::copy::DataSourceError;
use wl_clipboard_rs::copy::Error as ClipboardCopyError;
use wl_clipboard_rs::copy::MimeType;
use wl_clipboard_rs::copy::Options;
use wl_clipboard_rs::copy::Source;
use wl_clipboard_rs::copy::SourceCreationError;

use crate::insertion::InsertionBackend;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionOutcome;
use crate::insertion::InsertionText;
use crate::insertion::WaylandInputMethodBackend;

const TEXT_MIME: &str = "text/plain;charset=utf-8";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum DeliveryTarget {
    #[default]
    Stdout,
    Clipboard,
    Insert,
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
#[derive(Debug)]
pub(crate) enum DeliveryReport {
    Noop,
    Delivered {
        target: ConfirmedDeliveryTarget,
        preceding_failures: Vec<DeliveryAttemptFailure>,
    },
    InsertRequestSent {
        sent_bytes: usize,
    },
    InsertUncertain {
        maybe_sent_bytes: usize,
        failure: InsertionFailure,
    },
    NotDelivered {
        failures: DeliveryFailures,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfirmedDeliveryTarget {
    Stdout,
    Clipboard,
}

#[derive(Debug)]
pub(crate) struct DeliveryFailures {
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

    pub(crate) fn iter(&self) -> impl Iterator<Item = &DeliveryAttemptFailure> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

#[derive(Debug)]
pub(crate) enum DeliveryAttemptFailure {
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
pub(crate) enum ClipboardFailure {
    #[error("failed to copy text to clipboard while {operation}: {kind}: {message}")]
    CopyFailed {
        operation: ClipboardOperation,
        kind: ClipboardFailureKind,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardOperation {
    SetClipboard,
}

impl fmt::Display for ClipboardOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetClipboard => formatter.write_str("setting clipboard contents"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardFailureKind {
    NoSeats,
    SocketOpen {
        kind: io::ErrorKind,
    },
    WaylandConnection,
    WaylandCommunication,
    MissingProtocol {
        name: String,
        version: u32,
    },
    PrimarySelectionUnsupported,
    SeatNotFound,
    TemporaryFile {
        operation: ClipboardTemporaryFileOperation,
        kind: io::ErrorKind,
    },
    DataSource {
        operation: ClipboardDataSourceOperation,
        kind: io::ErrorKind,
    },
}

impl ClipboardFailureKind {
    fn from_copy_error(error: &ClipboardCopyError) -> Self {
        match error {
            ClipboardCopyError::NoSeats => Self::NoSeats,
            ClipboardCopyError::SocketOpenError(error) => Self::SocketOpen { kind: error.kind() },
            ClipboardCopyError::WaylandConnection(_) => Self::WaylandConnection,
            ClipboardCopyError::WaylandCommunication(_) => Self::WaylandCommunication,
            ClipboardCopyError::MissingProtocol { name, version } => Self::MissingProtocol {
                name: (*name).to_owned(),
                version: *version,
            },
            ClipboardCopyError::PrimarySelectionUnsupported => Self::PrimarySelectionUnsupported,
            ClipboardCopyError::SeatNotFound => Self::SeatNotFound,
            ClipboardCopyError::TempCopy(error) => {
                let (operation, kind) = clipboard_source_creation_failure(error);
                Self::TemporaryFile { operation, kind }
            }
            ClipboardCopyError::TempFileRemove(error) => Self::TemporaryFile {
                operation: ClipboardTemporaryFileOperation::RemoveFile,
                kind: error.kind(),
            },
            ClipboardCopyError::TempDirRemove(error) => Self::TemporaryFile {
                operation: ClipboardTemporaryFileOperation::RemoveDirectory,
                kind: error.kind(),
            },
            ClipboardCopyError::Paste(error) => {
                let (operation, kind) = clipboard_data_source_failure(error);
                Self::DataSource { operation, kind }
            }
        }
    }
}

impl fmt::Display for ClipboardFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSeats => formatter.write_str("no Wayland seats"),
            Self::SocketOpen { kind } => write!(formatter, "Wayland socket open failed ({kind:?})"),
            Self::WaylandConnection => formatter.write_str("Wayland connection failed"),
            Self::WaylandCommunication => formatter.write_str("Wayland communication failed"),
            Self::MissingProtocol { name, version } => {
                write!(formatter, "missing Wayland protocol {name} v{version}")
            }
            Self::PrimarySelectionUnsupported => {
                formatter.write_str("primary selection unsupported")
            }
            Self::SeatNotFound => formatter.write_str("requested Wayland seat not found"),
            Self::TemporaryFile { operation, kind } => {
                write!(
                    formatter,
                    "temporary file failure while {operation} ({kind:?})"
                )
            }
            Self::DataSource { operation, kind } => {
                write!(
                    formatter,
                    "data source failure while {operation} ({kind:?})"
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardTemporaryFileOperation {
    CreateDirectory,
    CreateFile,
    CopyData,
    WriteFile,
    OpenFile,
    ReadMetadata,
    SeekFile,
    ReadFile,
    TruncateFile,
    RemoveFile,
    RemoveDirectory,
}

impl fmt::Display for ClipboardTemporaryFileOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDirectory => formatter.write_str("creating temporary directory"),
            Self::CreateFile => formatter.write_str("creating temporary file"),
            Self::CopyData => formatter.write_str("copying data into temporary file"),
            Self::WriteFile => formatter.write_str("writing temporary file"),
            Self::OpenFile => formatter.write_str("opening temporary file"),
            Self::ReadMetadata => formatter.write_str("reading temporary file metadata"),
            Self::SeekFile => formatter.write_str("seeking temporary file"),
            Self::ReadFile => formatter.write_str("reading temporary file"),
            Self::TruncateFile => formatter.write_str("truncating temporary file"),
            Self::RemoveFile => formatter.write_str("removing temporary file"),
            Self::RemoveDirectory => formatter.write_str("removing temporary directory"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardDataSourceOperation {
    OpenFile,
    CopyToTarget,
}

impl fmt::Display for ClipboardDataSourceOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenFile => formatter.write_str("opening data file"),
            Self::CopyToTarget => formatter.write_str("copying data to target"),
        }
    }
}

fn clipboard_source_creation_failure(
    error: &SourceCreationError,
) -> (ClipboardTemporaryFileOperation, io::ErrorKind) {
    match error {
        SourceCreationError::TempDirCreate(error) => (
            ClipboardTemporaryFileOperation::CreateDirectory,
            error.kind(),
        ),
        SourceCreationError::TempFileCreate(error) => {
            (ClipboardTemporaryFileOperation::CreateFile, error.kind())
        }
        SourceCreationError::DataCopy(error) => {
            (ClipboardTemporaryFileOperation::CopyData, error.kind())
        }
        SourceCreationError::TempFileWrite(error) => {
            (ClipboardTemporaryFileOperation::WriteFile, error.kind())
        }
        SourceCreationError::TempFileOpen(error) => {
            (ClipboardTemporaryFileOperation::OpenFile, error.kind())
        }
        SourceCreationError::TempFileMetadata(error) => {
            (ClipboardTemporaryFileOperation::ReadMetadata, error.kind())
        }
        SourceCreationError::TempFileSeek(error) => {
            (ClipboardTemporaryFileOperation::SeekFile, error.kind())
        }
        SourceCreationError::TempFileRead(error) => {
            (ClipboardTemporaryFileOperation::ReadFile, error.kind())
        }
        SourceCreationError::TempFileTruncate(error) => {
            (ClipboardTemporaryFileOperation::TruncateFile, error.kind())
        }
    }
}

fn clipboard_data_source_failure(
    error: &DataSourceError,
) -> (ClipboardDataSourceOperation, io::ErrorKind) {
    match error {
        DataSourceError::FileOpen(error) => (ClipboardDataSourceOperation::OpenFile, error.kind()),
        DataSourceError::Copy(error) => (ClipboardDataSourceOperation::CopyToTarget, error.kind()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextOutputFailure {
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
    fn copy(&mut self, text: &str) -> std::result::Result<(), ClipboardFailure>;
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
pub(crate) fn deliver(target: DeliveryTarget, text: &str) -> DeliveryReport {
    let mut insertion = WaylandInputMethodBackend::default();
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
    clipboard: &mut impl ClipboardSink,
    text: &str,
    stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    let Some(insertion_text) = InsertionText::new(text) else {
        return DeliveryReport::Noop;
    };

    match insertion.insert(insertion_text) {
        InsertionOutcome::Submitted { sent_bytes } => {
            DeliveryReport::InsertRequestSent { sent_bytes }
        }
        InsertionOutcome::DeliveryUncertain {
            maybe_sent_bytes,
            failure,
        } => DeliveryReport::InsertUncertain {
            maybe_sent_bytes,
            failure,
        },
        InsertionOutcome::NotInserted(insert_failure) => {
            let insert_failure = DeliveryAttemptFailure::Insert(insert_failure);
            match clipboard.copy(text) {
                Ok(()) => DeliveryReport::Delivered {
                    target: ConfirmedDeliveryTarget::Clipboard,
                    preceding_failures: vec![insert_failure],
                },
                Err(clipboard_failure) => {
                    let clipboard_failure = DeliveryAttemptFailure::Clipboard(clipboard_failure);
                    let mut stdout = stdout();
                    match write_stdout(&mut stdout, text) {
                        Ok(()) => DeliveryReport::Delivered {
                            target: ConfirmedDeliveryTarget::Stdout,
                            preceding_failures: vec![insert_failure, clipboard_failure],
                        },
                        Err(stdout_failure) => DeliveryReport::NotDelivered {
                            failures: DeliveryFailures::new(
                                insert_failure,
                                vec![
                                    clipboard_failure,
                                    DeliveryAttemptFailure::Stdout(stdout_failure),
                                ],
                            ),
                        },
                    }
                }
            }
        }
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

fn write_stdout(stdout: &mut impl Write, text: &str) -> std::result::Result<(), TextOutputFailure> {
    write_text(stdout, text).map_err(|error| TextOutputFailure::from_io(&error))
}

struct WaylandClipboardSink;

impl ClipboardSink for WaylandClipboardSink {
    fn copy(&mut self, text: &str) -> std::result::Result<(), ClipboardFailure> {
        copy_to_clipboard(text)
    }
}

fn write_text(mut out: impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

fn copy_to_clipboard(text: &str) -> std::result::Result<(), ClipboardFailure> {
    let mut options = Options::new();
    options.clipboard(ClipboardType::Regular);

    options
        .copy(
            Source::Bytes(text.as_bytes().to_vec().into_boxed_slice()),
            MimeType::Specific(TEXT_MIME.to_owned()),
        )
        .map_err(|error| ClipboardFailure::CopyFailed {
            operation: ClipboardOperation::SetClipboard,
            kind: ClipboardFailureKind::from_copy_error(&error),
            message: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use clap::ValueEnum as _;

    use super::*;
    use crate::insertion::InsertionAuthorityLoss;
    use crate::insertion::InsertionBackendFailure;
    use crate::insertion::InsertionBackendKind;
    use crate::insertion::InsertionIoOperation;
    use crate::insertion::InsertionProtocolFailureKind;
    use crate::insertion::InsertionTargetKind;

    #[test]
    fn write_text_appends_newline() {
        let mut out = Vec::new();

        write_text(&mut out, "hello")
            .unwrap_or_else(|error| panic!("write_text should write to Vec: {error}"));

        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn write_text_surfaces_writer_errors() {
        let error =
            write_text(FailingWriter, "hello").expect_err("failing writer should surface an error");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    fn idle_timeout() -> InsertionFailure {
        InsertionFailure::IdleTimedOut {
            operation: InsertionIoOperation::WaitReadable,
        }
    }

    fn attempt_timeout() -> InsertionFailure {
        InsertionFailure::AttemptTimedOut {
            operation: InsertionIoOperation::WaitReadable,
        }
    }

    fn assert_delivered(
        report: DeliveryReport,
        expected_target: ConfirmedDeliveryTarget,
        failures: usize,
    ) {
        let DeliveryReport::Delivered {
            target,
            preceding_failures,
        } = report
        else {
            panic!("expected delivered report");
        };
        assert_eq!(target, expected_target);
        assert_eq!(preceding_failures.len(), failures);
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn delivery_target_defaults_to_stdout() {
        assert_eq!(DeliveryTarget::default(), DeliveryTarget::Stdout);
    }

    #[test]
    fn delivery_target_clap_values_round_trip() {
        for target in [
            DeliveryTarget::Stdout,
            DeliveryTarget::Clipboard,
            DeliveryTarget::Insert,
        ] {
            let Some(value) = target.to_possible_value() else {
                panic!("delivery target should expose a clap value");
            };

            assert_eq!(
                DeliveryTarget::from_str(value.get_name(), false).ok(),
                Some(target)
            );
        }
    }

    #[test]
    fn stdout_delivery_reports_stdout() {
        let mut stdout = Vec::new();

        let report = deliver_stdout(&mut stdout, "hello");

        assert_delivered(report, ConfirmedDeliveryTarget::Stdout, 0);
        assert_eq!(stdout, b"hello\n");
    }

    #[test]
    fn stdout_delivery_reports_writer_failure() {
        let mut stdout = FailingWriter;

        let report = deliver_stdout(&mut stdout, "hello");

        let DeliveryReport::NotDelivered { failures } = report else {
            panic!("expected stdout failure report");
        };
        let failures = failures.iter().collect::<Vec<_>>();
        let [DeliveryAttemptFailure::Stdout(failure)] = failures.as_slice() else {
            panic!("expected stdout failure");
        };
        assert_eq!(failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(failure.message, "broken pipe");
    }

    #[test]
    fn target_stdout_uses_stdout_without_touching_insert_or_clipboard() {
        let mut insertion = FakeInsertion::success();
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_with_effects(
            DeliveryTarget::Stdout,
            "hello",
            &mut insertion,
            &mut clipboard,
            || &mut stdout,
        );

        assert_delivered(report, ConfirmedDeliveryTarget::Stdout, 0);
        assert_eq!(stdout, b"hello\n");
        assert!(insertion.attempts.is_empty());
        assert!(clipboard.copies.is_empty());
    }

    #[test]
    fn target_clipboard_uses_clipboard_without_touching_insert() {
        let mut insertion = FakeInsertion::success();
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_with_effects(
            DeliveryTarget::Clipboard,
            "hello",
            &mut insertion,
            &mut clipboard,
            || &mut stdout,
        );

        assert_delivered(report, ConfirmedDeliveryTarget::Clipboard, 0);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert!(insertion.attempts.is_empty());
        assert!(stdout.is_empty());
    }

    #[test]
    fn target_insert_uses_insert_then_clipboard_fallback() {
        let mut insertion = FakeInsertion::failure(idle_timeout());
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_with_effects(
            DeliveryTarget::Insert,
            "hello",
            &mut insertion,
            &mut clipboard,
            || &mut stdout,
        );

        assert_delivered(report, ConfirmedDeliveryTarget::Clipboard, 1);
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert!(stdout.is_empty());
    }

    #[test]
    fn clipboard_delivery_success_reports_clipboard() {
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_clipboard(&mut clipboard, "hello", || &mut stdout);

        assert_delivered(report, ConfirmedDeliveryTarget::Clipboard, 0);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert!(stdout.is_empty());
    }

    #[test]
    fn clipboard_delivery_failure_reports_stdout_fallback() {
        let mut clipboard = FakeClipboard::failure("copy denied");
        let mut stdout = Vec::new();

        let report = deliver_clipboard(&mut clipboard, "hello", || &mut stdout);

        let DeliveryReport::Delivered {
            target,
            preceding_failures,
        } = report
        else {
            panic!("expected fallback delivery report");
        };
        assert_eq!(target, ConfirmedDeliveryTarget::Stdout);
        let [DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { message, .. })] =
            preceding_failures.as_slice()
        else {
            panic!("expected clipboard failure");
        };
        assert_eq!(message, "copy denied");
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert_eq!(stdout, b"hello\n");
    }

    #[test]
    fn clipboard_delivery_failure_reports_stdout_write_failure() {
        let mut clipboard = FakeClipboard::failure("copy denied");

        let report = deliver_clipboard(&mut clipboard, "hello", || FailingWriter);

        let DeliveryReport::NotDelivered { failures } = report else {
            panic!("expected clipboard and stdout failure report");
        };
        let failures = failures.iter().collect::<Vec<_>>();
        let [
            DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { message, .. }),
            DeliveryAttemptFailure::Stdout(stdout_failure),
        ] = failures.as_slice()
        else {
            panic!("expected clipboard and stdout failures");
        };
        assert_eq!(message, "copy denied");
        assert_eq!(stdout_failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(stdout_failure.message, "broken pipe");
        assert_eq!(clipboard.copies, vec!["hello"]);
    }

    #[test]
    fn insert_delivery_success_reports_insert() {
        let mut insertion = FakeInsertion::success();
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

        let DeliveryReport::InsertRequestSent { sent_bytes } = report else {
            panic!("expected input-method request report");
        };
        assert_eq!(sent_bytes, "hello".len());
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    #[test]
    fn insert_delivery_empty_text_is_noop() {
        let mut insertion = FakeInsertion::success();
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "", || &mut stdout);

        assert!(matches!(report, DeliveryReport::Noop));
        assert!(insertion.attempts.is_empty());
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    #[test]
    fn insert_delivery_failure_reports_clipboard_fallback() {
        let mut insertion = FakeInsertion::failure(idle_timeout());
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

        let DeliveryReport::Delivered {
            target,
            preceding_failures,
        } = report
        else {
            panic!("expected insert fallback to clipboard");
        };
        assert_eq!(target, ConfirmedDeliveryTarget::Clipboard);
        let [DeliveryAttemptFailure::Insert(insert_failure)] = preceding_failures.as_slice() else {
            panic!("expected insert failure");
        };
        assert_eq!(insert_failure, &idle_timeout());
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert!(stdout.is_empty());
    }

    #[test]
    fn fallback_safe_insert_failures_try_clipboard_without_touching_stdout() {
        for failure in fallback_safe_insert_failures() {
            let mut insertion = FakeInsertion::not_inserted(failure.clone());
            let mut clipboard = FakeClipboard::success();
            let mut stdout = Vec::new();

            let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

            let DeliveryReport::Delivered {
                target,
                preceding_failures,
            } = report
            else {
                panic!("expected fallback-safe insert failure to use clipboard");
            };
            assert_eq!(target, ConfirmedDeliveryTarget::Clipboard);
            let [DeliveryAttemptFailure::Insert(insert_failure)] = preceding_failures.as_slice()
            else {
                panic!("expected insert failure before clipboard fallback");
            };
            assert_eq!(insert_failure, &failure);
            assert_eq!(insertion.attempts, vec!["hello"]);
            assert_eq!(clipboard.copies, vec!["hello"]);
            assert!(stdout.is_empty());
        }
    }

    #[test]
    fn insert_and_clipboard_failure_reports_stdout_fallback() {
        let mut insertion = FakeInsertion::failure(idle_timeout());
        let mut clipboard = FakeClipboard::failure("copy denied");
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || &mut stdout);

        let DeliveryReport::Delivered {
            target,
            preceding_failures,
        } = report
        else {
            panic!("expected insert and clipboard fallback to stdout");
        };
        assert_eq!(target, ConfirmedDeliveryTarget::Stdout);
        let [
            DeliveryAttemptFailure::Insert(insert_failure),
            DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { message, .. }),
        ] = preceding_failures.as_slice()
        else {
            panic!("expected insert and clipboard failures");
        };
        assert_eq!(insert_failure, &idle_timeout());
        assert_eq!(message, "copy denied");
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert_eq!(stdout, b"hello\n");
    }

    #[test]
    fn insert_clipboard_and_stdout_failure_reports_not_delivered() {
        let mut insertion = FakeInsertion::failure(idle_timeout());
        let mut clipboard = FakeClipboard::failure("copy denied");

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || FailingWriter);

        let DeliveryReport::NotDelivered { failures } = report else {
            panic!("expected insert clipboard and stdout failure report");
        };
        let failures = failures.iter().collect::<Vec<_>>();
        let [
            DeliveryAttemptFailure::Insert(insert_failure),
            DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { message, .. }),
            DeliveryAttemptFailure::Stdout(stdout_failure),
        ] = failures.as_slice()
        else {
            panic!("expected insert clipboard and stdout failures");
        };
        assert_eq!(insert_failure, &idle_timeout());
        assert_eq!(message, "copy denied");
        assert_eq!(stdout_failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(stdout_failure.message, "broken pipe");
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
    }

    #[test]
    fn uncertain_insert_failure_skips_fallback_to_avoid_duplicate_text() {
        let mut insertion = FakeInsertion::uncertain(5, idle_timeout());
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello world", || {
            &mut stdout
        });

        let DeliveryReport::InsertUncertain {
            maybe_sent_bytes,
            failure,
        } = report
        else {
            panic!("expected uncertain insert report");
        };
        assert_eq!(maybe_sent_bytes, 5);
        assert_eq!(failure, idle_timeout());
        assert_eq!(insertion.attempts, vec!["hello world"]);
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    fn fallback_safe_insert_failures() -> Vec<InsertionFailure> {
        vec![
            idle_timeout(),
            InsertionFailure::BackendUnavailable {
                backend: InsertionBackendKind::WaylandInputMethod,
            },
            InsertionFailure::TargetUnavailable {
                target: InsertionTargetKind::Seat,
            },
            InsertionFailure::AmbiguousTarget {
                target: InsertionTargetKind::Seat,
                count: 2,
            },
            idle_timeout(),
            attempt_timeout(),
            InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodDeactivated,
            },
            InsertionFailure::InsertionAuthorityLost {
                reason: InsertionAuthorityLoss::InputMethodUnavailable,
            },
            InsertionFailure::BackendFailed {
                backend: InsertionBackendKind::WaylandInputMethod,
                failure: InsertionBackendFailure::Io {
                    operation: InsertionIoOperation::FlushRequests,
                    kind: io::ErrorKind::Other,
                    message: "flush failed before bytes".to_owned(),
                },
            },
            InsertionFailure::BackendFailed {
                backend: InsertionBackendKind::WaylandInputMethod,
                failure: InsertionBackendFailure::Protocol {
                    operation: InsertionIoOperation::ReadEvents,
                    kind: InsertionProtocolFailureKind::WaylandProtocol,
                    message: "protocol failed before bytes".to_owned(),
                },
            },
        ]
    }

    struct FakeInsertion {
        outcome: FakeInsertionOutcome,
        attempts: Vec<String>,
    }

    enum FakeInsertionOutcome {
        Success,
        NotInserted(InsertionFailure),
        Uncertain {
            sent_bytes: usize,
            failure: InsertionFailure,
        },
    }

    impl FakeInsertion {
        fn success() -> Self {
            Self {
                outcome: FakeInsertionOutcome::Success,
                attempts: Vec::new(),
            }
        }

        fn failure(failure: InsertionFailure) -> Self {
            Self::not_inserted(failure)
        }

        fn not_inserted(failure: InsertionFailure) -> Self {
            Self {
                outcome: FakeInsertionOutcome::NotInserted(failure),
                attempts: Vec::new(),
            }
        }

        fn uncertain(sent_bytes: usize, failure: InsertionFailure) -> Self {
            Self {
                outcome: FakeInsertionOutcome::Uncertain {
                    sent_bytes,
                    failure,
                },
                attempts: Vec::new(),
            }
        }
    }

    impl InsertionBackend for FakeInsertion {
        fn insert(&mut self, text: InsertionText<'_>) -> InsertionOutcome {
            let text = text.as_str();
            self.attempts.push(text.to_owned());
            match &self.outcome {
                FakeInsertionOutcome::Success => InsertionOutcome::Submitted {
                    sent_bytes: text.len(),
                },
                FakeInsertionOutcome::NotInserted(failure) => {
                    InsertionOutcome::NotInserted(failure.clone())
                }
                FakeInsertionOutcome::Uncertain {
                    sent_bytes,
                    failure,
                } => InsertionOutcome::DeliveryUncertain {
                    maybe_sent_bytes: *sent_bytes,
                    failure: failure.clone(),
                },
            }
        }
    }

    struct FakeClipboard {
        failure: Option<String>,
        copies: Vec<String>,
    }

    impl FakeClipboard {
        fn success() -> Self {
            Self {
                failure: None,
                copies: Vec::new(),
            }
        }

        fn failure(message: &str) -> Self {
            Self {
                failure: Some(message.to_owned()),
                copies: Vec::new(),
            }
        }
    }

    impl ClipboardSink for FakeClipboard {
        fn copy(&mut self, text: &str) -> std::result::Result<(), ClipboardFailure> {
            self.copies.push(text.to_owned());
            match &self.failure {
                Some(message) => Err(ClipboardFailure::CopyFailed {
                    operation: ClipboardOperation::SetClipboard,
                    kind: ClipboardFailureKind::NoSeats,
                    message: message.clone(),
                }),
                None => Ok(()),
            }
        }
    }
}
