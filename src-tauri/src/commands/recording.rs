use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use crate::state::{RecordingSession, RecordingState, TranscriptionState};
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
pub async fn toggle_recording(
    session: State<'_, RecordingSession>,
    transcription: State<'_, TranscriptionState>,
    settings: State<'_, SettingsState>,
    broadcast: State<'_, BroadcastServer>,
    app: AppHandle,
) -> Result<String, String> {
    match session.get_state().await {
        RecordingState::Idle => {
            session.set_state(RecordingState::Recording).await;

            app.emit(
                "recording-started",
                serde_json::json!({
                    "state": "recording"
                }),
            )
            .ok();

            let session_clone = session.inner().clone();
            let settings_clone = settings.inner().clone();
            let broadcast_clone = broadcast.inner().clone();
            let app_clone = app.clone();

            tokio::spawn(async move {
                if let Err(e) = crate::audio::recording::start(
                    &session_clone,
                    &settings_clone,
                    &broadcast_clone,
                    &app_clone,
                )
                .await
                {
                    eprintln!("[toggle_recording] Failed to start recording: {}", e);
                }
            });

            Ok("started".into())
        }
        RecordingState::Recording => {
            session.set_state(RecordingState::Transcribing).await;

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
                    session.elapsed_ms().await,
                )
                .await;

            let session_clone = session.inner().clone();
            let transcription_clone = transcription.inner().clone();
            let settings_clone = settings.inner().clone();
            let broadcast_clone = broadcast.inner().clone();
            let app_clone = app.clone();

            tokio::spawn(async move {
                let db: Option<Database> =
                    app_clone.try_state::<Database>().map(|s| s.inner().clone());
                if let Err(e) = crate::audio::recording::stop_and_transcribe(
                    &session_clone,
                    &transcription_clone,
                    &settings_clone,
                    &broadcast_clone,
                    db.as_ref(),
                    &app_clone,
                )
                .await
                {
                    eprintln!("[toggle_recording] Failed to transcribe: {}", e);
                }
            });

            Ok("stopping".into())
        }
        RecordingState::Transcribing => Ok("busy".into()),
    }
}

#[tauri::command]
pub async fn get_status(session: State<'_, RecordingSession>) -> Result<String, String> {
    let state = session.get_state().await;
    let state_str = match state {
        RecordingState::Idle => "idle",
        RecordingState::Recording => "recording",
        RecordingState::Transcribing => "transcribing",
    };
    Ok(state_str.into())
}
