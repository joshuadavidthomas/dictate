use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use dictate::dictation::DictationCommand;

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
    /// Send recording commands from compositor keybindings or scripts.
    Record {
        #[arg(value_name = "COMMAND", help = "start, stop, toggle, or cancel")]
        command: DictationCommand,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Daemon) {
        Command::Daemon => dictate::daemon::run(),
        Command::Record { command } => dictate::daemon::send(command),
    }
}
