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

pub fn deliver(target: DeliveryTarget, text: &str) {
    match target {
        DeliveryTarget::Stdout => deliver_stdout(text),
        DeliveryTarget::Clipboard => deliver_to_clipboard(text),
    }
}

fn deliver_to_clipboard(text: &str) {
    match copy_to_clipboard(text) {
        Ok(()) => {
            eprintln!(
                "dictation copied to clipboard ({} chars)",
                text.chars().count()
            );
        }
        Err(error) => {
            eprintln!("clipboard delivery failed: {error:#}; falling back to stdout");
            deliver_stdout(text);
        }
    }
}

fn deliver_stdout(text: &str) {
    if let Err(error) = write_text(std::io::stdout().lock(), text) {
        let _ = writeln!(
            std::io::stderr(),
            "failed to write dictation to stdout: {error}"
        );
    }
}

fn write_text(mut out: impl Write, text: &str) -> std::io::Result<()> {
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

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "broken pipe",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
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
}
