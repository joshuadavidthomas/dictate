//! Connection and message handling
//!
//! This module handles individual client connections and message processing.

use crate::audio::{AudioRecorder, buffer_to_wav};
use crate::get_recording_path;
use crate::protocol::{ClientMessage, ServerMessage};
use crate::transport::{AsyncConnection, SocketError};
use cpal::traits::StreamTrait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::ServerInner;
use super::SubscriberHandle;

type ServerResult<T> = std::result::Result<T, SocketError>;

pub(super) async fn handle_connection(
    stream: UnixStream,
    inner: Arc<ServerInner>,
) -> ServerResult<()> {
    inner.update_activity().await;

    // Convert UnixStream to AsyncConnection for line-delimited reading
    let (reader, writer) = stream.into_split();
    let mut conn = AsyncConnection {
        reader: tokio::io::BufReader::new(reader),
        writer,
    };

    // Track if this connection is a subscriber
    let mut subscriber_id: Option<String> = None;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

    loop {
        tokio::select! {
            // Read from client
            result = conn.read_client_message() => {
                match result? {
                    None => {
                        // Connection closed
                        break;
                    }
                    Some(request) => {
                        println!("Received request: {:?}", request);

                        match request {
                            ClientMessage::Subscribe { id } => {
                                // Add to subscribers
                                subscriber_id = Some(id.to_string());
                                let mut subs = inner.subscribers.lock().await;
                                subs.push(SubscriberHandle {
                                    id: id.to_string(),
                                    tx: event_tx.clone(),
                                });
                                drop(subs);

                                // Send initial status event (set state to Idle)
                                inner.set_current_state(crate::protocol::State::Idle).await;
                                inner.broadcast_status().await;

                                // Send acknowledgment
                                let response = ServerMessage::new_subscribed(id);
                                conn.write_server_message(&response).await?;
                            }
                            ClientMessage::Transcribe { id, max_duration, silence_duration, sample_rate } => {
                                // Spawn transcribe task in background so events can continue flowing
                                let inner_clone = Arc::clone(&inner);
                                let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel();

                                tokio::spawn(async move {
                                    let response = handle_transcribe_request(
                                        id,
                                        max_duration,
                                        silence_duration,
                                        sample_rate,
                                        inner_clone,
                                    ).await;
                                    let _ = result_tx.send(response);
                                });

                                // Wait for result in a nested select loop so events can flow
                                loop {
                                    tokio::select! {
                                        Some(response) = result_rx.recv() => {
                                            // Send transcription result
                                            conn.write_server_message(&response).await?;
                                            break;
                                        }

                                        // Continue processing events while waiting for result
                                        Some(event_data) = event_rx.recv() => {
                                            conn.writer.write_all(&event_data).await?;
                                            conn.writer.flush().await?;
                                        }
                                    }
                                }
                            }
                            _ => {
                                // Regular request-response (Status, etc.)
                                let response = process_message(request, Arc::clone(&inner)).await;
                                conn.write_server_message(&response).await?;
                            }
                        }
                    }
                }
            }

            // Send events to subscriber
            Some(event_data) = event_rx.recv() => {
                conn.writer.write_all(&event_data).await?;
                conn.writer.flush().await?;
            }
        }
    }

    // Clean up subscriber on disconnect
    if let Some(id) = subscriber_id {
        let mut subs = inner.subscribers.lock().await;
        subs.retain(|s| s.id != id);
    }

    Ok(())
}

