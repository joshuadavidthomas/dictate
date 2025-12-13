use crate::broadcast::BroadcastServer;
use crate::conf::{OsdPosition, OutputMode, SettingsState};
use crate::db::Database;
use crate::recording::{
    AudioDeviceInfo, AudioRecorder, RecordingState, SampleRate, SampleRateOption, ShortcutState,
};
use crate::transcription::{self, Model, Transcription, models};
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Instant;
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;

#[tauri::command]
pub async fn list_audio_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    AudioRecorder::list_devices().map_err(|e| format!("Failed to list audio devices: {}", e))
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
        .ok_or_else(|| "Device not found".to_string())?;

    // Filter to only supported rates
    Ok(SampleRate::ALL
        .iter()
        .filter(|rate| device.supported_sample_rates.contains(&rate.as_u32()))
        .map(|rate| rate.as_option())
        .collect())
}

#[tauri::command]
pub async fn test_audio_device(
    settings: State<'_, SettingsState>,
    device_name: Option<String>,
) -> Result<bool, String> {
    // Use configured sample rate (defaults to 16kHz if not set)
    let data = settings.get().await;
    let sample_rate = data.sample_rate;

    // Connection-only test: succeed if we can create a recorder
    match AudioRecorder::new_with_device(device_name.as_deref(), sample_rate) {
        Ok(_) => Ok(true),
        Err(e) => Err(format!("Failed to initialize audio device: {}", e)),
    }
}

#[tauri::command]
pub async fn get_audio_level(
    settings: State<'_, SettingsState>,
    device_name: Option<String>,
) -> Result<f32, String> {
    // Use configured sample rate for level probing
    let data = settings.get().await;
    let sample_rate = data.sample_rate;

    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)
        .map_err(|e| format!("Failed to initialize audio device: {}", e))?;

    recorder
        .get_audio_level()
        .map_err(|e| format!("Failed to get audio level: {}", e))
}

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

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    #[serde(flatten)]
    pub model: Model,
    pub is_downloaded: bool,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct ModelSize {
    #[serde(flatten)]
    pub model: Model,
    pub size_bytes: u64,
}

#[tauri::command]
pub async fn list_models() -> Result<Vec<ModelInfo>, String> {
    Ok(Model::all()
        .iter()
        .map(|&model| ModelInfo {
            model,
            is_downloaded: model.is_downloaded().unwrap_or(false),
            display_name: model.display_name().to_string(),
        })
        .collect())
}

