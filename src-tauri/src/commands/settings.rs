use crate::broadcast::BroadcastServer;
use crate::conf::{OsdPosition, OutputMode, Settings, SettingsState};
use crate::state::ShortcutState;
use std::str::FromStr;
use tauri::Manager;
use tauri::{AppHandle, State};

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
            "compositor": crate::platform::display::detect_compositor(),
        }))
    }
}
