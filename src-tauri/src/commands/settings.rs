use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn set_output_mode(
    settings: State<'_, SettingsState>,
    mode: String,
) -> Result<String, String> {
    crate::conf::operations::set_output_mode(&settings, &mode).await?;
    Ok(format!("Output mode set to: {}", mode))
}

#[tauri::command]
pub async fn get_output_mode() -> Result<String, String> {
    crate::conf::operations::get_output_mode()
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
pub async fn update_config_mtime(settings: State<'_, SettingsState>) -> Result<(), String> {
    settings.update_config_mtime().await
}

#[tauri::command]
pub async fn get_window_decorations() -> Result<bool, String> {
    crate::conf::operations::get_window_decorations()
}

#[tauri::command]
pub async fn set_window_decorations(
    settings: State<'_, SettingsState>,
    app: AppHandle,
    enabled: bool,
) -> Result<String, String> {
    crate::conf::operations::set_window_decorations(&settings, &app, enabled).await?;
    Ok(format!("Window decorations set to: {}", enabled))
}

#[tauri::command]
pub async fn get_osd_position() -> Result<String, String> {
    crate::conf::operations::get_osd_position()
}

#[tauri::command]
pub async fn set_osd_position(
    settings: State<'_, SettingsState>,
    broadcast: State<'_, BroadcastServer>,
    position: String,
) -> Result<String, String> {
    crate::conf::operations::set_osd_position(&settings, &broadcast, &position).await?;
    Ok(format!("OSD position set to: {}", position))
}
