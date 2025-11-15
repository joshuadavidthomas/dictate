use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use crate::state::{RecordingSnapshot, RecordingState, TranscriptionState};
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub async fn toggle_recording(
    recording: State<'_, RecordingState>,
    _transcription: State<'_, TranscriptionState>,
    _settings: State<'_, SettingsState>,
    broadcast: State<'_, BroadcastServer>,
    app: AppHandle,
) -> Result<String, String> {
    let snapshot = recording.snapshot().await;
    
    match snapshot {
        RecordingSnapshot::Idle => {
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
        RecordingSnapshot::Recording => {
            app.emit(
                "recording-stopped",
                serde_json::json!({
                    "state": "transcribing"
                }),
            )
            .ok();

            broadcast
                .send(&crate::broadcast::Message::StatusEvent {
                    state: RecordingSnapshot::Transcribing,
                    spectrum: None,
                    idle_hot: false,
                    ts: recording.elapsed_ms().await,
                })
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
        RecordingSnapshot::Transcribing | RecordingSnapshot::Error => Ok("busy".into()),
    }
}

#[tauri::command]
pub async fn get_status(recording: State<'_, RecordingState>) -> Result<String, String> {
    let snapshot = recording.snapshot().await;
    let state_str = snapshot.as_str();
    Ok(state_str.to_lowercase())
}
