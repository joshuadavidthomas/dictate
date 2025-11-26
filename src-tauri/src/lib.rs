mod broadcast;
mod cli;
mod commands;
mod conf;
mod db;
mod osd;
mod recording;
mod transcription;
mod tray;

use crate::broadcast::BroadcastServer;
use crate::recording::{RecordingState, ShortcutState};
use crate::transcription::{LoadedEngine, Model};
use conf::SettingsState;
use db::Database;
use std::collections::HashMap;
use std::time::Instant;
use tauri::Manager;
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(desktop)]
    let initial_command = cli::parse_initial_command();

    #[cfg(not(desktop))]
    let initial_command: Option<cli::Command> = None;

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            cli::handle_second_instance(app, args);
        }))
        .setup(move |app| {
            // Create system tray
            tray::create_tray(app.handle())?;

            // Initialize separate state components
            app.manage(RecordingState::new());
            app.manage(Mutex::new(None::<(Model, LoadedEngine)>));
            app.manage(Mutex::new(HashMap::<Model, (u64, Instant)>::new()));
            app.manage(SettingsState::new());
            app.manage(BroadcastServer::new());
            app.manage(ShortcutState::new());

            // Setup broadcast  Tauri events bridge
            let broadcast: tauri::State<BroadcastServer> = app.state();
            broadcast.spawn_tauri_bridge(app.handle().clone());

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
                let settings_handle: tauri::State<SettingsState> = app.state();
                let window_opt = app.get_webview_window("main");
                tauri::async_runtime::block_on(async move {
                    let settings_data = settings_handle.get().await;
                    if let Some(window) = window_opt {
                        if let Err(e) = window.set_decorations(settings_data.window_decorations) {
                            eprintln!("[setup] Failed to set window decorations: {}", e);
                        } else {
                            eprintln!(
                                "[setup] Window decorations set to: {}",
                                settings_data.window_decorations
                            );
                        }
                    }
                });
            }

            // Initialize keyboard shortcut backend
            {
                let settings_handle: tauri::State<SettingsState> = app.state();
                let shortcut_handle: tauri::State<ShortcutState> = app.state();
                let app_clone = app.handle().clone();

                tauri::async_runtime::block_on(async move {
                    let backend = crate::recording::create_backend(app_clone.clone());
                    shortcut_handle.set_backend(backend).await;

                    let settings_data = settings_handle.get().await;
                    if let Some(shortcut_str) = &settings_data.shortcut {
                        let backend_guard = shortcut_handle.backend().await;
                        if let Some(backend) = backend_guard.as_ref()
                            && let Err(e) = backend.register(shortcut_str).await
                        {
                            eprintln!("[setup] Failed to register shortcut: {}", e);
                        }
                    }
                });
            }

            // Handle CLI arguments from first instance
            #[cfg(desktop)]
            if let Some(command) = initial_command {
                cli::handle_command(app.handle(), command);
            }

            // Get a broadcast receiver for the iced OSD before spawning
            let broadcast: tauri::State<BroadcastServer> = app.state();
            let broadcast_rx = broadcast.subscribe();

            // Spawn iced OSD on startup (always running) with channel receiver
            let settings_handle: tauri::State<SettingsState> = app.state();
            let osd_position =
                tauri::async_runtime::block_on(async { settings_handle.get().await.osd_position });

            std::thread::spawn(move || {
                eprintln!("[setup] Starting iced layer-shell overlay with channel receiver");

                if let Err(e) = crate::osd::run_osd_observer(broadcast_rx, osd_position) {
                    eprintln!("[setup] Failed to run OSD: {}", e);
                }
            });

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                eprintln!("[setup] Preloading transcription model...");

                // Create a simple runtime for this thread
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let transcription: tauri::State<Mutex<Option<(Model, LoadedEngine)>>> =
                        app_handle.state();
                    let settings_handle: tauri::State<SettingsState> = app_handle.state();
                    let mut cache = transcription.lock().await;

                    // Skip if already loaded
                    if cache.is_some() {
                        return;
                    }

                    let settings_data = settings_handle.get().await;

                    // Use catalog to find preferred model or fallback
                    let model_id = crate::transcription::Model::preferred_or_default(settings_data.preferred_model);

                    // Check if model is downloaded
                    match model_id.is_downloaded() {
                        Ok(true) => {
                            eprintln!("[setup] Loading model {:?}", model_id);

                            // Get model path and load engine
                            match model_id.local_path() {
                                Ok(path) => {
                                    use transcribe_rs::{
                                        TranscriptionEngine as TranscribeTrait,
                                        engines::parakeet::{ParakeetEngine, ParakeetModelParams},
                                        engines::whisper::WhisperEngine,
                                    };

                                    let load_result = if model_id.is_directory() {
                                        // Parakeet model
                                        let mut parakeet_engine = ParakeetEngine::new();
                                        parakeet_engine.load_model_with_params(&path, ParakeetModelParams::int8())
                                            .map(|_| LoadedEngine::Parakeet { engine: parakeet_engine })
                                            .map_err(|e| format!("Failed to load Parakeet model: {}", e))
                                    } else {
                                        // Whisper model
                                        let mut whisper_engine = WhisperEngine::new();
                                        whisper_engine.load_model(&path)
                                            .map(|_| LoadedEngine::Whisper { engine: whisper_engine })
                                            .map_err(|e| format!("Failed to load Whisper model: {}", e))
                                    };

                                    match load_result {
                                        Ok(engine) => {
                                            *cache = Some((model_id, engine));
                                            eprintln!("[setup] Model preloaded successfully");
                                        }
                                        Err(e) => eprintln!("[setup] Failed to preload model: {}", e),
                                    }
                                }
                                Err(e) => eprintln!("[setup] Failed to get model path: {}", e),
                            }
                        }
                        Ok(false) => {
                            eprintln!("[setup] Model {:?} not downloaded - download it with model manager", model_id);
                        }
                        Err(e) => {
                            eprintln!("[setup] Failed to check model download status: {}", e);
                        }
                    }
                });
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::toggle_recording,
            commands::get_status,
            commands::get_setting,
            commands::set_setting,
            commands::get_version,
            commands::check_config_changed,
            commands::mark_config_synced,
            commands::validate_shortcut,
            commands::get_shortcut_capabilities,
            commands::get_transcription_history,
            commands::get_transcription_by_id,
            commands::delete_transcription_by_id,
            commands::search_transcription_history,
            commands::get_transcription_count,
            commands::list_audio_devices,
            commands::get_sample_rate_options,
            commands::get_sample_rate_options_for_device,
            commands::test_audio_device,
            commands::get_audio_level,
            commands::list_models,
            commands::get_model_storage_info,
            commands::download_model,
            commands::remove_model,
            commands::get_model_sizes,
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
