use std::fmt;
use std::io;
use std::io::Write;

use anyhow::Context;
use anyhow::Result;
use clap::ValueEnum;
use wl_clipboard_rs::copy::ClipboardType;
use wl_clipboard_rs::copy::MimeType;
use wl_clipboard_rs::copy::Options;
use wl_clipboard_rs::copy::Source;

use crate::insertion::InsertFailure;
use crate::insertion::InsertOutcome;
use crate::insertion::TextInsertionBackend;
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
    Delivered {
        target: ConfirmedDeliveryTarget,
        preceding_failures: Vec<DeliveryAttemptFailure>,
    },
    InsertRequestSent {
        sent_bytes: usize,
    },
    InsertUncertain {
        maybe_sent_bytes: usize,
        failure: InsertFailure,
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
    Insert(InsertFailure),
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

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClipboardFailure {
    #[error("failed to copy text to clipboard: {source:#}")]
    CopyFailed {
        #[source]
        source: anyhow::Error,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextOutputFailure {
    kind: io::ErrorKind,
    message: String,
}

impl TextOutputFailure {
    fn from_io(error: io::Error) -> Self {
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

trait ClipboardSink {
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

fn deliver_with_effects<W: Write>(
    target: DeliveryTarget,
    text: &str,
    insertion: &mut impl TextInsertionBackend,
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
    insertion: &mut impl TextInsertionBackend,
    clipboard: &mut impl ClipboardSink,
    text: &str,
    stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    match insertion.insert(text) {
        InsertOutcome::SentToInputMethod { sent_bytes } => {
            DeliveryReport::InsertRequestSent { sent_bytes }
        }
        InsertOutcome::DeliveryUncertain {
            maybe_sent_bytes,
            failure,
        } => DeliveryReport::InsertUncertain {
            maybe_sent_bytes,
            failure,
        },
        InsertOutcome::NotInserted(insert_failure) => {
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
    write_text(stdout, text).map_err(TextOutputFailure::from_io)
}

struct WaylandClipboardSink;

impl ClipboardSink for WaylandClipboardSink {
    fn copy(&mut self, text: &str) -> std::result::Result<(), ClipboardFailure> {
        copy_to_clipboard(text).map_err(|source| ClipboardFailure::CopyFailed { source })
    }
}

fn write_text(mut out: impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut options = Options::new();
    options.clipboard(ClipboardType::Regular);

    options
        .copy(
            Source::Bytes(text.as_bytes().to_vec().into_boxed_slice()),
            MimeType::Specific(TEXT_MIME.to_owned()),
        )
        .context("failed to set clipboard")
}

#[cfg(test)]
mod tests {
    use clap::ValueEnum as _;

    use super::*;

    #[test]
    fn write_text_appends_newline() {
        let mut out = Vec::new();

        write_text(&mut out, "hello").unwrap();

        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn write_text_surfaces_writer_errors() {
        let error = write_text(FailingWriter, "hello").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
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
            let value = target.to_possible_value().unwrap();

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
        let mut insertion = FakeInsertion::failure("no text input");
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
        let [DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { source })] =
            preceding_failures.as_slice()
        else {
            panic!("expected clipboard failure");
        };
        assert_eq!(source.to_string(), "copy denied");
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
            DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { source }),
            DeliveryAttemptFailure::Stdout(stdout_failure),
        ] = failures.as_slice()
        else {
            panic!("expected clipboard and stdout failures");
        };
        assert_eq!(source.to_string(), "copy denied");
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
    fn insert_delivery_failure_reports_clipboard_fallback() {
        let mut insertion = FakeInsertion::failure("no text input");
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
        assert_eq!(
            insert_failure.to_string(),
            "Wayland text insertion failed: no text input"
        );
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert!(stdout.is_empty());
    }

    #[test]
    fn insert_and_clipboard_failure_reports_stdout_fallback() {
        let mut insertion = FakeInsertion::failure("no text input");
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
            DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { source }),
        ] = preceding_failures.as_slice()
        else {
            panic!("expected insert and clipboard failures");
        };
        assert_eq!(
            insert_failure.to_string(),
            "Wayland text insertion failed: no text input"
        );
        assert_eq!(source.to_string(), "copy denied");
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert_eq!(stdout, b"hello\n");
    }

    #[test]
    fn insert_clipboard_and_stdout_failure_reports_not_delivered() {
        let mut insertion = FakeInsertion::failure("no text input");
        let mut clipboard = FakeClipboard::failure("copy denied");

        let report = deliver_insert(&mut insertion, &mut clipboard, "hello", || FailingWriter);

        let DeliveryReport::NotDelivered { failures } = report else {
            panic!("expected insert clipboard and stdout failure report");
        };
        let failures = failures.iter().collect::<Vec<_>>();
        let [
            DeliveryAttemptFailure::Insert(insert_failure),
            DeliveryAttemptFailure::Clipboard(ClipboardFailure::CopyFailed { source }),
            DeliveryAttemptFailure::Stdout(stdout_failure),
        ] = failures.as_slice()
        else {
            panic!("expected insert clipboard and stdout failures");
        };
        assert_eq!(
            insert_failure.to_string(),
            "Wayland text insertion failed: no text input"
        );
        assert_eq!(source.to_string(), "copy denied");
        assert_eq!(stdout_failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(stdout_failure.message, "broken pipe");
        assert_eq!(insertion.attempts, vec!["hello"]);
        assert_eq!(clipboard.copies, vec!["hello"]);
    }

    #[test]
    fn uncertain_insert_failure_skips_fallback_to_avoid_duplicate_text() {
        let mut insertion = FakeInsertion::uncertain(5, "second chunk timed out");
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
        let insert_failure = failure;
        assert_eq!(
            insert_failure.to_string(),
            "Wayland text insertion failed: second chunk timed out"
        );
        assert_eq!(insertion.attempts, vec!["hello world"]);
        assert!(clipboard.copies.is_empty());
        assert!(stdout.is_empty());
    }

    struct FakeInsertion {
        outcome: FakeInsertionOutcome,
        attempts: Vec<String>,
    }

    enum FakeInsertionOutcome {
        Success,
        Failure(String),
        Uncertain { sent_bytes: usize, message: String },
    }

    impl FakeInsertion {
        fn success() -> Self {
            Self {
                outcome: FakeInsertionOutcome::Success,
                attempts: Vec::new(),
            }
        }

        fn failure(message: &str) -> Self {
            Self {
                outcome: FakeInsertionOutcome::Failure(message.to_owned()),
                attempts: Vec::new(),
            }
        }

        fn uncertain(sent_bytes: usize, message: &str) -> Self {
            Self {
                outcome: FakeInsertionOutcome::Uncertain {
                    sent_bytes,
                    message: message.to_owned(),
                },
                attempts: Vec::new(),
            }
        }
    }

    impl TextInsertionBackend for FakeInsertion {
        fn insert(&mut self, text: &str) -> InsertOutcome {
            self.attempts.push(text.to_owned());
            match &self.outcome {
                FakeInsertionOutcome::Success => InsertOutcome::SentToInputMethod {
                    sent_bytes: text.len(),
                },
                FakeInsertionOutcome::Failure(message) => {
                    InsertOutcome::NotInserted(InsertFailure::ProtocolFailed {
                        message: message.clone(),
                    })
                }
                FakeInsertionOutcome::Uncertain {
                    sent_bytes,
                    message,
                } => InsertOutcome::DeliveryUncertain {
                    maybe_sent_bytes: *sent_bytes,
                    failure: InsertFailure::ProtocolFailed {
                        message: message.clone(),
                    },
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
                    source: anyhow::anyhow!(message.clone()),
                }),
                None => Ok(()),
            }
        }
    }
}
