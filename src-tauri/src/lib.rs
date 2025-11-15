mod audio;
mod broadcast;
mod commands;
mod conf;
mod db;
mod history;
mod models;
mod osd;
mod protocol;
mod state;
mod text;
mod transcription;
mod transport;
mod tray;

use crate::broadcast::BroadcastServer;
use crate::models::ModelManager;
use crate::osd::TranscriptionConfig;
use crate::transcription::TranscriptionEngine;
use conf::SettingsState;
use db::Database;
use state::{RecordingSession, RecordingState, TranscriptionState};
use tauri::Manager;
use tauri_plugin_cli::CliExt;

/// Helper function to handle CLI commands
fn handle_cli_command(app: &tauri::AppHandle, command: &str) {
    eprintln!("[cli] Handling command: {}", command);

    let app_clone = app.clone();
    let command = command.to_string();
    tauri::async_runtime::spawn(async move {
        let session: tauri::State<RecordingSession> = app_clone.state();

        match command.as_str() {
            "toggle" => {
                // CLI commands need full state - will be implemented separately
                eprintln!("[cli] CLI toggle not yet implemented in refactored version");
            }
            "start" => {
                let rec_state = session.get_state().await;
                if rec_state == RecordingState::Idle {
                    // CLI commands need full state - will be implemented separately
                    eprintln!("[cli] CLI start not yet implemented in refactored version");
                } else {
                    eprintln!("[cli] Cannot start - already recording or transcribing");
                }
            }
            "stop" => {
                let rec_state = session.get_state().await;
                if rec_state == RecordingState::Recording {
                    // CLI commands need full state - will be implemented separately
                    eprintln!("[cli] CLI stop not yet implemented in refactored version");
                } else {
                    eprintln!("[cli] Cannot stop - not currently recording");
                }
            }
            _ => eprintln!("[cli] Unknown command: {}", command),
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_cli::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            eprintln!("[cli] Second instance detected with args: {:?}", args);

            // Parse arguments - args[0] is binary name, args[1] is subcommand
            if args.len() > 1 {
                let command = &args[1];
                handle_cli_command(app, command);
            }
        }))
        .setup(|app| {
            // Create system tray
            tray::create_tray(app.handle())?;

            // Initialize separate state components
            let settings = SettingsState::new();
            let broadcast = BroadcastServer::new();

            app.manage(RecordingSession::new());
            app.manage(TranscriptionState::new());
            app.manage(settings.clone());
            app.manage(broadcast.clone());

            // Initialize database asynchronously
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    eprintln!("[setup] Initializing database...");
                    match crate::db::init_db().await {
                        Ok(pool) => {
                            app_handle.manage(Database::new(pool));
                            eprintln!("[setup] Database initialized successfully");
                        }
                        Err(e) => {
                            eprintln!("[setup] Failed to initialize database: {}", e);
                        }
                    }
                });
            }

            // Apply window decorations setting from config
            {
                let settings_clone = settings.clone();
                let window_opt = app.get_webview_window("main");
                tauri::async_runtime::block_on(async move {
                    let settings_lock = settings_clone.settings().lock().await;
                    if let Some(window) = window_opt {
                        if let Err(e) = window.set_decorations(settings_lock.window_decorations) {
                            eprintln!("[setup] Failed to set window decorations: {}", e);
                        } else {
                            eprintln!(
                                "[setup] Window decorations set to: {}",
                                settings_lock.window_decorations
                            );
                        }
                    }
                });
            }

            // Handle CLI arguments from first instance
            match app.cli().matches() {
                Ok(matches) => {
                    if let Some(subcommand) = matches.subcommand {
                        eprintln!("[cli] First instance executing: {}", subcommand.name);
                        handle_cli_command(&app.handle(), &subcommand.name);
                    }
                }
                Err(e) => eprintln!("[cli] Failed to parse CLI: {}", e),
            }

            // Get a broadcast receiver for the iced OSD before spawning
            let broadcast_rx = broadcast.subscribe();

            // Spawn iced OSD on startup (always running) with channel receiver
            let settings_for_osd = settings.clone();
            std::thread::spawn(move || {
                // Load OSD position from settings
                let osd_position = {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async { settings_for_osd.settings().lock().await.osd_position })
                };

                let config = TranscriptionConfig {
                    max_duration: 0,
                    silence_duration: 2,
                    sample_rate: 16000,
                };

                eprintln!("[setup] Starting iced layer-shell overlay with channel receiver");

                if let Err(e) = crate::osd::run_osd_observer(broadcast_rx, config, osd_position) {
                    eprintln!("[setup] Failed to run OSD: {}", e);
                }
            });

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                eprintln!("[setup] Preloading transcription model...");

                // Create a simple runtime for this thread
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let transcription: tauri::State<TranscriptionState> = app_handle.state();
                    let mut engine_opt = transcription.engine().lock().await;
                    let mut engine = TranscriptionEngine::new();

                    // Try to find and load a model
                    let model_path = {
                        let manager = ModelManager::new().ok();
                        manager.and_then(|m| {
                            // Try parakeet-v3 first, then whisper-base
                            m.get_model_path("parakeet-v3")
                                .or_else(|| m.get_model_path("whisper-base"))
                        })
                    };

                    if let Some(path) = model_path {
                        eprintln!("[setup] Loading model from: {}", path.display());
                        match engine.load_model(&path.to_string_lossy()) {
                            Ok(_) => eprintln!("[setup] Model preloaded successfully"),
                            Err(e) => eprintln!("[setup] Failed to preload model: {}", e),
                        }
                    } else {
                        eprintln!("[setup] No model found - download one with model manager");
                    }

                    *engine_opt = Some(engine);
                });
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::toggle_recording,
            commands::get_status,
            commands::set_output_mode,
            commands::get_output_mode,
            commands::get_version,
            commands::check_config_changed,
            commands::update_config_mtime,
            commands::get_window_decorations,
            commands::set_window_decorations,
            commands::get_osd_position,
            commands::set_osd_position,
            commands::get_transcription_history,
            commands::get_transcription_by_id,
            commands::delete_transcription_by_id,
            commands::search_transcription_history,
            commands::get_transcription_count,
            commands::list_audio_devices,
            commands::get_audio_device,
            commands::set_audio_device,
            commands::get_sample_rate,
            commands::get_sample_rate_options,
            commands::set_sample_rate,
            commands::test_audio_device,
            commands::get_audio_level,
        ])
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent closing, just hide
                #[cfg(desktop)]
                {
                    _window.hide().ok();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
