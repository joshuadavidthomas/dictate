use std::path::PathBuf;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;
use dictate::delivery::DeliveryTarget;
use dictate::dictation::DictationCommand;
use dictate::models::ModelCatalogEntry;
use dictate::text::DictationFormatter;
use dictate::transcription::TranscriptionResult;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
        #[arg(long)]
        raw: bool,
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
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Daemon { delivery: None }) {
        Command::Daemon { delivery } => dictate::daemon::run(delivery),
        Command::Record { command } => dictate::daemon::send(command),
        Command::Transcribe { wav, raw, model } => transcribe_wav(wav, raw, model),
        Command::Debug {
            list,
            screen,
            scenario,
        } => dictate::debug::run(dictate::debug::Args {
            list,
            screen,
            scenario,
        }),
    }
}

fn transcribe_wav(wav: PathBuf, raw: bool, model: Option<String>) -> Result<()> {
    let settings = dictate::settings::load()?;
    let model = match model {
        Some(model_id) => dictate::models::model_by_id(&model_id).ok_or_else(|| {
            anyhow!(
                "unknown model id {:?}; valid model ids: {}; example: --model {}",
                model_id,
                valid_model_ids(),
                dictate::models::DEFAULT_MODEL_ID.as_str()
            )
        })?,
        None => settings.model()?,
    };
    let model_dir = model.ensure_downloaded()?;
    let recognizer = model.create_recognizer(&model_dir)?;
    let utterance = dictate::audio::load_wav_utterance(&wav)?;

    match dictate::transcription::transcribe(&recognizer, &utterance) {
        TranscriptionResult::Transcript(raw_transcript) if raw => {
            println!("{}", raw_transcript.as_str());
        }
        TranscriptionResult::Transcript(raw_transcript) => {
            let formatted =
                DictationFormatter.format(raw_transcript, &settings.dictation_context());
            println!("{}", formatted.as_str());
        }
        TranscriptionResult::NoTranscript(failure) => {
            bail!("{}", failure.message());
        }
    }

    Ok(())
}

fn valid_model_ids() -> String {
    ModelCatalogEntry::all()
        .iter()
        .map(|model| model.id().as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
