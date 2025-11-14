use tauri::{AppHandle, Emitter, Manager, State};
use crate::state::{AppState, RecordingState, ActiveRecording, OutputMode};
use crate::audio::{AudioRecorder, AudioDeviceInfo, SampleRate, SampleRateOption, buffer_to_wav};
use crate::transcription::TranscriptionEngine;
use crate::history::{TranscriptionHistory, list_transcriptions, get_transcription, delete_transcription, search_transcriptions, count_transcriptions};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::convert::TryFrom;
use cpal::traits::StreamTrait;

#[derive(Clone, Serialize)]
struct StatusUpdate {
    state: String,
}

#[derive(Clone, Serialize)]
struct TranscriptionResult {
    text: String,
}

#[tauri::command]
pub async fn toggle_recording(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let mut rec_state = state.recording_state.lock().await;
    
    match *rec_state {
        RecordingState::Idle => {
            eprintln!("[toggle_recording] Starting recording");
            *rec_state = RecordingState::Recording;
            drop(rec_state);
            
            // Emit event to frontend
            app.emit("recording-started", StatusUpdate { 
                state: "recording".into() 
            }).ok();
            
            // NOTE: Don't broadcast to OSD yet - wait for the spectrum loop to send
            // the first broadcast with actual audio data. This ensures OSD appears
            // when recording is truly ready rather than during hardware initialization.
            
            // Start actual recording
            let app_clone = app.clone();
            tokio::spawn(async move {
                let state: tauri::State<AppState> = app_clone.state();
                if let Err(e) = start_recording(&state, &app_clone).await {
                    eprintln!("[toggle_recording] Failed to start recording: {}", e);
                }
            });
            
            Ok("started".into())
        }
        RecordingState::Recording => {
            eprintln!("[toggle_recording] Stopping recording");
            *rec_state = RecordingState::Transcribing;
            drop(rec_state);
            
            // Emit event to frontend
            app.emit("recording-stopped", StatusUpdate { 
                state: "transcribing".into() 
            }).ok();
            
            // Broadcast to iced OSD
            state.broadcast.broadcast_status(
                crate::protocol::State::Transcribing,
                None,
                state.elapsed_ms()
            ).await;
            
            // Stop recording and transcribe
            let app_handle = app.clone();
            tokio::spawn(async move {
                if let Err(e) = stop_and_transcribe(app_handle).await {
                    eprintln!("[toggle_recording] Failed to transcribe: {}", e);
                }
            });
            
            Ok("stopping".into())
        }
        RecordingState::Transcribing => {
            eprintln!("[toggle_recording] Already transcribing");
            Ok("busy".into())
        }
    }
}

async fn start_recording(state: &AppState, _app: &AppHandle) -> Result<(), String> {
    // Get audio settings from config
    let (device_name, sample_rate) = {
        let settings = state.settings.lock().await;
        (settings.audio_device.clone(), settings.sample_rate)
    };
    
    // Create recorder with configured device and sample rate
    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)
        .map_err(|e| e.to_string())?;
    
    // Create recording buffers
    let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = Arc::new(AtomicBool::new(false));
    
    // Create spectrum channel
    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();
    
    // Start recording stream with spectrum analysis
    let stream = recorder
        .start_recording_background(
            audio_buffer.clone(),
            stop_signal.clone(),
            None, // No silence detection for manual mode
            Some(spectrum_tx),
        )
        .map_err(|e| e.to_string())?;
    
    // Start the stream
    stream.play().map_err(|e| e.to_string())?;
    
    // Spawn task to broadcast spectrum data
    let broadcast = state.broadcast.clone();
    let start_time = state.start_time;
    tokio::spawn(async move {
        while let Some(spectrum) = spectrum_rx.recv().await {
            let ts = start_time.elapsed().as_millis() as u64;
            
            // Broadcast to iced OSD
            broadcast.broadcast_status(
                crate::protocol::State::Recording,
                Some(spectrum),
                ts
            ).await;
        }
    });
    
    // Store the active recording
    let mut current = state.current_recording.lock().await;
    *current = Some(ActiveRecording {
        audio_buffer,
        stop_signal,
        stream: Some(stream),
        start_time: std::time::Instant::now(),
    });
    
    eprintln!("[start_recording] Recording started successfully");
    Ok(())
}

