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

const TEXT_MIME: &str = "text/plain;charset=utf-8";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum DeliveryTarget {
    #[default]
    Stdout,
    Clipboard,
}

#[must_use = "delivery may fail; handle the DeliveryReport"]
#[derive(Debug)]
pub(crate) enum DeliveryReport {
    DeliveredToStdout,
    DeliveredToClipboard,
    ClipboardFailedDeliveredToStdout { clipboard_failure: ClipboardFailure },
    NotDelivered(DeliveryFailure),
}

#[derive(Debug)]
pub(crate) enum DeliveryFailure {
    StdoutFailed {
        failure: TextOutputFailure,
    },
    ClipboardAndStdoutFailed {
        clipboard_failure: ClipboardFailure,
        stdout_failure: TextOutputFailure,
    },
}

impl fmt::Display for DeliveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StdoutFailed { failure } => write!(formatter, "stdout failed: {failure}"),
            Self::ClipboardAndStdoutFailed {
                clipboard_failure,
                stdout_failure,
            } => write!(
                formatter,
                "clipboard failed: {clipboard_failure}; stdout failed: {stdout_failure}"
            ),
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
    let mut clipboard = WaylandClipboardSink;

    deliver_with_effects(target, text, &mut clipboard, || io::stdout().lock())
}

fn deliver_with_effects<W: Write>(
    target: DeliveryTarget,
    text: &str,
    clipboard: &mut impl ClipboardSink,
    stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    match target {
        DeliveryTarget::Stdout => {
            let mut stdout = stdout();
            deliver_stdout(&mut stdout, text)
        }
        DeliveryTarget::Clipboard => deliver_clipboard(clipboard, text, stdout),
    }
}

fn deliver_clipboard<W: Write>(
    clipboard: &mut impl ClipboardSink,
    text: &str,
    stdout: impl FnOnce() -> W,
) -> DeliveryReport {
    match clipboard.copy(text) {
        Ok(()) => DeliveryReport::DeliveredToClipboard,
        Err(clipboard_failure) => {
            let mut stdout = stdout();
            match write_stdout(&mut stdout, text) {
                Ok(()) => DeliveryReport::ClipboardFailedDeliveredToStdout { clipboard_failure },
                Err(stdout_failure) => {
                    DeliveryReport::NotDelivered(DeliveryFailure::ClipboardAndStdoutFailed {
                        clipboard_failure,
                        stdout_failure,
                    })
                }
            }
        }
    }
}

fn deliver_stdout(stdout: &mut impl Write, text: &str) -> DeliveryReport {
    match write_stdout(stdout, text) {
        Ok(()) => DeliveryReport::DeliveredToStdout,
        Err(stdout_failure) => DeliveryReport::NotDelivered(DeliveryFailure::StdoutFailed {
            failure: stdout_failure,
        }),
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
        .context("failed to set Wayland clipboard")
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
        for target in [DeliveryTarget::Stdout, DeliveryTarget::Clipboard] {
            let value = target.to_possible_value().unwrap();

            assert_eq!(
                DeliveryTarget::from_str(value.get_name(), false).ok(),
                Some(target)
            );
        }
    }

    #[test]
    fn stdout_delivery_reports_stdout() {
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report = deliver_with_effects(DeliveryTarget::Stdout, "hello", &mut clipboard, || {
            &mut stdout
        });

        assert!(matches!(report, DeliveryReport::DeliveredToStdout));
        assert_eq!(stdout, b"hello\n");
        assert!(clipboard.copies.is_empty());
    }

    #[test]
    fn stdout_delivery_reports_writer_failure() {
        let mut clipboard = FakeClipboard::success();

        let report = deliver_with_effects(DeliveryTarget::Stdout, "hello", &mut clipboard, || {
            FailingWriter
        });

        let DeliveryReport::NotDelivered(DeliveryFailure::StdoutFailed { failure }) = report else {
            panic!("expected stdout failure report");
        };
        assert_eq!(failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(failure.message, "broken pipe");
        assert!(clipboard.copies.is_empty());
    }

    #[test]
    fn clipboard_delivery_success_reports_clipboard() {
        let mut clipboard = FakeClipboard::success();
        let mut stdout = Vec::new();

        let report =
            deliver_with_effects(DeliveryTarget::Clipboard, "hello", &mut clipboard, || {
                &mut stdout
            });

        assert!(matches!(report, DeliveryReport::DeliveredToClipboard));
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert!(stdout.is_empty());
    }

    #[test]
    fn clipboard_delivery_failure_reports_stdout_fallback() {
        let mut clipboard = FakeClipboard::failure("copy denied");
        let mut stdout = Vec::new();

        let report =
            deliver_with_effects(DeliveryTarget::Clipboard, "hello", &mut clipboard, || {
                &mut stdout
            });

        let DeliveryReport::ClipboardFailedDeliveredToStdout { clipboard_failure } = report else {
            panic!("expected fallback delivery report");
        };
        let ClipboardFailure::CopyFailed { source } = clipboard_failure;
        assert_eq!(source.to_string(), "copy denied");
        assert_eq!(clipboard.copies, vec!["hello"]);
        assert_eq!(stdout, b"hello\n");
    }

    #[test]
    fn clipboard_delivery_failure_reports_stdout_write_failure() {
        let mut clipboard = FakeClipboard::failure("copy denied");

        let report =
            deliver_with_effects(DeliveryTarget::Clipboard, "hello", &mut clipboard, || {
                FailingWriter
            });

        let DeliveryReport::NotDelivered(DeliveryFailure::ClipboardAndStdoutFailed {
            clipboard_failure,
            stdout_failure,
        }) = report
        else {
            panic!("expected clipboard and stdout failure report");
        };
        let ClipboardFailure::CopyFailed { source } = clipboard_failure;
        assert_eq!(source.to_string(), "copy denied");
        assert_eq!(stdout_failure.kind, io::ErrorKind::BrokenPipe);
        assert_eq!(stdout_failure.message, "broken pipe");
        assert_eq!(clipboard.copies, vec!["hello"]);
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
