mod audio;
mod models;
mod protocol;
mod server;
mod socket;
mod text;
mod transcription;
mod transport;
mod ui;

use crate::audio::{AudioRecorder, SilenceDetector};
use crate::models::ModelManager;
use crate::server::{SocketClient, SocketServer};
use crate::socket::DEFAULT_SOCKET_PATH;
use crate::text::TextInserter;
use crate::transcription::TranscriptionEngine;
use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use jiff::Zoned;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "dictate")]
#[command(about = "Lightweight CLI voice transcription service for Linux")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the transcription service
    Service {
        /// Unix socket path
        #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
        socket_path: String,

        /// Model to load (e.g., whisper-base, parakeet-v0.3)
        #[arg(long, default_value = "whisper-base")]
        model: String,

        /// Unload model after inactivity (seconds, 0 = never unload)
        #[arg(long, default_value = "0")]
        idle_timeout: u64,
    },

    /// Record audio and return transcription
    Transcribe {
        /// Type text at cursor position
        #[arg(long)]
        insert: bool,

        /// Copy text to clipboard
        #[arg(long)]
        copy: bool,

        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Maximum recording duration in seconds
        #[arg(long, default_value = "30")]
        max_duration: u64,

        /// Silence duration before auto-stopping in seconds
        #[arg(long, default_value = "2")]
        silence_duration: u64,

        /// Audio sample rate in Hz
        #[arg(long, default_value = "16000")]
        sample_rate: u32,

        /// Service socket path
        #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
        socket_path: String,
    },

    /// Check service health and configuration
    Status {
        /// Service socket path
        #[arg(long, default_value = DEFAULT_SOCKET_PATH)]
        socket_path: String,
    },

    /// List available audio recording devices
    Devices,

    /// Manage transcription models
    Models {
        #[command(subcommand)]
        action: ModelAction,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum ModelAction {
    /// Show downloaded and available models
    List,
    /// Download model from HuggingFace
    Download { model: String },
    /// Remove local model file
    Remove { model: String },
}

fn get_uid() -> String {
    std::env::var("UID").unwrap_or_else(|_| {
        // Fallback: use nix to get actual UID
        nix::unistd::getuid().to_string()
    })
}

fn expand_socket_path(path: &str) -> String {
    let expanded = path.replace("$UID", &get_uid());

    // Support $RUNTIME_DIRECTORY for systemd RuntimeDirectory=
    if let Ok(runtime_dir) = std::env::var("RUNTIME_DIRECTORY") {
        expanded.replace("$RUNTIME_DIRECTORY", &runtime_dir)
    } else {
        expanded
    }
}

pub fn get_recordings_dir() -> Result<PathBuf> {
    let data_dir = directories::BaseDirs::new()
        .ok_or_else(|| anyhow!("Could not find data directory"))?
        .data_local_dir()
        .join("dictate")
        .join("recordings");

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&data_dir)?;

    Ok(data_dir)
}

pub fn get_recording_path() -> Result<PathBuf> {
    let recordings_dir = get_recordings_dir()?;
    let timestamp = Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    Ok(recordings_dir.join(format!("{}.wav", timestamp)))
}

