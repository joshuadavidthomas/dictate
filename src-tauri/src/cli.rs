use clap::{Parser, Subcommand};

use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use crate::state::{RecordingSnapshot, RecordingState, TranscriptionState};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Parser)]
#[command(
    name = "dictate",
    about = "Dictate - Voice transcription for Linux",
    version,
    propagate_version = true,
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
                let recording = app_clone.state::<RecordingState>();
                let snapshot = recording.snapshot().await;

                match snapshot {
                    RecordingSnapshot::Idle => {
                        let settings = app_clone.state::<SettingsState>();
                        let broadcast = app_clone.state::<BroadcastServer>();

                        if let Err(e) = crate::audio::recording::start(
                            &recording,
                            &settings,
                            &broadcast,
                            &app_clone,
                        )
                        .await
                        {
                            eprintln!("[cli] Failed to start recording: {}", e);
                        }
                    }
                    RecordingSnapshot::Recording => {
                        let transcription = app_clone.state::<TranscriptionState>();
                        let settings = app_clone.state::<SettingsState>();
                        let broadcast = app_clone.state::<BroadcastServer>();
                        let db = app_clone.try_state::<Database>();

                        if let Err(e) = crate::audio::recording::stop_and_transcribe(
                            &recording,
                            &transcription,
                            &settings,
                            &broadcast,
                            db.as_deref(),
                            &app_clone,
                        )
                        .await
                        {
                            eprintln!("[cli] Failed to stop and transcribe: {}", e);
                        }
                    }
                    RecordingSnapshot::Transcribing | RecordingSnapshot::Error => {
                        eprintln!(
                            "[cli] Busy - cannot toggle while transcribing or in error state"
                        );
                    }
                }
            }
            Command::Start => {
                let recording = app_clone.state::<RecordingState>();
                let snapshot = recording.snapshot().await;

                if snapshot == RecordingSnapshot::Idle {
                    let settings = app_clone.state::<SettingsState>();
                    let broadcast = app_clone.state::<BroadcastServer>();

                    if let Err(e) = crate::audio::recording::start(
                        &recording,
                        &settings,
                        &broadcast,
                        &app_clone,
                    )
                    .await
                    {
                        eprintln!("[cli] Failed to start recording: {}", e);
                    }
                } else {
                    eprintln!("[cli] Cannot start - already recording or transcribing");
                }
            }
            Command::Stop => {
                let recording = app_clone.state::<RecordingState>();
                let snapshot = recording.snapshot().await;

                if snapshot == RecordingSnapshot::Recording {
                    let transcription = app_clone.state::<TranscriptionState>();
                    let settings = app_clone.state::<SettingsState>();
                    let broadcast = app_clone.state::<BroadcastServer>();
                    let db = app_clone.try_state::<Database>();

                    if let Err(e) = crate::audio::recording::stop_and_transcribe(
                        &recording,
                        &transcription,
                        &settings,
                        &broadcast,
                        db.as_deref(),
                        &app_clone,
                    )
                    .await
                    {
                        eprintln!("[cli] Failed to stop and transcribe: {}", e);
                    }
                } else {
                    eprintln!("[cli] Cannot stop - not currently recording");
                }
            }
        }
    });
}