#[tauri::command]
pub async fn get_model_storage_info() -> Result<models::StorageInfo, String> {
    models::storage_info().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn download_model(
    model: Model,
    broadcast: State<'_, BroadcastServer>,
) -> Result<(), String> {
    model.download(&broadcast).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn remove_model(model: Model) -> Result<(), String> {
    model.remove().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_model_sizes(
    size_cache: State<'_, Mutex<HashMap<Model, (u64, Instant)>>>,
) -> Result<Vec<ModelSize>, String> {
    let client = reqwest::Client::new();
    let mut cache = size_cache.lock().await;

    let sizes = models::get_all_model_sizes(&client, &mut cache)
        .await
        .map_err(|e| e.to_string())?;

    Ok(sizes
        .into_iter()
        .map(|(model, size)| ModelSize {
            model,
            size_bytes: size,
        })
        .collect())
}

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

#[tauri::command]
pub async fn get_setting(
    settings: State<'_, SettingsState>,
    key: String,
) -> Result<serde_json::Value, String> {
    let data = settings.get().await;

    match key.as_str() {
        "output_mode" => Ok(serde_json::to_value(data.output_mode.as_str()).unwrap()),
        "audio_device" => Ok(serde_json::to_value(&data.audio_device).unwrap()),
        "sample_rate" => Ok(serde_json::to_value(data.sample_rate).unwrap()),
        "preferred_model" => Ok(serde_json::to_value(data.preferred_model).unwrap()),
        "window_decorations" => Ok(serde_json::to_value(data.window_decorations).unwrap()),
        "osd_position" => Ok(serde_json::to_value(data.osd_position.as_str()).unwrap()),
        "shortcut" => Ok(serde_json::to_value(data.shortcut).unwrap()),
        _ => Err(format!("Unknown setting: {}", key)),
    }
}

#[tauri::command]
pub async fn set_setting(
    app: AppHandle,
    broadcast: State<'_, BroadcastServer>,
    shortcut_state: State<'_, ShortcutState>,
    settings: State<'_, SettingsState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    match key.as_str() {
        "output_mode" => {
            let mode = serde_json::from_value::<String>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            let parsed = OutputMode::from_str(&mode)?;
            settings.update(|s| s.output_mode = parsed).await
        }

        "audio_device" => {
            let device_name = serde_json::from_value::<Option<String>>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;

            // Validation
            if let Some(ref name) = device_name {
                let devices = AudioRecorder::list_devices()
                    .map_err(|e| format!("Failed to list devices: {}", e))?;
                if !devices.iter().any(|d| &d.name == name) {
                    return Err(format!("Audio device '{}' not found", name));
                }
            }

            settings.update(|s| s.audio_device = device_name).await
        }

        "sample_rate" => {
            let rate = serde_json::from_value::<u32>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            SampleRate::try_from(rate).map_err(|e| e.to_string())?;
            settings.update(|s| s.sample_rate = rate).await
        }

        "preferred_model" => {
            let model = serde_json::from_value::<Option<Model>>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;

            settings.update(|s| s.preferred_model = model).await
        }

        "window_decorations" => {
            let enabled = serde_json::from_value::<bool>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;

            settings.update(|s| s.window_decorations = enabled).await?;

            // Side effect
            if let Some(window) = app.get_webview_window("main") {
                window
                    .set_decorations(enabled)
                    .map_err(|e| format!("Failed to set decorations: {}", e))?;
            }

            Ok(())
        }

        "osd_position" => {
            let position = serde_json::from_value::<String>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            let parsed = OsdPosition::from_str(&position)?;

            settings.update(|s| s.osd_position = parsed).await?;

            // Side effect
            broadcast.osd_position_updated(parsed).await;

            Ok(())
        }

        "shortcut" => {
            let shortcut = serde_json::from_value::<Option<String>>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;

            // Unregister existing
            let backend_guard = shortcut_state.backend().await;
            if let Some(backend) = backend_guard.as_ref() {
                backend
                    .unregister()
                    .await
                    .map_err(|e| format!("Failed to unregister shortcut: {}", e))?;
            }

            settings.update(|s| s.shortcut = shortcut.clone()).await?;

            // Register new if provided
            if let Some(new_shortcut) = &shortcut
                && let Some(backend) = backend_guard.as_ref()
            {
                backend
                    .register(new_shortcut)
                    .await
                    .map_err(|e| format!("Failed to register shortcut: {}", e))?;
                log::info!("Registered new shortcut: {}", new_shortcut);
            }

            Ok(())
        }

        _ => Err(format!("Unknown setting: {}", key)),
    }
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
pub async fn validate_shortcut(shortcut: String) -> Result<bool, String> {
    // Basic validation - just check it's not empty and has some structure
    if shortcut.trim().is_empty() {
        return Err("Shortcut cannot be empty".to_string());
    }

    // Check if it contains at least one modifier and one key
    if !shortcut.contains('+') {
        return Err(
            "Shortcut must contain at least one modifier (Ctrl, Alt, Shift, Super)".to_string(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_size_serialization() {
        let size = ModelSize {
            model: Model::Whisper(crate::transcription::models::WhisperModel::Tiny),
            size_bytes: 1234567,
        };
        let json = serde_json::to_string(&size).unwrap();
        println!("ModelSize JSON: {}", json);
        assert!(json.contains("\"engine\""));
        assert!(json.contains("\"whisper\""));
        assert!(json.contains("\"id\""));
        assert!(json.contains("\"tiny\""));
        assert!(json.contains("\"size_bytes\""));
        assert!(json.contains("1234567"));
    }
}