async fn stop_and_transcribe(app: AppHandle) -> Result<(), String> {
    let state: tauri::State<AppState> = app.state();
    
    // Stop the recording
    let audio_buffer = {
        let mut current = state.current_recording.lock().await;
        if let Some(mut recording) = current.take() {
            // Signal stop
            recording.stop_signal.store(true, Ordering::Release);
            
            // Drop stream to stop recording
            recording.stream.take();
            
            // Small delay to ensure last samples are written
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            
            recording.audio_buffer
        } else {
            return Err("No active recording".into());
        }
    };
    
    // Get the recorded audio
    let buffer = audio_buffer.lock().unwrap().clone();
    
    if buffer.is_empty() {
        eprintln!("[stop_and_transcribe] No audio recorded");
        
        let mut rec_state = state.recording_state.lock().await;
        *rec_state = RecordingState::Idle;
        
        app.emit("transcription-complete", StatusUpdate {
            state: "idle".into()
        }).ok();
        
        return Err("No audio recorded".into());
    }
    
    eprintln!("[stop_and_transcribe] Recorded {} samples", buffer.len());
    
    // Calculate duration from buffer length and sample rate
    let duration_ms = (buffer.len() as i64 * 1000) / 16000;
    eprintln!("[stop_and_transcribe] Duration: {}ms", duration_ms);
    
    // Save to recordings directory with timestamp
    let recordings_dir = {
        use directories::ProjectDirs;
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| "Failed to get project directories".to_string())?;
        let dir = project_dirs.data_dir().join("recordings");
        tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
        dir
    };
    
    let timestamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    let audio_path = recordings_dir.join(format!("{}.wav", timestamp));
    buffer_to_wav(&buffer, &audio_path, 16000).map_err(|e| e.to_string())?;
    
    eprintln!("[stop_and_transcribe] Audio saved to: {:?}", audio_path);
    
    // Transcribe
    let text = {
        let mut engine_opt = state.engine.lock().await;
        
        // Create/load engine if needed
        if engine_opt.is_none() {
            let mut engine = TranscriptionEngine::new();
            
            // Try to find and load a model
            let model_path = {
                use crate::models::ModelManager;
                let manager = ModelManager::new().ok();
                manager.and_then(|m| {
                    // Try parakeet-v3 first, then whisper-base
                    m.get_model_path("parakeet-v3")
                        .or_else(|| m.get_model_path("whisper-base"))
                })
            };
            
            if let Some(path) = model_path {
                eprintln!("[transcribe] Loading model from: {}", path.display());
                match engine.load_model(&path.to_string_lossy()) {
                    Ok(_) => eprintln!("[transcribe] Model loaded successfully"),
                    Err(e) => eprintln!("[transcribe] Failed to load model: {}", e),
                }
            } else {
                eprintln!("[transcribe] No model found, transcription will fail");
            }
            
            *engine_opt = Some(engine);
        }
        
        let engine = engine_opt.as_mut().unwrap();
        engine.transcribe_file(&audio_path).map_err(|e| e.to_string())?
    };
    
    eprintln!("[stop_and_transcribe] Transcription: {}", text);
    
    // Save transcription to database (only if non-empty)
    if !text.trim().is_empty() {
        let db_pool = state.db_pool.lock().await;
        if let Some(pool) = db_pool.as_ref() {
            use crate::history::{NewTranscription, save_transcription};
            
            let current_output_mode = *state.output_mode.lock().await;
            let output_mode_str = match current_output_mode {
                OutputMode::Print => "print",
                OutputMode::Copy => "copy",
                OutputMode::Insert => "insert",
            }.to_string();
            
            let model_name = {
                let engine_opt = state.engine.lock().await;
                engine_opt.as_ref()
                    .and_then(|e| e.get_model_path())
                    .map(|p| p.to_string())
            };
            
            let audio_size = std::fs::metadata(&audio_path)
                .ok()
                .map(|m| m.len() as i64);
            
            let mut new_transcription = NewTranscription::new(text.clone())
                .with_output_mode(output_mode_str)
                .with_duration(duration_ms)
                .with_audio_path(audio_path.to_string_lossy().to_string());
            
            if let Some(model) = model_name {
                new_transcription = new_transcription.with_model(model);
            }
            
            if let Some(size) = audio_size {
                new_transcription = new_transcription.with_audio_size(size);
            }
            
            match save_transcription(pool, new_transcription).await {
                Ok(id) => {
                    eprintln!("[stop_and_transcribe] Saved transcription with ID: {}", id);
                }
                Err(e) => {
                    eprintln!("[stop_and_transcribe] Failed to save transcription: {}", e);
                }
            }
        } else {
            eprintln!("[stop_and_transcribe] Database not initialized, skipping save");
        }
    } else {
        eprintln!("[stop_and_transcribe] Skipping save: transcription is empty");
    }
    
    // Emit result to Svelte UI
    app.emit("transcription-result", TranscriptionResult {
        text: text.clone(),
    }).ok();
    
    // Broadcast result to iced OSD
    state.broadcast.broadcast_result(text.clone()).await;
    
    // Handle output based on configured mode
    let output_mode = *state.output_mode.lock().await;
    let text_inserter = crate::text::TextInserter::new();
    
    match output_mode {
        OutputMode::Print => {
            println!("{}", text);
        }
        OutputMode::Copy => {
            match text_inserter.copy_to_clipboard(&app, &text) {
                Ok(()) => {
                    eprintln!("[stop_and_transcribe] Text copied to clipboard");
                }
                Err(e) => {
                    eprintln!("[stop_and_transcribe] Failed to copy to clipboard: {}", e);
                    println!("{}", text); // Fallback to print
                }
            }
        }
        OutputMode::Insert => {
            match text_inserter.insert_text(&text) {
                Ok(()) => {
                    eprintln!("[stop_and_transcribe] Text inserted at cursor");
                }
                Err(e) => {
                    eprintln!("[stop_and_transcribe] Failed to insert text: {}", e);
                    println!("{}", text); // Fallback to print
                }
            }
        }
    }
    
    // Back to idle
    let mut rec_state = state.recording_state.lock().await;
    *rec_state = RecordingState::Idle;
    
    app.emit("transcription-complete", StatusUpdate {
        state: "idle".into()
    }).ok();
    
    // Broadcast idle state
    state.broadcast.broadcast_status(
        crate::protocol::State::Idle,
        None,
        state.elapsed_ms()
    ).await;
    
    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<String, String> {
    let rec_state = state.recording_state.lock().await;
    let status = match *rec_state {
        RecordingState::Idle => "idle",
        RecordingState::Recording => "recording",
        RecordingState::Transcribing => "transcribing",
    };
    Ok(status.to_string())
}

