mod audio;
mod broadcast;
mod commands;
mod models;
mod protocol;
mod state;
mod text;
mod transcription;
mod transport;
mod tray;
mod ui;

use broadcast::BroadcastServer;
use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Create system tray
            tray::create_tray(app.handle())?;

            // Initialize app state
            let state = AppState::new();
            app.manage(state);

            // Get a broadcast receiver for the iced OSD before spawning
            let broadcast_rx = {
                let state: tauri::State<AppState> = app.state();
                state.broadcast.subscribe()
            };
            
            // Spawn iced OSD on startup (always running) with channel receiver
            std::thread::spawn(move || {
                use crate::ui::TranscriptionConfig;
                
                let config = TranscriptionConfig {
                    max_duration: 0,
                    silence_duration: 2,
                    sample_rate: 16000,
                    insert: false,
                    copy: false,
                };
                
                eprintln!("[setup] Starting iced layer-shell overlay with channel receiver");
                
                if let Err(e) = crate::ui::run_osd_observer(broadcast_rx, config) {
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
