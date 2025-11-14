use tauri::{AppHandle, Emitter, Manager, State};
use crate::state::{AppState, RecordingState, ActiveRecording, OutputMode};
use crate::audio::{AudioRecorder, buffer_to_wav};
use crate::transcription::TranscriptionEngine;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use cpal::traits::StreamTrait;

#[derive(Clone, Serialize)]
struct StatusUpdate {
    state: String,
}

#[derive(Clone, Serialize)]
struct TranscriptionResult {
    text: String,
}

#[tauri::command]
pub async fn toggle_recording(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let mut rec_state = state.recording_state.lock().await;
    
    match *rec_state {
        RecordingState::Idle => {
            eprintln!("[toggle_recording] Starting recording");
            *rec_state = RecordingState::Recording;
            drop(rec_state);
            
            // Emit event to frontend
            app.emit("recording-started", StatusUpdate { 
                state: "recording".into() 
            }).ok();
            
            // Broadcast to iced OSD
            state.broadcast.broadcast_status(
                crate::protocol::State::Recording,
                None,
                state.elapsed_ms()
            ).await;
            
            // Start actual recording
            let app_clone = app.clone();
            tokio::spawn(async move {
                let state: tauri::State<AppState> = app_clone.state();
                if let Err(e) = start_recording(&state, &app_clone).await {
                    eprintln!("[toggle_recording] Failed to start recording: {}", e);
                }
            });
            
            Ok("started".into())
        }
        RecordingState::Recording => {
            eprintln!("[toggle_recording] Stopping recording");
            *rec_state = RecordingState::Transcribing;
            drop(rec_state);
            
            // Emit event to frontend
            app.emit("recording-stopped", StatusUpdate { 
                state: "transcribing".into() 
            }).ok();
            
            // Broadcast to iced OSD
            state.broadcast.broadcast_status(
                crate::protocol::State::Transcribing,
                None,
                state.elapsed_ms()
            ).await;
            
            // Stop recording and transcribe
            let app_handle = app.clone();
            tokio::spawn(async move {
                if let Err(e) = stop_and_transcribe(app_handle).await {
                    eprintln!("[toggle_recording] Failed to transcribe: {}", e);
                }
            });
            
            Ok("stopping".into())
        }
        RecordingState::Transcribing => {
            eprintln!("[toggle_recording] Already transcribing");
            Ok("busy".into())
        }
    }
}

async fn start_recording(state: &AppState, app: &AppHandle) -> Result<(), String> {
    // Create recorder if needed
    {
        let mut rec_opt = state.recorder.lock().await;
        if rec_opt.is_none() {
            *rec_opt = Some(AudioRecorder::new().map_err(|e| e.to_string())?);
        }
    }
    
    let recorder = {
        let _rec_opt = state.recorder.lock().await;
        // AudioRecorder doesn't need to be cloned, we'll use it through the lock
        AudioRecorder::new().map_err(|e| e.to_string())?
    };
    
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
    
    // Spawn task to broadcast spectrum data
    let broadcast = state.broadcast.clone();
    let start_time = state.start_time;
    tokio::spawn(async move {
        while let Some(spectrum) = spectrum_rx.recv().await {
            let ts = start_time.elapsed().as_millis() as u64;
            
            // Broadcast to iced OSD
            broadcast.broadcast_status(
                crate::protocol::State::Recording,
                Some(spectrum),
                ts
            ).await;
        }
    });
    
    // Store the active recording
    let mut current = state.current_recording.lock().await;
    *current = Some(ActiveRecording {
        audio_buffer,
        stop_signal,
        stream: Some(stream),
    });
    
    eprintln!("[start_recording] Recording started successfully");
    Ok(())
}

