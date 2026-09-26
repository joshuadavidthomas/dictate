use std::fmt;
use std::io;
use std::io::Write;

use crate::insertion::CompletedInsertion;
use crate::insertion::InsertionBackend;
use crate::insertion::InsertionFailure;
use crate::insertion::InsertionOutcome;
use crate::insertion::InsertionText;
use crate::insertion::UncertainInsertion;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
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

impl ClipboardFailure {
    #[must_use]
    pub fn new(kind: ClipboardFailureKind) -> Self {
        Self { kind }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardFailureKind {
    Unavailable,
    Connection,
    Communication,
    MissingCapability {
        name: String,
        version: u32,
    },
    Unsupported,
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
    TemporaryStorage(io::ErrorKind),
    DataTransfer(io::ErrorKind),
    Platform {
        operation: &'static str,
    },
}

impl fmt::Display for ClipboardFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("clipboard is unavailable"),
            Self::Connection => formatter.write_str("clipboard connection failed"),
            Self::Communication => formatter.write_str("clipboard communication failed"),
            Self::MissingCapability { name, version } => {
                write!(formatter, "missing clipboard capability {name} v{version}")
            }
            Self::Unsupported => formatter.write_str("clipboard operation is unsupported"),
            Self::Io { operation, kind } => {
                write!(formatter, "{operation} failed ({kind:?})")
            }
            Self::TemporaryStorage(kind) => {
                write!(formatter, "temporary storage failed ({kind:?})")
            }
            Self::DataTransfer(kind) => write!(formatter, "data transfer failed ({kind:?})"),
            Self::Platform { operation } => formatter.write_str(operation),
        }
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

pub trait ClipboardSink {
    fn copy(&mut self, text: &str) -> Result<(), ClipboardFailure>;
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
pub fn deliver(
    target: DeliveryTarget,
    text: &str,
    insertion: &mut impl InsertionBackend,
    clipboard: &mut impl ClipboardSink,
) -> DeliveryReport {
    deliver_with_effects(target, text, insertion, clipboard, || io::stdout().lock())
}

fn deliver_with_effects<W: Write>(
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

fn write_text(mut out: impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insertion::ClipboardRestoration;
    use crate::insertion::DirectTypingClipboard;
    use crate::insertion::InputSynthesisFailure;
    use crate::insertion::PrePasteFailure;

    fn completed() -> CompletedInsertion {
        CompletedInsertion::ClipboardPaste {
            transcript_bytes: 5,
            restoration: ClipboardRestoration::Restored,
        }
    }

    fn insertion_failure() -> InsertionFailure {
        InsertionFailure::DirectFallbackUnavailable {
            fallback_reason: PrePasteFailure::ClipboardChanged,
            failure: InputSynthesisFailure::Spawn {
                kind: io::ErrorKind::NotFound,
                message: "input helper not found".to_owned(),
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
                    kind: ClipboardFailureKind::Unavailable,
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
            failure: InputSynthesisFailure::TimedOut,
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