#[tauri::command]
pub async fn set_output_mode(
    state: State<'_, AppState>,
    mode: String,
) -> Result<String, String> {
    let output_mode = match mode.as_str() {
        "print" => OutputMode::Print,
        "copy" => OutputMode::Copy,
        "insert" => OutputMode::Insert,
        _ => return Err(format!("Invalid mode: {}", mode)),
    };
    
    *state.output_mode.lock().await = output_mode;
    eprintln!("[set_output_mode] Output mode set to: {:?}", output_mode);
    
    // Update settings and persist to disk
    let mut settings = state.settings.lock().await;
    settings.output_mode = output_mode;
    if let Err(e) = settings.save() {
        eprintln!("[set_output_mode] Failed to save config: {}", e);
        // Don't fail the command if saving fails, settings are still updated in memory
    }
    
    // Update mtime to reflect our save
    if let Ok(new_mtime) = crate::conf::config_mtime() {
        let mut mtime = state.config_mtime.lock().await;
        *mtime = Some(new_mtime);
    }
    
    Ok(format!("Output mode set to: {}", mode))
}

#[tauri::command]
pub async fn get_output_mode(state: State<'_, AppState>) -> Result<String, String> {
    // Read directly from config file to avoid stale in-memory state
    let settings = crate::conf::Settings::load();
    let mode_str = match settings.output_mode {
        OutputMode::Print => "print",
        OutputMode::Copy => "copy",
        OutputMode::Insert => "insert",
    };
    Ok(mode_str.to_string())
}

#[tauri::command]
pub fn get_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let git_sha = env!("GIT_SHA");
    format!("{}-{}", version, git_sha)
}

#[tauri::command]
pub async fn check_config_changed(state: State<'_, AppState>) -> Result<bool, String> {
    let current_mtime = crate::conf::config_mtime()
        .map_err(|e| format!("Failed to get config mtime: {}", e))?;
    
    let last_mtime = state.config_mtime.lock().await;
    
    Ok(last_mtime.map_or(false, |t| t != current_mtime))
}

