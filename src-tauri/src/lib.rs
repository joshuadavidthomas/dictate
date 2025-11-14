mod audio;
mod broadcast;
mod commands;
mod conf;
mod db;
mod history;
mod models;
mod protocol;
mod state;
mod text;
mod transcription;
mod transport;
mod tray;
mod ui;

use state::{AppState, RecordingState};
use tauri::Manager;
use tauri_plugin_cli::CliExt;

/// Helper function to handle CLI commands
fn handle_cli_command(app: &tauri::AppHandle, command: &str) {
    eprintln!("[cli] Handling command: {}", command);
    
    let app_clone = app.clone();
    let command = command.to_string();
    tauri::async_runtime::spawn(async move {
        let state: tauri::State<AppState> = app_clone.state();
        
        match command.as_str() {
            "toggle" => {
                if let Err(e) = crate::commands::toggle_recording(state, app_clone.clone()).await {
                    eprintln!("[cli] Toggle failed: {}", e);
                }
            }
            "start" => {
                let rec_state = state.recording_state.lock().await;
                if *rec_state == RecordingState::Idle {
                    drop(rec_state);
                    if let Err(e) = crate::commands::toggle_recording(state, app_clone.clone()).await {
                        eprintln!("[cli] Start failed: {}", e);
                    }
                } else {
                    eprintln!("[cli] Cannot start - already recording or transcribing");
                }
            }
            "stop" => {
                let rec_state = state.recording_state.lock().await;
                if *rec_state == RecordingState::Recording {
                    drop(rec_state);
                    if let Err(e) = crate::commands::toggle_recording(state, app_clone.clone()).await {
                        eprintln!("[cli] Stop failed: {}", e);
                    }
                } else {
                    eprintln!("[cli] Cannot stop - not currently recording");
                }
            }
            _ => eprintln!("[cli] Unknown command: {}", command)
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

            // Initialize app state
            let state = AppState::new();
            app.manage(state);
            
            // Initialize database
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    eprintln!("[setup] Initializing database...");
                    match crate::db::init_db().await {
                        Ok(pool) => {
                            let state: tauri::State<AppState> = app_handle.state();
                            let mut db_pool = state.db_pool.lock().await;
                            *db_pool = Some(pool);
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
                let settings = conf::Settings::load();
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(e) = window.set_decorations(settings.window_decorations) {
                        eprintln!("[setup] Failed to set window decorations: {}", e);
                    } else {
                        eprintln!("[setup] Window decorations set to: {}", settings.window_decorations);
                    }
                }
            }

            // Handle CLI arguments from first instance
            match app.cli().matches() {
                Ok(matches) => {
                    if let Some(subcommand) = matches.subcommand {
                        eprintln!("[cli] First instance executing: {}", subcommand.name);
                        handle_cli_command(&app.handle(), &subcommand.name);
                    }
                }
                Err(e) => eprintln!("[cli] Failed to parse CLI: {}", e)
            }

            // Get a broadcast receiver for the iced OSD before spawning
            let broadcast_rx = {
                let state: tauri::State<AppState> = app.state();
                state.broadcast.subscribe()
            };
            
            // Spawn iced OSD on startup (always running) with channel receiver
            std::thread::spawn(move || {
                use crate::ui::TranscriptionConfig;
                
                // Load OSD position from settings
                let settings = conf::Settings::load();
                let osd_position = settings.osd_position;
                
                let config = TranscriptionConfig {
                    max_duration: 0,
                    silence_duration: 2,
                    sample_rate: 16000,
                };
                
                eprintln!("[setup] Starting iced layer-shell overlay with channel receiver");
                
                if let Err(e) = crate::ui::run_osd_observer(broadcast_rx, config, osd_position) {
                    eprintln!("[setup] Failed to run OSD: {}", e);
                }
            });

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                use crate::models::ModelManager;
                use crate::transcription::TranscriptionEngine;

                eprintln!("[setup] Preloading transcription model...");

                // Create a simple runtime for this thread
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let state: tauri::State<AppState> = app_handle.state();
                    let mut engine_opt = state.engine.lock().await;
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
