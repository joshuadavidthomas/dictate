use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use clap::CommandFactory;
use clap::FromArgMatches;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use dictate_desktop::DeliveryTarget;
use dictate_speech::DictationCommand;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DeliveryArg {
    Stdout,
    Clipboard,
    Insert,
}

impl From<DeliveryArg> for DeliveryTarget {
    fn from(delivery: DeliveryArg) -> Self {
        match delivery {
            DeliveryArg::Stdout => Self::Stdout,
            DeliveryArg::Clipboard => Self::Clipboard,
            DeliveryArg::Insert => Self::Insert,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DebugStatsFormat {
    Json,
}

impl From<DebugStatsFormat> for dictate::debug::StatsFormat {
    fn from(format: DebugStatsFormat) -> Self {
        match format {
            DebugStatsFormat::Json => Self::Json,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the resident Dictate daemon.
    Daemon {
        /// Override the delivery target in this build channel's config file.
        #[arg(long, value_enum, value_name = "TARGET")]
        delivery: Option<DeliveryArg>,
    },
    /// Send recording commands from compositor keybindings or scripts.
    Record {
        #[arg(value_name = "COMMAND", help = "start, stop, toggle, or cancel")]
        command: DictationCommand,
    },
    /// Insert the last completed dictation at the current cursor.
    Paste,
    /// Hide the current Dictate status overlay.
    Dismiss,
    /// Transcribe a WAV file through the dictation pipeline without the daemon.
    Transcribe {
        /// Path to a 16 kHz mono WAV file.
        #[arg(value_name = "WAV")]
        wav: PathBuf,
        /// Print the raw recognizer hypothesis instead of formatted dictation.
        ///
        /// With --json, both raw and formatted text are emitted, so this flag has no effect.
        #[arg(long)]
        raw: bool,
        /// Emit raw, formatted, timing, and model metadata as one JSON object.
        #[arg(long)]
        json: bool,
        /// Override the model in this build channel's config file.
        #[arg(long, value_name = "MODEL_ID")]
        model: Option<String>,
    },
    /// Open the interactive debug harness.
    Debug {
        /// Print registered screens and scenarios as JSON without opening a window.
        #[arg(long)]
        list: bool,
        /// Open the window with the named screen selected.
        #[arg(long, value_name = "SCREEN")]
        screen: Option<String>,
        /// Open the window with the named scenario selected.
        #[arg(long, value_name = "SCENARIO")]
        scenario: Option<String>,
        /// Stream one JSON object per frame plus a final aggregate line to stdout.
        #[arg(long, value_enum, value_name = "FORMAT")]
        stats: Option<DebugStatsFormat>,
        /// Stop after a duration such as 2s, 500ms, or plain seconds; implies --exit.
        #[arg(long, value_name = "DURATION", value_parser = parse_debug_duration)]
        duration: Option<Duration>,
        /// Stop after N rendered preview frames; implies --exit.
        #[arg(long, value_name = "N")]
        frames: Option<u64>,
        /// Close the debug window and quit when a duration or frame bound is reached.
        #[arg(long)]
        exit: bool,
    },
}

pub fn run() -> Result<()> {
    let invoked_name = std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or(env!("CARGO_PKG_NAME"))
        .to_owned();
    let matches = Cli::command().name(invoked_name).get_matches();
    let cli = Cli::from_arg_matches(&matches)?;

    match cli.command.unwrap_or(Command::Daemon { delivery: None }) {
        Command::Daemon { delivery } => dictate::daemon::run(delivery.map(Into::into)),
        Command::Record { command } => dictate::daemon::send(command),
        Command::Paste => dictate::daemon::paste_last(),
        Command::Dismiss => dictate::daemon::dismiss(),
        Command::Transcribe {
            wav,
            raw,
            json,
            model,
        } => transcribe_wav(&wav, raw, json, model.as_deref()),
        Command::Debug {
            list,
            screen,
            scenario,
            stats,
            duration,
            frames,
            exit,
        } => dictate::debug::run(
            &dictate::debug::Args {
                list,
                screen,
                scenario,
                stats: stats.map(Into::into),
                duration,
                frames,
                exit,
            },
            || {
                let settings = dictate::settings::load()?;
                settings.transcription_plan(None)
            },
        ),
    }
}

fn parse_debug_duration(value: &str) -> Result<Duration, String> {
    if let Some(milliseconds) = value.strip_suffix("ms") {
        let milliseconds = u64::from_str(milliseconds).map_err(|parse_error| {
            format!("invalid millisecond duration {value:?}: {parse_error}")
        })?;
        return Ok(Duration::from_millis(milliseconds));
    }

    if let Some(seconds) = value.strip_suffix('s') {
        let seconds = f64::from_str(seconds)
            .map_err(|parse_error| format!("invalid second duration {value:?}: {parse_error}"))?;
        return duration_from_seconds(seconds, value);
    }

    let seconds = f64::from_str(value).map_err(|parse_error| {
        format!("invalid duration {value:?}; use 2s, 500ms, or plain seconds: {parse_error}")
    })?;
    duration_from_seconds(seconds, value)
}

fn duration_from_seconds(seconds: f64, original: &str) -> Result<Duration, String> {
    if seconds.is_sign_negative() || !seconds.is_finite() {
        return Err(format!(
            "duration must be a finite non-negative value: {original:?}"
        ));
    }

    Ok(Duration::from_secs_f64(seconds))
}

fn transcribe_wav(wav: &Path, raw: bool, json: bool, model: Option<&str>) -> Result<()> {
    let settings = dictate::settings::load()?;
    let plan = settings.transcription_plan(model)?;
    let result = dictate_speech::transcribe_file(wav, plan)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else if raw {
        println!("{}", result.raw);
    } else {
        println!("{}", result.formatted);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_arguments_parse_and_convert_exhaustively() {
        for (name, argument, target) in [
            ("stdout", DeliveryArg::Stdout, DeliveryTarget::Stdout),
            (
                "clipboard",
                DeliveryArg::Clipboard,
                DeliveryTarget::Clipboard,
            ),
            ("insert", DeliveryArg::Insert, DeliveryTarget::Insert),
        ] {
            assert_eq!(DeliveryArg::from_str(name, false).ok(), Some(argument));
            assert_eq!(DeliveryTarget::from(argument), target);
        }
    }
}
