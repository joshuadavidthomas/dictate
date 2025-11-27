use clap::{Parser, Subcommand};

use tauri::AppHandle;

#[derive(Debug, Parser)]
#[command(
    name = "dictate",
    about = "Dictate - Voice transcription for Linux",
    version,
    propagate_version = true
)]
pub struct Cli {
    /// Command to execute when invoked from the CLI.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Toggle recording (start if idle, stop if recording)
    Toggle,
}

/// Parse CLI arguments for the first (main) instance.
/// Returns the subcommand, if any.
#[cfg(desktop)]
pub fn parse_initial_command() -> Option<Command> {
    let cli = Cli::parse();
    cli.command
}

/// Handle CLI arguments for a subsequent instance, forwarding the
/// parsed command to the already-running app instance.
pub fn handle_second_instance(app: &AppHandle, args: Vec<String>) {
    log::info!("Second instance detected with args: {:?}", args);

    match Cli::try_parse_from(args) {
        Ok(cli) => {
            if let Some(command) = cli.command {
                handle_command(app, command);
            } else {
                log::debug!("No subcommand specified in second instance");
            }
        }
        Err(e) => {
            log::error!("Failed to parse CLI from second instance: {e}");
        }
    }
}

/// Execute a CLI command against the running app.
pub fn handle_command(app: &AppHandle, command: Command) {
    log::info!("Handling command: {:?}", command);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
            log::error!("toggle_recording failed: {}", e);
        }
    });
}
