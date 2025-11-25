use clap::{Parser, Subcommand};

use tauri::{AppHandle, Manager};

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
    /// Start recording
    Start,
    /// Stop current recording
    Stop,
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
    eprintln!("[cli] Second instance detected with args: {:?}", args);

    match Cli::try_parse_from(args) {
        Ok(cli) => {
            if let Some(command) = cli.command {
                handle_command(app, command);
            } else {
                eprintln!("[cli] No subcommand specified in second instance");
            }
        }
        Err(e) => {
            eprintln!("[cli] Failed to parse CLI from second instance: {e}");
        }
    }
}

/// Execute a CLI command against the running app.
pub fn handle_command(app: &AppHandle, command: Command) {
    eprintln!("[cli] Handling command: {:?}", command);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        match command {
            Command::Toggle => {
                if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
                    eprintln!("[cli] toggle_recording failed: {}", e);
                }
            }
            Command::Start => {
                let recording = app_clone.state::<crate::recording::RecordingState>();
                if recording.snapshot().await == crate::recording::RecordingSnapshot::Idle {
                    if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
                        eprintln!("[cli] start failed: {}", e);
                    }
                } else {
                    eprintln!("[cli] Cannot start - already recording or transcribing");
                }
            }
            Command::Stop => {
                let recording = app_clone.state::<crate::recording::RecordingState>();
                if recording.snapshot().await == crate::recording::RecordingSnapshot::Recording {
                    if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
                        eprintln!("[cli] stop failed: {}", e);
                    }
                } else {
                    eprintln!("[cli] Cannot stop - not currently recording");
                }
            }
        }
    });
}