#[tauri::command]
pub async fn update_config_mtime(state: State<'_, AppState>) -> Result<(), String> {
    let current_mtime = crate::conf::config_mtime()
        .map_err(|e| format!("Failed to get config mtime: {}", e))?;
    
    let mut mtime = state.config_mtime.lock().await;
    *mtime = Some(current_mtime);
    
    Ok(())
}

#[tauri::command]
pub async fn get_window_decorations(state: State<'_, AppState>) -> Result<bool, String> {
    // Read directly from config file to avoid stale in-memory state
    let settings = crate::conf::Settings::load();
    Ok(settings.window_decorations)
}

#[tauri::command]
pub async fn set_window_decorations(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<String, String> {
    // Update settings and persist to disk
    let mut settings = state.settings.lock().await;
    settings.window_decorations = enabled;
    if let Err(e) = settings.save() {
        eprintln!("[set_window_decorations] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }
    
    // Update mtime to reflect our save
    if let Ok(new_mtime) = crate::conf::config_mtime() {
        let mut mtime = state.config_mtime.lock().await;
        *mtime = Some(new_mtime);
    }
    
    // Apply to the main window
    if let Some(window) = app.get_webview_window("main") {
        window.set_decorations(enabled)
            .map_err(|e| format!("Failed to set decorations: {}", e))?;
        eprintln!("[set_window_decorations] Window decorations set to: {}", enabled);
    }
    
    Ok(format!("Window decorations set to: {}", enabled))
}

#[tauri::command]
pub async fn get_osd_position(state: State<'_, AppState>) -> Result<String, String> {
    let settings = crate::conf::Settings::load();
    let position = match settings.osd_position {
        crate::conf::OsdPosition::Top => "top",
        crate::conf::OsdPosition::Bottom => "bottom",
    };
    Ok(position.to_string())
}

#[tauri::command]
pub async fn set_osd_position(
    state: State<'_, AppState>,
    position: String,
) -> Result<String, String> {
    use crate::conf::OsdPosition;
    
    let osd_position = match position.as_str() {
        "top" => OsdPosition::Top,
        "bottom" => OsdPosition::Bottom,
        _ => return Err(format!("Invalid position: {}", position)),
    };
    
    let mut settings = state.settings.lock().await;
    settings.osd_position = osd_position;
    if let Err(e) = settings.save() {
        eprintln!("[set_osd_position] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }
    
    // Update mtime to reflect our save
    if let Ok(new_mtime) = crate::conf::config_mtime() {
        let mut mtime = state.config_mtime.lock().await;
        *mtime = Some(new_mtime);
    }
    
    // Broadcast config update to OSD
    state.broadcast.broadcast_config_update(osd_position).await;
    
    eprintln!("[set_osd_position] OSD position set to: {}", position);
    Ok(format!("OSD position set to: {}", position))
}

// History commands

#[tauri::command]
pub async fn get_transcription_history(
    state: State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<TranscriptionHistory>, String> {
    let db_pool = state.db_pool.lock().await;
    let pool = db_pool.as_ref()
        .ok_or_else(|| "Database not initialized".to_string())?;
    
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);
    
    list_transcriptions(pool, limit, offset)
        .await
        .map_err(|e| format!("Failed to get transcription history: {}", e))
}

#[tauri::command]
pub async fn get_transcription_by_id(
    state: State<'_, AppState>,
    id: i64,
) -> Result<Option<TranscriptionHistory>, String> {
    let db_pool = state.db_pool.lock().await;
    let pool = db_pool.as_ref()
        .ok_or_else(|| "Database not initialized".to_string())?;
    
    get_transcription(pool, id)
        .await
        .map_err(|e| format!("Failed to get transcription: {}", e))
}

#[tauri::command]
pub async fn delete_transcription_by_id(
    state: State<'_, AppState>,
    id: i64,
) -> Result<bool, String> {
    let db_pool = state.db_pool.lock().await;
    let pool = db_pool.as_ref()
        .ok_or_else(|| "Database not initialized".to_string())?;
    
    delete_transcription(pool, id)
        .await
        .map_err(|e| format!("Failed to delete transcription: {}", e))
}

#[tauri::command]
pub async fn search_transcription_history(
    state: State<'_, AppState>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<TranscriptionHistory>, String> {
    let db_pool = state.db_pool.lock().await;
    let pool = db_pool.as_ref()
        .ok_or_else(|| "Database not initialized".to_string())?;
    
    let limit = limit.unwrap_or(50);
    
    search_transcriptions(pool, &query, limit)
        .await
        .map_err(|e| format!("Failed to search transcriptions: {}", e))
}

#[tauri::command]
pub async fn get_transcription_count(
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let db_pool = state.db_pool.lock().await;
    let pool = db_pool.as_ref()
        .ok_or_else(|| "Database not initialized".to_string())?;
    
    count_transcriptions(pool)
        .await
        .map_err(|e| format!("Failed to count transcriptions: {}", e))
}

// Audio device commands

#[tauri::command]
pub async fn list_audio_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    AudioRecorder::list_devices()
        .map_err(|e| format!("Failed to list audio devices: {}", e))
}

#[tauri::command]
pub async fn get_audio_device(_state: State<'_, AppState>) -> Result<Option<String>, String> {
    let settings = crate::conf::Settings::load();
    Ok(settings.audio_device)
}

#[tauri::command]
pub async fn set_audio_device(
    state: State<'_, AppState>,
    device_name: Option<String>,
) -> Result<String, String> {
    // Validate device exists if specified
    if let Some(ref name) = device_name {
        let devices = AudioRecorder::list_devices()
            .map_err(|e| format!("Failed to list devices: {}", e))?;
        
        if !devices.iter().any(|d| &d.name == name) {
            return Err(format!("Audio device '{}' not found", name));
        }
    }
    
    // Update settings and persist to disk
    let mut settings = state.settings.lock().await;
    settings.audio_device = device_name.clone();
    if let Err(e) = settings.save() {
        eprintln!("[set_audio_device] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }
    
    // Update mtime to reflect our save
    if let Ok(new_mtime) = crate::conf::config_mtime() {
        let mut mtime = state.config_mtime.lock().await;
        *mtime = Some(new_mtime);
    }
    
    let message = match &device_name {
        Some(name) => format!("Audio device set to: {}", name),
        None => "Audio device set to system default".to_string(),
    };
    
    eprintln!("[set_audio_device] {}", message);
    Ok(message)
}

#[tauri::command]
pub async fn get_sample_rate(_state: State<'_, AppState>) -> Result<u32, String> {
    let settings = crate::conf::Settings::load();
    Ok(settings.sample_rate)
}

#[tauri::command]
pub async fn get_sample_rate_options() -> Result<Vec<SampleRateOption>, String> {
    Ok(SampleRate::all_options())
}

#[tauri::command]
pub async fn set_sample_rate(
    state: State<'_, AppState>,
    sample_rate: u32,
) -> Result<String, String> {
    // Validate sample rate using the TryFrom trait
    SampleRate::try_from(sample_rate)
        .map_err(|e| e.to_string())?;
    
    // Update settings and persist to disk
    let mut settings = state.settings.lock().await;
    settings.sample_rate = sample_rate;
    if let Err(e) = settings.save() {
        eprintln!("[set_sample_rate] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }
    
    // Update mtime to reflect our save
    if let Ok(new_mtime) = crate::conf::config_mtime() {
        let mut mtime = state.config_mtime.lock().await;
        *mtime = Some(new_mtime);
    }
    
    eprintln!("[set_sample_rate] Sample rate set to: {} Hz", sample_rate);
    Ok(format!("Sample rate set to: {} Hz", sample_rate))
}

#[tauri::command]
pub async fn test_audio_device(device_name: Option<String>) -> Result<bool, String> {
    // Try to create a recorder with the specified device
    let sample_rate = 16000;
    match AudioRecorder::new_with_device(device_name.as_deref(), sample_rate) {
        Ok(_) => Ok(true),
        Err(e) => Err(format!("Failed to initialize audio device: {}", e)),
    }
}

#[tauri::command]
pub async fn get_audio_level(device_name: Option<String>) -> Result<f32, String> {
    let sample_rate = 16000;
    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)
        .map_err(|e| format!("Failed to initialize audio device: {}", e))?;
    
    recorder.get_audio_level()
        .map_err(|e| format!("Failed to get audio level: {}", e))
}
