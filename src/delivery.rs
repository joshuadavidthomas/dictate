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

pub fn deliver(target: DeliveryTarget, text: &str) -> Result<()> {
    match target {
        DeliveryTarget::Stdout => {
            deliver_stdout(text);
            Ok(())
        }
        DeliveryTarget::Clipboard => deliver_to_clipboard(text),
    }
}

fn deliver_to_clipboard(text: &str) -> Result<()> {
    match copy_to_clipboard(text) {
        Ok(()) => {
            eprintln!(
                "dictation copied to clipboard ({} chars)",
                text.chars().count()
            );
            Ok(())
        }
        Err(error) => {
            eprintln!("clipboard delivery failed: {error:#}; falling back to stdout");
            deliver_stdout(text);
            Ok(())
        }
    }
}

fn deliver_stdout(text: &str) {
    println!("{text}");
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
