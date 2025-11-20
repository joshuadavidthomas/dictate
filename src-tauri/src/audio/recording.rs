use crate::audio::{AudioRecorder, buffer_to_wav};
use crate::broadcast::BroadcastServer;
use crate::conf::{OutputMode, SettingsState};
use crate::db::Database;
use crate::history::NewTranscription;
use crate::models::{ModelId, ModelManager, ParakeetModel, WhisperModel};
use crate::state::{RecordingSnapshot, RecordingState, TranscriptionState};
use crate::transcription::TranscriptionEngine;
use cpal::traits::StreamTrait;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tauri::AppHandle;

#[derive(Clone, Serialize)]
struct StatusUpdate {
    state: String,
}

#[derive(Clone, Serialize)]
struct TranscriptionResult {
    text: String,
}

pub async fn start(
    recording: &RecordingState,
    settings: &SettingsState,
    broadcast: &BroadcastServer,
    _app: &AppHandle,
) -> Result<(), String> {
    // Get audio settings from config
    let settings_data = settings.get().await;
    let device_name = settings_data.audio_device.clone();
    let sample_rate = settings_data.sample_rate;

    // Create recorder with configured device and sample rate
    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)
        .map_err(|e| e.to_string())?;

    // Create recording buffers
    let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = Arc::new(AtomicBool::new(false));

    // Create spectrum channel
    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();

    // Start recording stream with spectrum analysis
    let stream = recorder
        .start_recording_background(
            audio_buffer.clone(),
            stop_signal.clone(),
            None, // No silence detection for manual mode
            Some(spectrum_tx),
        )
        .map_err(|e| e.to_string())?;

    // Start the stream
    stream.play().map_err(|e| e.to_string())?;

    // Handle spectrum broadcasting
    // Spawn a background task that reads from spectrum_rx and broadcasts
    let broadcast = broadcast.clone();
    let start_time = std::time::Instant::now();
    tokio::spawn(async move {
        while let Some(spectrum) = spectrum_rx.recv().await {
            let ts = start_time.elapsed().as_millis() as u64;
            broadcast
                .recording_status(RecordingSnapshot::Recording, Some(spectrum), false, ts)
                .await;
        }
    });

    // Start recording with the new state API
    recording.start_recording(stream, audio_buffer, stop_signal).await;

    eprintln!("[start_recording] Recording started successfully");
    Ok(())
}

