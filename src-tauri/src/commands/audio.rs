use crate::audio::{AudioDeviceInfo, AudioRecorder, SampleRate, SampleRateOption};
use crate::conf::{Settings, SettingsState};
use tauri::State;

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

    recorder
        .get_audio_level()
        .map_err(|e| format!("Failed to get audio level: {}", e))
}
