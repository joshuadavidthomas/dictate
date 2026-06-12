use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use dictate::delivery::DeliveryTarget;
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
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Daemon { delivery: None }) {
        Command::Daemon { delivery } => dictate::daemon::run(delivery),
        Command::Record { command } => dictate::daemon::send(command),
    }
}
