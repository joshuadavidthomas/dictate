use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use crate::state::{RecordingState, TranscriptionState};
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub async fn toggle_recording(
    recording: State<'_, RecordingState>,
    _transcription: State<'_, TranscriptionState>,
    _settings: State<'_, SettingsState>,
    broadcast: State<'_, BroadcastServer>,
    app: AppHandle,
) -> Result<String, String> {
    let protocol_state = recording.to_protocol_state().await;
    
    match protocol_state {
        crate::protocol::State::Idle => {
            app.emit(
                "recording-started",
                serde_json::json!({
                    "state": "recording"
                }),
            )
            .ok();

            let app_clone = app.clone();

            tokio::spawn(async move {
                let recording_handle = app_clone.state::<RecordingState>();
                let settings_handle = app_clone.state::<SettingsState>();
                let broadcast_handle = app_clone.state::<BroadcastServer>();
                if let Err(e) = crate::audio::recording::start(
                    &recording_handle,
                    &settings_handle,
                    &broadcast_handle,
                    &app_clone,
                )
                .await
                {
                    eprintln!("[toggle_recording] Failed to start recording: {}", e);
                }
            });

            Ok("started".into())
        }
        crate::protocol::State::Recording => {
            app.emit(
                "recording-stopped",
                serde_json::json!({
                    "state": "transcribing"
                }),
            )
            .ok();

            broadcast
                .broadcast_status(
                    crate::protocol::State::Transcribing,
                    None,
                    recording.elapsed_ms().await,
                )
                .await;

            let app_clone = app.clone();

            tokio::spawn(async move {
                let recording_handle = app_clone.state::<RecordingState>();
                let transcription_handle = app_clone.state::<TranscriptionState>();
                let settings_handle = app_clone.state::<SettingsState>();
                let broadcast_handle = app_clone.state::<BroadcastServer>();
                let db = app_clone.try_state::<Database>();
                if let Err(e) = crate::audio::recording::stop_and_transcribe(
                    &recording_handle,
                    &transcription_handle,
                    &settings_handle,
                    &broadcast_handle,
                    db.as_deref(),
                    &app_clone,
                )
                .await
                {
                    eprintln!("[toggle_recording] Failed to transcribe: {}", e);
                }
            });

            Ok("stopping".into())
        }
        crate::protocol::State::Transcribing | crate::protocol::State::Error => Ok("busy".into()),
    }
}

#[tauri::command]
pub async fn get_status(recording: State<'_, RecordingState>) -> Result<String, String> {
    let protocol_state = recording.to_protocol_state().await;
    let state_str = protocol_state.as_str();
    Ok(state_str.to_lowercase())
}
