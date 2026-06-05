use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use dictate::control::ControlCommand;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the resident Dictate daemon.
    Daemon,
    /// Control recording from compositor keybindings or scripts.
    Record {
        #[command(subcommand)]
        action: RecordAction,
    },
    #[command(hide = true)]
    App,
}

#[derive(Clone, Copy, Debug, Subcommand)]
enum RecordAction {
    /// Start recording.
    Start,
    /// Stop recording and transcribe.
    Stop,
    /// Start when idle, stop when recording.
    Toggle,
    /// Cancel the active recording without transcribing.
    Cancel,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Daemon) {
        Command::Daemon => dictate::daemon::run(),
        Command::Record { action } => dictate::control::send_command(action.into()),
        Command::App => {
            dictate::app::run();
            Ok(())
        }
    }
}

impl From<RecordAction> for ControlCommand {
    fn from(action: RecordAction) -> Self {
        match action {
            RecordAction::Start => Self::Start,
            RecordAction::Stop => Self::Stop,
            RecordAction::Toggle => Self::Toggle,
            RecordAction::Cancel => Self::Cancel,
        }
    }
}
