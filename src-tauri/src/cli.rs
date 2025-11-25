use clap::{Parser, Subcommand};

use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use crate::state::{RecordingSnapshot, RecordingState, TranscriptionState};
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
                let recording = app_clone.state::<RecordingState>();
                let snapshot = recording.snapshot().await;

                match snapshot {
                    RecordingSnapshot::Idle => {
                        let settings = app_clone.state::<SettingsState>();
                        let broadcast = app_clone.state::<BroadcastServer>();

                        if let Err(e) = crate::audio::recording::start(
                            &recording, &settings, &broadcast, &app_clone,
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

                        let result = async {
                            // Step 1: Stop recording and get audio
                            let recorded_audio = crate::audio::recording::stop(&recording).await?;
                            
                            // Step 2: Transcribe audio
                            let context = crate::transcription::TranscriptionContext {
                                engine_state: &transcription,
                                settings: &settings,
                                database: db.as_deref(),
                            };
                            let transcription_result = crate::transcription::Transcription::from_audio(
                                recorded_audio,
                                context,
                            ).await?;
                            
                            // Step 3: Deliver output
                            let output_mode = settings.get().await.output_mode;
                            output_mode.deliver(&transcription_result.text, &app_clone)?;
                            
                            Ok::<_, anyhow::Error>(transcription_result)
                        }.await;
                        
                        match result {
                            Ok(transcription_result) => {
                                // Broadcast transcription result to OSD
                                let duration_secs = transcription_result.duration_ms.unwrap_or(0) as f32 / 1000.0;
                                let model = transcription_result.model_id
                                    .map(|id| format!("{:?}", id))
                                    .unwrap_or_else(|| "unknown".to_string());
                                
                                broadcast
                                    .transcription_result(transcription_result.text.clone(), duration_secs, model)
                                    .await;
                                
                                // Finish transcription state machine
                                recording.finish_transcription().await;
                                
                                // Broadcast idle state
                                broadcast
                                    .recording_status(
                                        RecordingSnapshot::Idle,
                                        None,
                                        true,
                                        0,
                                    )
                                    .await;
                            }
                            Err(e) => {
                                eprintln!("[cli] Failed to stop and transcribe: {}", e);
                                recording.finish_transcription().await;
                                broadcast
                                    .recording_status(RecordingSnapshot::Error, None, false, 0)
                                    .await;
                            }
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
                        &recording, &settings, &broadcast, &app_clone,
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

                    let result = async {
                        // Step 1: Stop recording and get audio
                        let recorded_audio = crate::audio::recording::stop(&recording).await?;
                        
                        // Step 2: Transcribe audio
                        let context = crate::transcription::TranscriptionContext {
                            engine_state: &transcription,
                            settings: &settings,
                            database: db.as_deref(),
                        };
                        let transcription_result = crate::transcription::Transcription::from_audio(
                            recorded_audio,
                            context,
                        ).await?;
                        
                        // Step 3: Deliver output
                        let output_mode = settings.get().await.output_mode;
                        output_mode.deliver(&transcription_result.text, &app_clone)?;
                        
                        Ok::<_, anyhow::Error>(transcription_result)
                    }.await;
                    
                    match result {
                        Ok(transcription_result) => {
                            // Broadcast transcription result to OSD
                            let duration_secs = transcription_result.duration_ms.unwrap_or(0) as f32 / 1000.0;
                            let model = transcription_result.model_id
                                .map(|id| format!("{:?}", id))
                                .unwrap_or_else(|| "unknown".to_string());
                            
                            broadcast
                                .transcription_result(transcription_result.text.clone(), duration_secs, model)
                                .await;
                            
                            // Finish transcription state machine
                            recording.finish_transcription().await;
                            
                            // Broadcast idle state
                            broadcast
                                .recording_status(
                                    RecordingSnapshot::Idle,
                                    None,
                                    true,
                                    0,
                                )
                                .await;
                        }
                        Err(e) => {
                            eprintln!("[cli] Failed to stop and transcribe: {}", e);
                            recording.finish_transcription().await;
                            broadcast
                                .recording_status(RecordingSnapshot::Error, None, false, 0)
                                .await;
                        }
                    }
                } else {
                    eprintln!("[cli] Cannot stop - not currently recording");
                }
            }
        }
    });
}