pub async fn stop_and_transcribe(
    recording: &RecordingState,
    transcription_state: &TranscriptionState,
    settings: &SettingsState,
    broadcast: &BroadcastServer,
    db: Option<&Database>,
    app: &AppHandle,
) -> Result<(), String> {
    // Stop the recording and get the audio buffer
    let audio_buffer = recording
        .stop_recording()
        .await
        .ok_or_else(|| "No active recording".to_string())?;

    // Small delay to ensure last samples are written
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the recorded audio
    let buffer = audio_buffer.lock().unwrap().clone();

    if buffer.is_empty() {
        eprintln!("[stop_and_transcribe] No audio recorded");

        recording.finish_transcription().await;

        return Err("No audio recorded".into());
    }

    eprintln!("[stop_and_transcribe] Recorded {} samples", buffer.len());

    // Calculate duration from buffer length and sample rate
    let duration_ms = (buffer.len() as i64 * 1000) / 16000;
    eprintln!("[stop_and_transcribe] Duration: {}ms", duration_ms);

    // Save to recordings directory with timestamp
    let recordings_dir = {
        use directories::ProjectDirs;
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| "Failed to get project directories".to_string())?;
        let dir = project_dirs.data_dir().join("recordings");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        dir
    };

    let timestamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    let audio_path = recordings_dir.join(format!("{}.wav", timestamp));
    buffer_to_wav(&buffer, &audio_path, 16000).map_err(|e| e.to_string())?;

    eprintln!("[stop_and_transcribe] Audio saved to: {:?}", audio_path);

    // Transcribe
    let text = {
        let mut engine_opt = transcription_state.engine().await;

        // Create/load engine if needed
        if engine_opt.is_none() {
            let mut engine = TranscriptionEngine::new();

            // Try to find and load a model
            let model_path = {
                let manager = ModelManager::new().ok();
                manager.and_then(|m| {
                    let settings = crate::conf::Settings::load();

                    if let Some(pref) = settings.preferred_model {
                        if let Some(path) = m.get_model_path(pref) {
                            return Some(path);
                        }
                    }

                    // Fallback order: parakeet-v3, then whisper-base
                    for candidate in [
                        ModelId::Parakeet(ParakeetModel::V3),
                        ModelId::Whisper(WhisperModel::Base),
                    ] {
                        if let Some(path) = m.get_model_path(candidate) {
                            return Some(path);
                        }
                    }

                    None
                })
            };

            if let Some(path) = model_path {
                eprintln!("[transcribe] Loading model from: {}", path.display());
                match engine.load_model(&path.to_string_lossy()) {
                    Ok(_) => eprintln!("[transcribe] Model loaded successfully"),
                    Err(e) => eprintln!("[transcribe] Failed to load model: {}", e),
                }
            } else {
                eprintln!("[transcribe] No model found, transcription will fail");
            }

            *engine_opt = Some(engine);
        }

        let engine = engine_opt.as_mut().unwrap();
        engine
            .transcribe_file(&audio_path)
            .map_err(|e| e.to_string())?
    };

    eprintln!("[stop_and_transcribe] Transcription: {}", text);

    // Save transcription to database (only if non-empty)
    if !text.trim().is_empty() {
        if let Some(database) = db {
            let current_output_mode = settings.get().await.output_mode;

            let output_mode_str = match current_output_mode {
                OutputMode::Print => "print",
                OutputMode::Copy => "copy",
                OutputMode::Insert => "insert",
            }
            .to_string();

            let model_name = {
                let engine_opt = transcription_state.engine().await;
                engine_opt
                    .as_ref()
                    .and_then(|e| e.get_model_path())
                    .map(|p| p.to_string())
            };

            let audio_size = std::fs::metadata(&audio_path).ok().map(|m| m.len() as i64);

            let mut new_transcription = NewTranscription::new(text.clone())
                .with_output_mode(output_mode_str)
                .with_duration(duration_ms)
                .with_audio_path(audio_path.to_string_lossy().to_string());

            if let Some(model) = model_name {
                new_transcription = new_transcription.with_model(model);
            }

            if let Some(size) = audio_size {
                new_transcription = new_transcription.with_audio_size(size);
            }

            match crate::db::transcriptions::save(database.pool(), new_transcription).await {
                Ok(id) => {
                    eprintln!("[stop_and_transcribe] Saved transcription with ID: {}", id);
                }
                Err(e) => {
                    eprintln!("[stop_and_transcribe] Failed to save transcription: {}", e);
                }
            }
        } else {
            eprintln!("[stop_and_transcribe] Database not initialized, skipping save");
        }
    } else {
        eprintln!("[stop_and_transcribe] Skipping save: transcription is empty");
    }

    // Broadcast result to iced OSD
    broadcast
        .transcription_result(
            text.clone(),
            duration_ms as f32 / 1000.0,
            "parakeet-v3".into(),
        )
        .await;

    // Handle output based on configured mode
    let output_mode = settings.get().await.output_mode;

    let text_inserter = crate::text::TextInserter::new();

    match output_mode {
        OutputMode::Print => {
            println!("{}", text);
        }
        OutputMode::Copy => {
            match text_inserter.copy_to_clipboard(app, &text) {
                Ok(()) => {
                    eprintln!("[stop_and_transcribe] Text copied to clipboard");
                }
                Err(e) => {
                    eprintln!("[stop_and_transcribe] Failed to copy to clipboard: {}", e);
                    println!("{}", text); // Fallback to print
                }
            }
        }
        OutputMode::Insert => {
            match text_inserter.insert_text(&text) {
                Ok(()) => {
                    eprintln!("[stop_and_transcribe] Text inserted at cursor");
                }
                Err(e) => {
                    eprintln!("[stop_and_transcribe] Failed to insert text: {}", e);
                    println!("{}", text); // Fallback to print
                }
            }
        }
    }

    // Back to idle
    recording.finish_transcription().await;

    // Broadcast idle state
    broadcast
        .recording_status(RecordingSnapshot::Idle, None, false, 0)
        .await;

    Ok(())
}
