use crate::broadcast::BroadcastServer;
use crate::conf::{OsdPosition, OutputMode, Settings, SettingsState};
use tauri::{AppHandle, Manager};

pub async fn set_output_mode(settings: &SettingsState, mode: &str) -> Result<(), String> {
    let output_mode = match mode {
        "print" => OutputMode::Print,
        "copy" => OutputMode::Copy,
        "insert" => OutputMode::Insert,
        _ => return Err(format!("Invalid mode: {}", mode)),
    };

    eprintln!("[set_output_mode] Output mode set to: {:?}", output_mode);

    // Update settings and persist to disk
    settings.update(|s| s.output_mode = output_mode).await;
    if let Err(e) = settings.save().await {
        eprintln!("[set_output_mode] Failed to save config: {}", e);
        // Don't fail the command if saving fails, settings are still updated in memory
    }

    // Update mtime to reflect our save
    if let Err(e) = settings.update_config_mtime().await {
        eprintln!("[set_output_mode] Failed to update config mtime: {}", e);
    }

    Ok(())
}

pub fn get_output_mode() -> Result<String, String> {
    // Read directly from config file to avoid stale in-memory state
    let settings = Settings::load();
    let mode_str = match settings.output_mode {
        OutputMode::Print => "print",
        OutputMode::Copy => "copy",
        OutputMode::Insert => "insert",
    };
    Ok(mode_str.to_string())
}

pub async fn set_window_decorations(
    settings: &SettingsState,
    app: &AppHandle,
    enabled: bool,
) -> Result<(), String> {
    // Update settings and persist to disk
    settings.update(|s| s.window_decorations = enabled).await;
    if let Err(e) = settings.save().await {
        eprintln!("[set_window_decorations] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }

    // Update mtime to reflect our save
    if let Err(e) = settings.update_config_mtime().await {
        eprintln!(
            "[set_window_decorations] Failed to update config mtime: {}",
            e
        );
    }

    // Apply to the main window
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_decorations(enabled)
            .map_err(|e| format!("Failed to set decorations: {}", e))?;
        eprintln!(
            "[set_window_decorations] Window decorations set to: {}",
            enabled
        );
    }

    Ok(())
}

pub fn get_window_decorations() -> Result<bool, String> {
    // Read directly from config file to avoid stale in-memory state
    let settings = Settings::load();
    Ok(settings.window_decorations)
}

pub async fn set_osd_position(
    settings: &SettingsState,
    broadcast: &BroadcastServer,
    position: &str,
) -> Result<(), String> {
    let osd_position = match position {
        "top" => OsdPosition::Top,
        "bottom" => OsdPosition::Bottom,
        _ => return Err(format!("Invalid position: {}", position)),
    };

    settings.update(|s| s.osd_position = osd_position).await;
    if let Err(e) = settings.save().await {
        eprintln!("[set_osd_position] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }

    // Update mtime to reflect our save
    if let Err(e) = settings.update_config_mtime().await {
        eprintln!("[set_osd_position] Failed to update config mtime: {}", e);
    }

    // Broadcast config update to OSD
    broadcast.broadcast_config_update(osd_position).await;

    eprintln!("[set_osd_position] OSD position set to: {}", position);
    Ok(())
}

pub fn get_osd_position() -> Result<String, String> {
    let settings = Settings::load();
    let position = match settings.osd_position {
        OsdPosition::Top => "top",
        OsdPosition::Bottom => "bottom",
    };
    Ok(position.to_string())
}

pub async fn set_audio_device(
    settings: &SettingsState,
    device_name: Option<String>,
) -> Result<(), String> {
    settings.update(|s| s.audio_device = device_name.clone()).await;
    if let Err(e) = settings.save().await {
        eprintln!("[set_audio_device] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }

    // Update mtime to reflect our save
    if let Err(e) = settings.update_config_mtime().await {
        eprintln!("[set_audio_device] Failed to update config mtime: {}", e);
    }

    Ok(())
}

pub fn get_audio_device() -> Result<Option<String>, String> {
    let settings = Settings::load();
    Ok(settings.audio_device)
}

pub async fn set_sample_rate(settings: &SettingsState, sample_rate: u32) -> Result<(), String> {
    settings.update(|s| s.sample_rate = sample_rate).await;
    if let Err(e) = settings.save().await {
        eprintln!("[set_sample_rate] Failed to save config: {}", e);
        return Err(format!("Failed to save settings: {}", e));
    }

    // Update mtime to reflect our save
    if let Err(e) = settings.update_config_mtime().await {
        eprintln!("[set_sample_rate] Failed to update config mtime: {}", e);
    }

    Ok(())
}

pub fn get_sample_rate() -> Result<u32, String> {
    let settings = Settings::load();
    Ok(settings.sample_rate)
}
