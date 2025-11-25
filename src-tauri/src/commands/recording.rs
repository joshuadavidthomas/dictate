use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use crate::state::{RecordingSnapshot, RecordingState};
use crate::transcription::TranscriptionState;
use tauri::{AppHandle, Manager, State};

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
            broadcast
                .recording_status(
                    RecordingSnapshot::Transcribing,
                    None,
                    false,
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
                
                // Three-step orchestration
                let result = async {
                    // Step 1: Stop recording and get audio
                    let recorded_audio = crate::audio::recording::stop(&recording_handle).await?;
                    
                    // Step 2: Transcribe audio
                    let context = crate::transcription::TranscriptionContext {
                        engine_state: &transcription_handle,
                        settings: &settings_handle,
                        database: db.as_deref(),
                    };
                    let transcription = crate::transcription::Transcription::from_audio(
                        recorded_audio,
                        context,
                    ).await?;
                    
                    // Step 3: Deliver output
                    let output_mode = settings_handle.get().await.output_mode;
                    output_mode.deliver(&transcription.text, &app_clone)?;
                    
                    Ok::<_, anyhow::Error>(transcription)
                }.await;
                
                match result {
                    Ok(transcription) => {
                        // Broadcast transcription result to OSD for animation coordination
                        let duration_secs = transcription.duration_ms.unwrap_or(0) as f32 / 1000.0;
                        let model = transcription.model_id
                            .map(|id| format!("{:?}", id))
                            .unwrap_or_else(|| "unknown".to_string());
                        
                        broadcast_handle
                            .transcription_result(transcription.text.clone(), duration_secs, model)
                            .await;
                        
                        // Finish transcription state machine
                        recording_handle.finish_transcription().await;
                        
                        // Broadcast idle state
                        broadcast_handle
                            .recording_status(
                                RecordingSnapshot::Idle,
                                None,
                                true,
                                0,
                            )
                            .await;
                    }
                    Err(e) => {
                        eprintln!("[toggle_recording] Failed to process recording: {}", e);
                        recording_handle.finish_transcription().await;
                        broadcast_handle
                            .recording_status(RecordingSnapshot::Error, None, false, 0)
                            .await;
                    }
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
