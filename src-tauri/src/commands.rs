use crate::broadcast::BroadcastServer;
use crate::conf::{OsdPosition, OutputMode, Settings, SettingsState};
use crate::db::Database;
use crate::models::{ModelEngine, ModelId, ModelInfo, ModelManager};
use crate::recording::{AudioDeviceInfo, AudioRecorder, RecordingState, SampleRate, SampleRateOption, ShortcutState};
use crate::transcription::{self, Transcription};
use serde::Serialize;
use std::str::FromStr;
use tauri::{AppHandle, Manager, State};

// ============================================================================
// Audio Commands
// ============================================================================

#[tauri::command]
pub async fn list_audio_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    AudioRecorder::list_devices().map_err(|e| format!("Failed to list audio devices: {}", e))
}

#[tauri::command]
pub async fn get_audio_device() -> Result<Option<String>, String> {
    let settings = Settings::load();
    Ok(settings.audio_device)
}

#[tauri::command]
pub async fn set_audio_device(
    settings: State<'_, SettingsState>,
    device_name: Option<String>,
) -> Result<String, String> {
    // Validate device exists if specified
    if let Some(ref name) = device_name {
        let devices =
            AudioRecorder::list_devices().map_err(|e| format!("Failed to list devices: {}", e))?;

        if !devices.iter().any(|d| &d.name == name) {
            return Err(format!("Audio device '{}' not found", name));
        }
    }

    settings.set_audio_device(device_name.clone()).await?;

    let message = match &device_name {
        Some(name) => format!("Audio device set to: {}", name),
        None => "Audio device set to system default".to_string(),
    };

    eprintln!("[set_audio_device] {}", message);
    Ok(message)
}

#[tauri::command]
pub async fn get_sample_rate() -> Result<u32, String> {
    let settings = Settings::load();
    Ok(settings.sample_rate)
}

#[tauri::command]
pub async fn get_sample_rate_options() -> Result<Vec<SampleRateOption>, String> {
    Ok(SampleRate::all_options())
}

#[tauri::command]
pub async fn get_sample_rate_options_for_device(
    device_name: Option<String>,
) -> Result<Vec<SampleRateOption>, String> {
    // If device_name is None (System Default), return all options
    if device_name.is_none() {
        return Ok(SampleRate::all_options());
    }

    // Get device's supported rates
    let devices =
        AudioRecorder::list_devices().map_err(|e| format!("Failed to list devices: {}", e))?;

    let device = devices
        .iter()
        .find(|d| Some(&d.name) == device_name.as_ref())
        .ok_or_else(|| format!("Device not found"))?;

    // Filter to only supported rates
    Ok(SampleRate::ALL
        .iter()
        .filter(|rate| device.supported_sample_rates.contains(&rate.as_u32()))
        .map(|rate| rate.as_option())
        .collect())
}

#[tauri::command]
pub async fn set_sample_rate(
    settings: State<'_, SettingsState>,
    sample_rate: u32,
) -> Result<String, String> {
    // Validate sample rate using the TryFrom trait
    SampleRate::try_from(sample_rate).map_err(|e| e.to_string())?;

    settings.set_sample_rate(sample_rate).await?;

    eprintln!("[set_sample_rate] Sample rate set to: {} Hz", sample_rate);
    Ok(format!("Sample rate set to: {} Hz", sample_rate))
}

#[tauri::command]
pub async fn test_audio_device(device_name: Option<String>) -> Result<bool, String> {
    // Use configured sample rate (defaults to 16kHz if not set)
    let settings = Settings::load();
    let sample_rate = settings.sample_rate;

    // Connection-only test: succeed if we can create a recorder
    match AudioRecorder::new_with_device(device_name.as_deref(), sample_rate) {
        Ok(_) => Ok(true),
        Err(e) => Err(format!("Failed to initialize audio device: {}", e)),
    }
}

#[tauri::command]
pub async fn get_audio_level(device_name: Option<String>) -> Result<f32, String> {
    // Use configured sample rate for level probing
    let settings = Settings::load();
    let sample_rate = settings.sample_rate;

    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)
        .map_err(|e| format!("Failed to initialize audio device: {}", e))?;

    recorder
        .get_audio_level()
        .map_err(|e| format!("Failed to get audio level: {}", e))
}

// ============================================================================
// History Commands
// ============================================================================