async fn standalone_transcribe(
    max_duration: u64,
    silence_duration: u64,
    insert: bool,
    copy: bool,
    format: OutputFormat,
) -> anyhow::Result<()> {
    println!("Recording...");

    // Create audio recorder
    let recorder = AudioRecorder::new()
        .map_err(|e| anyhow::anyhow!("Failed to create audio recorder: {}", e))?;

    // Create silence detector with configurable duration
    let silence_detector = Some(SilenceDetector::new(
        0.01,
        Duration::from_secs(silence_duration),
    ));

    // Get recording path in app data directory
    let output_path =
        get_recording_path().map_err(|e| anyhow::anyhow!("Failed to get recording path: {}", e))?;

    // Record audio
    let duration = recorder
        .record_to_wav(
            &output_path,
            Duration::from_secs(max_duration),
            silence_detector,
        )
        .map_err(|e| anyhow::anyhow!("Failed to record audio: {}", e))?;

    println!(
        "Recording complete ({:.1}s), transcribing...",
        duration.as_secs_f32()
    );

    // Load model and transcribe
    let mut engine = TranscriptionEngine::new();
    let model_manager = ModelManager::new()
        .map_err(|e| anyhow::anyhow!("Failed to create model manager: {}", e))?;

    let model_name = "whisper-base";
    let model_path = model_manager.get_model_path(model_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Model '{}' not found. Download it with: dictate models download {}",
            model_name,
            model_name
        )
    })?;

    engine
        .load_model(&model_path.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("Failed to load model: {}", e))?;

    let text = engine
        .transcribe_file(output_path)
        .map_err(|e| anyhow::anyhow!("Transcription failed: {}", e))?;

    // Handle the transcribed text
    let inserter = TextInserter::new();

    // Handle --insert flag
    if insert {
        match inserter.insert_text(&text) {
            Ok(()) => {
                println!("Text inserted at cursor position");
            }
            Err(e) => {
                eprintln!("Failed to insert text: {}", e);
                println!("{}", text);
            }
        }
    }

    // Handle --copy flag
    if copy {
        match inserter.copy_to_clipboard(&text) {
            Ok(()) => {
                println!("Text copied to clipboard");
            }
            Err(e) => {
                eprintln!("Failed to copy to clipboard: {}", e);
                println!("{}", text);
            }
        }
    }

    // If neither --insert nor --copy specified, output text
    if !insert && !copy {
        match format {
            OutputFormat::Text => {
                println!("{}", text);
            }
            OutputFormat::Json => {
                let json = serde_json::json!({
                    "text": text,
                    "duration": duration.as_secs_f32(),
                });
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Service {
            socket_path,
            model,
            idle_timeout,
        } => {
            // Expand $UID in socket path
            let expanded_socket_path = expand_socket_path(&socket_path);

            eprintln!("Starting dictate service");
            eprintln!("Socket: {}", expanded_socket_path);
            eprintln!("Model: {}", model);
            eprintln!("Idle timeout: {}s", idle_timeout);

            let mut server = match SocketServer::new(&expanded_socket_path, &model, idle_timeout) {
                Ok(server) => server,
                Err(e) => {
                    eprintln!("Failed to create socket server: {}", e);
                    return;
                }
            };

            if let Err(e) = server.run().await {
                eprintln!("Socket server error: {}", e);
            }
        }

        Commands::Transcribe {
            insert,
            copy,
            format,
            max_duration,
            silence_duration,
            sample_rate,
            socket_path,
        } => {
            let expanded_socket_path = expand_socket_path(&socket_path);
            
            // Check if JSON format is requested - UI doesn't support this yet
            if !matches!(format, OutputFormat::Text) {
                eprintln!("Error: JSON output format is not supported with UI mode");
                eprintln!("Tip: Use --format text (default) for transcription with UI");
                return;
            }
            
            // Launch UI-driven transcription
            let config = crate::ui::TranscriptionConfig {
                max_duration,
                silence_duration,
                sample_rate,
                insert,
                copy,
            };
            
            if let Err(e) = crate::ui::run_osd(&expanded_socket_path, config) {
                eprintln!("UI transcription failed: {}", e);
                eprintln!();
                eprintln!("Make sure the service is running:");
                eprintln!("  dictate service");
                eprintln!("Or with systemd:");
                eprintln!("  systemctl --user start dictate");
            }
        }

        Commands::Status { socket_path } => {
            println!("Checking service status with socket_path={:?}", socket_path);

            let expanded_socket_path = expand_socket_path(&socket_path);

            let client = SocketClient::new(expanded_socket_path);

            match client.status().await {
                Ok(response) => {
                    match response {
                        crate::protocol::Response::Status {
                            service_running,
                            model_loaded,
                            model_path,
                            audio_device,
                            uptime_seconds,
                            last_activity_seconds_ago,
                            ..
                        } => {
                            println!("Service Status:");
                            let status_json = serde_json::json!({
                                "service_running": service_running,
                                "model_loaded": model_loaded,
                                "model_path": model_path,
                                "audio_device": audio_device,
                                "uptime_seconds": uptime_seconds,
                                "last_activity_seconds_ago": last_activity_seconds_ago,
                            });
                            match serde_json::to_string_pretty(&status_json) {
                                Ok(json) => println!("{}", json),
                                Err(e) => eprintln!("Failed to serialize status to JSON: {}", e),
                            }
                        }
                        crate::protocol::Response::Error { error, .. } => {
                            eprintln!("Error from service: {}", error);
                        }
                        _ => {
                            eprintln!("Unexpected response type");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to get status: {}", e);
                }
            }
        }

        Commands::Devices => {
            println!("Listing audio devices");

            match AudioRecorder::list_devices() {
                Ok(devices) => {
                    println!("Available Audio Devices:");
                    println!(
                        "{:<30} {:<10} {:<20} Formats",
                        "Name", "Default", "Sample Rates"
                    );
                    println!("{}", "-".repeat(80));

                    for device in devices {
                        let default_str = if device.is_default { "YES" } else { "NO" };
                        let sample_rates = device
                            .supported_sample_rates
                            .iter()
                            .take(3)
                            .map(|sr| sr.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");

                        let formats = device
                            .supported_formats
                            .iter()
                            .take(2)
                            .map(|f| format!("{:?}", f))
                            .collect::<Vec<_>>()
                            .join(", ");

                        println!(
                            "{:<30} {:<10} {:<20} {}",
                            &device.name[..device.name.len().min(30)],
                            default_str,
                            sample_rates,
                            formats
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Failed to list audio devices: {}", e);
                }
            }
        }

        Commands::Models { action } => {
            match ModelManager::new() {
                Ok(mut manager) => {
                    match action {
                        ModelAction::List => {
                            println!("Available Models:");
                            println!(
                                "{:<20} {:<10} {:<15} {:<10} Path",
                                "Name", "Type", "Size", "Downloaded"
                            );
                            println!("{}", "-".repeat(90));

                            // Fetch all sizes in parallel first
                            let sizes = match manager.get_all_model_sizes().await {
                                Ok(sizes) => sizes,
                                Err(e) => {
                                    eprintln!("Warning: Failed to fetch model sizes: {}", e);
                                    HashMap::new()
                                }
                            };

                            let models = manager.list_available_models();
                            for model in models {
                                let downloaded = if model.is_downloaded() { "YES" } else { "NO" };

                                // Get size from parallel fetch result
                                let size_str = if let Some(&size) = sizes.get(model.name()) {
                                    format!("{}MB", size / 1_048_576)
                                } else {
                                    "Unknown".to_string()
                                };

                                let engine_type = match model.engine_type() {
                                    crate::models::EngineType::Whisper => "Whisper",
                                    crate::models::EngineType::Parakeet => "Parakeet",
                                };

                                let path = model
                                    .local_path
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "N/A".to_string());

                                println!(
                                    "{:<20} {:<10} {:<15} {:<10} {}",
                                    model.name(),
                                    engine_type,
                                    size_str,
                                    downloaded,
                                    path
                                );
                            }

                            // Show storage info
                            if let Ok(storage) = manager.get_storage_info() {
                                println!("\nStorage Information:");
                                println!("Models Directory: {}", storage.models_dir.display());
                                println!(
                                    "Downloaded: {}/{} models",
                                    storage.downloaded_count, storage.available_count
                                );
                                println!("Total Size: {} MB", storage.total_size / 1_048_576);
                            }
                        }
                        ModelAction::Download { model } => {
                            println!("Downloading model: {}", model);

                            // Check if model exists
                            if let Some(model_info) = manager.get_model_info(&model) {
                                if model_info.is_downloaded() {
                                    println!(
                                        "Model '{}' is already downloaded at: {}",
                                        model,
                                        model_info
                                            .local_path
                                            .as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_else(|| "unknown".to_string())
                                    );
                                } else if let Err(e) = manager.download_model(&model).await {
                                    eprintln!("Failed to download model '{}': {}", model, e);
                                }
                            } else {
                                eprintln!("Model '{}' not found. Available models:", model);
                                for model_info in manager.list_available_models() {
                                    println!("  - {}", model_info.name());
                                }
                            }
                        }
                        ModelAction::Remove { model } => {
                            println!("Removing model: {}", model);

                            if let Some(model_info) = manager.get_model_info(&model) {
                                if !model_info.is_downloaded() {
                                    println!("Model '{}' is not downloaded", model);
                                } else if let Err(e) = manager.remove_model(&model).await {
                                    eprintln!("Failed to remove model '{}': {}", model, e);
                                }
                            } else {
                                eprintln!("Model '{}' not found", model);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to initialize model manager: {}", e);
                }
            }
        }
    }
}