async fn stop_and_transcribe(app: AppHandle) -> Result<(), String> {
    let state: tauri::State<AppState> = app.state();
    
    // Stop the recording
    let audio_buffer = {
        let mut current = state.current_recording.lock().await;
        if let Some(mut recording) = current.take() {
            // Signal stop
            recording.stop_signal.store(true, Ordering::Release);
            
            // Drop stream to stop recording
            recording.stream.take();
            
            // Small delay to ensure last samples are written
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            
            recording.audio_buffer
        } else {
            return Err("No active recording".into());
        }
    };
    
    // Get the recorded audio
    let buffer = audio_buffer.lock().unwrap().clone();
    
    if buffer.is_empty() {
        eprintln!("[stop_and_transcribe] No audio recorded");
        
        let mut rec_state = state.recording_state.lock().await;
        *rec_state = RecordingState::Idle;
        
        app.emit("transcription-complete", StatusUpdate {
            state: "idle".into()
        }).ok();
        
        return Err("No audio recorded".into());
    }
    
    eprintln!("[stop_and_transcribe] Recorded {} samples", buffer.len());
    
    // Write to temp file
    let temp_path = std::env::temp_dir().join("dictate_recording.wav");
    buffer_to_wav(&buffer, &temp_path, 16000).map_err(|e| e.to_string())?;
    
    eprintln!("[stop_and_transcribe] Audio saved to: {:?}", temp_path);
    
    // Transcribe
    let text = {
        let mut engine_opt = state.engine.lock().await;
        
        // Create/load engine if needed
        if engine_opt.is_none() {
            let mut engine = TranscriptionEngine::new();
            
            // Try to find and load a model
            let model_path = {
                use crate::models::ModelManager;
                let manager = ModelManager::new().ok();
                manager.and_then(|m| {
                    // Try parakeet-v3 first, then whisper-base
                    m.get_model_path("parakeet-v3")
                        .or_else(|| m.get_model_path("whisper-base"))
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
        engine.transcribe_file(&temp_path).map_err(|e| e.to_string())?
    };
    
    eprintln!("[stop_and_transcribe] Transcription: {}", text);
    
    // Emit result to Svelte UI
    app.emit("transcription-result", TranscriptionResult {
        text: text.clone(),
    }).ok();
    
    // Broadcast result to iced OSD
    state.broadcast.broadcast_result(text.clone()).await;
    
    // Handle output based on configured mode
    let output_mode = *state.output_mode.lock().await;
    let text_inserter = crate::text::TextInserter::new();
    
    match output_mode {
        OutputMode::Print => {
            println!("{}", text);
        }
        OutputMode::Copy => {
            match text_inserter.copy_to_clipboard(&text) {
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
    let mut rec_state = state.recording_state.lock().await;
    *rec_state = RecordingState::Idle;
    
    app.emit("transcription-complete", StatusUpdate {
        state: "idle".into()
    }).ok();
    
    // Broadcast idle state
    state.broadcast.broadcast_status(
        crate::protocol::State::Idle,
        None,
        state.elapsed_ms()
    ).await;
    
    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<String, String> {
    let rec_state = state.recording_state.lock().await;
    let status = match *rec_state {
        RecordingState::Idle => "idle",
        RecordingState::Recording => "recording",
        RecordingState::Transcribing => "transcribing",
    };
    Ok(status.to_string())
}

#[tauri::command]
pub async fn set_output_mode(
    state: State<'_, AppState>,
    mode: String,
) -> Result<String, String> {
    let output_mode = match mode.as_str() {
        "print" => OutputMode::Print,
        "copy" => OutputMode::Copy,
        "insert" => OutputMode::Insert,
        _ => return Err(format!("Invalid mode: {}", mode)),
    };
    
    *state.output_mode.lock().await = output_mode;
    eprintln!("[set_output_mode] Output mode set to: {:?}", output_mode);
    Ok(format!("Output mode set to: {}", mode))
}

#[tauri::command]
pub async fn get_output_mode(state: State<'_, AppState>) -> Result<String, String> {
    let mode = *state.output_mode.lock().await;
    let mode_str = match mode {
        OutputMode::Print => "print",
        OutputMode::Copy => "copy",
        OutputMode::Insert => "insert",
    };
    Ok(mode_str.to_string())
}

#[tauri::command]
pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