#[tauri::command]
pub async fn get_transcription_history(
    db: State<'_, Database>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Transcription>, String> {
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);

    transcription::list(db.pool(), limit, offset)
        .await
        .map_err(|e| format!("Failed to get transcription history: {}", e))
}

#[tauri::command]
pub async fn get_transcription_by_id(
    db: State<'_, Database>,
    id: i64,
) -> Result<Option<Transcription>, String> {
    transcription::get(db.pool(), id)
        .await
        .map_err(|e| format!("Failed to get transcription: {}", e))
}

#[tauri::command]
pub async fn delete_transcription_by_id(db: State<'_, Database>, id: i64) -> Result<bool, String> {
    transcription::delete(db.pool(), id)
        .await
        .map_err(|e| format!("Failed to delete transcription: {}", e))
}

#[tauri::command]
pub async fn search_transcription_history(
    db: State<'_, Database>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<Transcription>, String> {
    let limit = limit.unwrap_or(50);

    transcription::search(db.pool(), &query, limit)
        .await
        .map_err(|e| format!("Failed to search transcriptions: {}", e))
}

#[tauri::command]
pub async fn get_transcription_count(db: State<'_, Database>) -> Result<i64, String> {
    transcription::count(db.pool())
        .await
        .map_err(|e| format!("Failed to count transcriptions: {}", e))
}

// ============================================================================
// Models Commands
// ============================================================================

#[derive(Debug, Serialize)]
pub struct UiModelInfo {
    pub id: ModelId,
    pub engine: ModelEngine,
    pub is_downloaded: bool,
    pub is_directory: bool,
    pub download_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UiStorageInfo {
    pub models_dir: String,
    pub total_size_bytes: u64,
    pub downloaded_count: usize,
    pub available_count: usize,
}

#[derive(Debug, Serialize)]
pub struct UiModelSize {
    pub id: ModelId,
    pub size_bytes: u64,
}

fn map_model_info(info: &ModelInfo) -> UiModelInfo {
    UiModelInfo {
        id: info.id,
        engine: info.engine(),
        is_downloaded: info.is_downloaded(),
        is_directory: info.is_directory(),
        download_url: info.download_url().map(|s| s.to_string()),
    }
}

#[tauri::command]
pub async fn list_models() -> Result<Vec<UiModelInfo>, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    Ok(manager
        .list_available_models()
        .into_iter()
        .map(map_model_info)
        .collect())
}

#[tauri::command]
pub async fn get_model_storage_info() -> Result<UiStorageInfo, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    let info = manager.get_storage_info().map_err(|e| e.to_string())?;

    Ok(UiStorageInfo {
        models_dir: info.models_dir.to_string_lossy().to_string(),
        total_size_bytes: info.total_size,
        downloaded_count: info.downloaded_count,
        available_count: info.available_count,
    })
}

