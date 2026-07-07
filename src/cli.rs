use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use dictate::delivery::DeliveryTarget;
use dictate::dictation::DictationCommand;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
        /// Override the delivery target configured in ~/.config/dictate/config.toml.
        #[arg(long, value_enum, value_name = "TARGET")]
        delivery: Option<DeliveryTarget>,
    },
    /// Send recording commands from compositor keybindings or scripts.
    Record {
        #[arg(value_name = "COMMAND", help = "start, stop, toggle, or cancel")]
        command: DictationCommand,
    },
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
        /// Override the model configured in ~/.config/dictate/config.toml.
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
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Daemon { delivery: None }) {
        Command::Daemon { delivery } => dictate::daemon::run(delivery),
        Command::Record { command } => dictate::daemon::send(command),
        Command::Transcribe {
            wav,
            raw,
            json,
            model,
        } => transcribe_wav(wav, raw, json, model),
        Command::Debug {
            list,
            screen,
            scenario,
            stats,
            duration,
            frames,
            exit,
        } => dictate::debug::run(dictate::debug::Args {
            list,
            screen,
            scenario,
            stats: stats.map(Into::into),
            duration,
            frames,
            exit,
        }),
    }
}

fn parse_debug_duration(value: &str) -> Result<Duration, String> {
    if let Some(milliseconds) = value.strip_suffix("ms") {
        let milliseconds = u64::from_str(milliseconds)
            .map_err(|_| format!("invalid millisecond duration {value:?}"))?;
        return Ok(Duration::from_millis(milliseconds));
    }

    if let Some(seconds) = value.strip_suffix('s') {
        let seconds =
            f64::from_str(seconds).map_err(|_| format!("invalid second duration {value:?}"))?;
        return duration_from_seconds(seconds, value);
    }

    let seconds = f64::from_str(value)
        .map_err(|_| format!("invalid duration {value:?}; use 2s, 500ms, or plain seconds"))?;
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

fn transcribe_wav(wav: PathBuf, raw: bool, json: bool, model: Option<String>) -> Result<()> {
    let result = dictate::eval::transcribe_file(&wav, model.as_deref())?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else if raw {
        println!("{}", result.raw);
    } else {
        println!("{}", result.formatted);
    }

    Ok(())
}