/// Handle transcribe request in background task
async fn handle_transcribe_request(
    id: uuid::Uuid,
    max_duration: u64,
    silence_duration: u64,
    _sample_rate: u32,
    inner: Arc<ServerInner>,
) -> ServerMessage {
    // Update and broadcast Recording state
    inner.set_current_state(crate::protocol::State::Recording).await;
    inner.clear_spectrum().await; // Reset spectrum for new recording
    inner.broadcast_status().await;

    let recorder = match AudioRecorder::new() {
        Ok(recorder) => recorder,
        Err(e) => {
            return ServerMessage::Error {
                id,
                error: format!("Failed to create audio recorder: {}", e),
            };
        }
    };

    let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = Arc::new(AtomicBool::new(false));

    let silence_detector = Some(crate::audio::SilenceDetector::new(
        0.01,
        Duration::from_secs(silence_duration),
    ));

    // Create spectrum channel for OSD updates
    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();

    // Spawn task to broadcast spectrum updates immediately as they arrive
    // No throttling - UI will handle its own consumption rate via render loop
    let inner_clone = Arc::clone(&inner);
    tokio::spawn(async move {
        while let Some(bands) = spectrum_rx.recv().await {
            inner_clone.update_spectrum(bands).await;
            inner_clone.broadcast_status().await; // Broadcast immediately with spectrum data
        }
    });

    let stream = match recorder.start_recording_background(
        audio_buffer.clone(),
        stop_signal.clone(),
        silence_detector,
        Some(spectrum_tx),
    ) {
        Ok(stream) => stream,
        Err(e) => {
            return ServerMessage::Error {
                id,
                error: format!("Failed to start recording: {}", e),
            };
        }
    };

    // Start the stream
    if let Err(e) = stream.play() {
        return ServerMessage::Error {
            id,
            error: format!("Failed to start audio stream: {}", e),
        };
    }

    let start_time = Instant::now();

    // Wait for stop signal (from silence detection) or max duration
    let max_duration_time = Duration::from_secs(max_duration);
    loop {
        if stop_signal.load(Ordering::Acquire) {
            println!("Recording stopped due to silence detection");
            break;
        }

        if start_time.elapsed() >= max_duration_time {
            println!("Recording stopped due to max duration");
            stop_signal.store(true, Ordering::Release);
            break;
        }

        // Check every 100ms
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Drop stream to stop recording
    drop(stream);

    // Small delay to ensure last samples are written
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Broadcast Transcribing state immediately after recording stops
    // This fills the gap between recording stopping and transcription starting
    inner.set_current_state(crate::protocol::State::Transcribing).await;
    inner.clear_spectrum().await; // No spectrum during transcription
    inner.broadcast_status().await;

    // Get audio buffer
    let buffer = match audio_buffer.lock() {
        Ok(buffer) => buffer.clone(),
        Err(e) => {
            return ServerMessage::Error {
                id,
                error: format!("Failed to access audio buffer: {}", e),
            };
        }
    };

    let duration = start_time.elapsed();

    if buffer.is_empty() {
        return ServerMessage::Error {
            id,
            error: "No audio recorded".to_string(),
        };
    }

    // Write buffer to recording file in app data directory
    let recording_path = match get_recording_path() {
        Ok(path) => path,
        Err(e) => {
            return ServerMessage::Error {
                id,
                error: format!("Failed to get recording path: {}", e),
            };
        }
    };

    if let Err(e) = buffer_to_wav(&buffer, &recording_path, 16000) {
        return ServerMessage::Error {
            id,
            error: format!("Failed to write audio file: {}", e),
        };
    }

    // Delay to ensure Transcribing state is visible in OSD
    // even for very fast transcriptions (500ms gives UI time to render)
    // Note: Transcribing state was already broadcast right after recording stopped
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Transcribe using preloaded model
    // First check if we need to reload the model
    let model_loaded = inner
        .with_transcription_engine(|engine| Ok(engine.is_model_loaded()))
        .await
        .unwrap_or(false);

    if !model_loaded {
        println!("Model was unloaded, reloading...");
        // Get the model path from the model manager
        let model_path_result = inner.with_model_manager(|manager| {
            manager
                .get_model_path(&inner.model_name)
                .ok_or_else(|| format!("Model '{}' not found", &inner.model_name))
                .map(|p| p.to_string_lossy().to_string())
        }).await;

        match model_path_result {
            Ok(model_path) => {
                let reload_result = inner.with_transcription_engine(|engine| {
                    engine
                        .load_model(&model_path)
                        .map_err(|e| format!("Failed to reload model: {}", e))
                }).await;

                match reload_result {
                    Ok(_) => println!("Model reloaded successfully"),
                    Err(e) => {
                        return ServerMessage::Error { id, error: e };
                    }
                }
            }
            Err(e) => {
                return ServerMessage::Error {
                    id,
                    error: format!("Failed to get model path: {}", e),
                };
            }
        }
    }

    // Now transcribe
    let response = match inner.with_transcription_engine(|engine| {
        match engine.transcribe_file(&recording_path) {
            Ok(text) => Ok((
                text,
                engine.get_model_path().unwrap_or("unknown").to_string(),
            )),
            Err(e) => Err(format!("Transcription failed: {}", e)),
        }
    }).await {
        Ok((text, model_path)) => {
            ServerMessage::new_result(id, text, duration.as_secs_f32(), model_path)
        }
        Err(e) => ServerMessage::Error { id, error: e },
    };

    // Update and broadcast Idle state (transcription complete)
    inner.set_current_state(crate::protocol::State::Idle).await;
    inner.clear_spectrum().await; // No spectrum when idle
    inner.broadcast_status().await;

    response
}

async fn process_message(request: ClientMessage, inner: Arc<ServerInner>) -> ServerMessage {
    match request {
        ClientMessage::Transcribe { id, .. } => {
            // Transcribe requests are now handled directly in handle_connection
            // This shouldn't be reached
            ServerMessage::Error {
                id,
                error: "Transcribe requests should be handled in background task".to_string(),
            }
        }

        ClientMessage::Status { id } => {
            let (service_running, model_loaded, model_path, audio_device, uptime_seconds, last_activity_seconds_ago) 
                = inner.get_status().await;
            ServerMessage::new_status(
                id,
                service_running,
                model_loaded,
                model_path,
                audio_device,
                uptime_seconds,
                last_activity_seconds_ago,
            )
        }

        ClientMessage::Subscribe { id } => {
            // This should never be reached as Subscribe is handled in handle_connection
            ServerMessage::Error {
                id,
                error: "Subscribe should be handled at connection level".to_string(),
            }
        }
    }
}