#[tauri::command]
pub async fn download_model(
    id: ModelId,
    broadcast: State<'_, BroadcastServer>,
) -> Result<(), String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    manager
        .download_model(id, &broadcast)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_model(id: ModelId) -> Result<(), String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    manager.remove_model(id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_preferred_model() -> Result<Option<ModelId>, String> {
    let settings = Settings::load();
    Ok(settings.preferred_model)
}

#[tauri::command]
pub async fn get_model_sizes() -> Result<Vec<UiModelSize>, String> {
    let mut manager = ModelManager::new().map_err(|e| e.to_string())?;
    let sizes = manager
        .get_all_model_sizes()
        .await
        .map_err(|e| e.to_string())?;

    Ok(sizes
        .into_iter()
        .map(|(id, size_bytes)| UiModelSize { id, size_bytes })
        .collect())
}

#[tauri::command]
pub async fn set_preferred_model(
    settings: State<'_, SettingsState>,
    model: Option<ModelId>,
) -> Result<(), String> {
    // Optional validation: ensure the model is one we know about
    if let Some(m) = model {
        let manager = ModelManager::new().map_err(|e| e.to_string())?;
        if manager.get_model_info(m).is_none() {
            return Err(format!("Unknown model: {:?}", m));
        }
    }

    settings.set_preferred_model(model).await
}

// ============================================================================
// Recording Commands
// ============================================================================

#[tauri::command]
pub async fn toggle_recording(app: AppHandle) -> Result<String, String> {
    crate::recording::toggle_recording(&app)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_status(recording: State<'_, RecordingState>) -> Result<String, String> {
    let snapshot = recording.snapshot().await;
    let state_str = snapshot.as_str();
    Ok(state_str.to_lowercase())
}

// ============================================================================
// Settings Commands
// ============================================================================

#[tauri::command]
pub async fn set_output_mode(
    settings: State<'_, SettingsState>,
    mode: String,
) -> Result<String, String> {
    let parsed = OutputMode::from_str(&mode)?;
    settings.set_output_mode(parsed).await?;
    Ok(format!("Output mode set to: {}", parsed.as_str()))
}

#[tauri::command]
pub async fn get_output_mode() -> Result<String, String> {
    let settings = Settings::load();
    Ok(settings.output_mode.as_str().to_string())
}

#[tauri::command]
pub fn get_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let git_sha = env!("GIT_SHA");
    format!("{}-{}", version, git_sha)
}

#[tauri::command]
pub async fn check_config_changed(settings: State<'_, SettingsState>) -> Result<bool, String> {
    settings.check_config_changed().await
}

#[tauri::command]
pub async fn mark_config_synced(settings: State<'_, SettingsState>) -> Result<(), String> {
    settings.mark_config_synced().await
}

#[tauri::command]
pub async fn get_window_decorations() -> Result<bool, String> {
    let settings = Settings::load();
    Ok(settings.window_decorations)
}

#[tauri::command]
pub async fn set_window_decorations(
    settings: State<'_, SettingsState>,
    app: AppHandle,
    enabled: bool,
) -> Result<String, String> {
    settings.set_window_decorations(enabled).await?;

    if let Some(window) = app.get_webview_window("main") {
        window
            .set_decorations(enabled)
            .map_err(|e| format!("Failed to set decorations: {}", e))?;
    }

    Ok(format!("Window decorations set to: {}", enabled))
}

#[tauri::command]
pub async fn get_osd_position() -> Result<String, String> {
    let settings = Settings::load();
    Ok(settings.osd_position.as_str().to_string())
}

#[tauri::command]
pub async fn set_osd_position(
    settings: State<'_, SettingsState>,
    broadcast: State<'_, BroadcastServer>,
    position: String,
) -> Result<String, String> {
    let parsed = OsdPosition::from_str(&position)?;
    settings.set_osd_position(parsed).await?;

    broadcast.osd_position_updated(parsed).await;

    Ok(format!("OSD position set to: {}", parsed.as_str()))
}

#[tauri::command]
pub async fn get_shortcut() -> Result<Option<String>, String> {
    let settings = Settings::load();
    Ok(settings.shortcut)
}

#[tauri::command]
pub async fn set_shortcut(
    settings: State<'_, SettingsState>,
    shortcut_state: State<'_, ShortcutState>,
    shortcut: Option<String>,
) -> Result<(), String> {
    // Unregister existing shortcut
    let backend_guard = shortcut_state.backend().await;
    if let Some(backend) = backend_guard.as_ref() {
        backend
            .unregister()
            .await
            .map_err(|e| format!("Failed to unregister shortcut: {}", e))?;
    }

    // Save new shortcut to config
    settings.set_shortcut(shortcut.clone()).await?;

    // Register new shortcut if provided
    if let Some(new_shortcut) = &shortcut {
        if let Some(backend) = backend_guard.as_ref() {
            backend
                .register(new_shortcut)
                .await
                .map_err(|e| format!("Failed to register shortcut: {}", e))?;

            eprintln!("[shortcut] Registered new shortcut: {}", new_shortcut);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn validate_shortcut(shortcut: String) -> Result<bool, String> {
    // Basic validation - just check it's not empty and has some structure
    if shortcut.trim().is_empty() {
        return Err("Shortcut cannot be empty".to_string());
    }
    
    // Check if it contains at least one modifier and one key
    if !shortcut.contains('+') {
        return Err("Shortcut must contain at least one modifier (Ctrl, Alt, Shift, Super)".to_string());
    }
    
    Ok(true)
}

#[tauri::command]
pub async fn get_shortcut_capabilities(
    shortcut_state: State<'_, ShortcutState>,
) -> Result<serde_json::Value, String> {
    let backend_guard = shortcut_state.backend().await;

    if let Some(backend) = backend_guard.as_ref() {
        let caps = backend.capabilities();

        Ok(serde_json::json!({
            "platform": format!("{:?}", caps.platform),
            "canRegister": caps.can_register,
            "compositor": caps.compositor,
        }))
    } else {
        Ok(serde_json::json!({
            "platform": "Unknown",
            "canRegister": false,
            "compositor": crate::recording::detect_compositor(),
        }))
    }
}
