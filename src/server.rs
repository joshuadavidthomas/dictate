//! Socket server for handling transcription requests
//!
//! This module provides the main server that listens for client connections
//! and coordinates transcription requests.

use crate::audio::{AudioRecorder, buffer_to_wav};
use crate::get_recording_path;
use crate::models::ModelManager;
use crate::protocol::{ClientMessage, ServerMessage, State};
use crate::transcription::TranscriptionEngine;
use crate::transport::{AsyncConnection, SocketError, encode_server_message};
use cpal::traits::StreamTrait;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, Notify, RwLock};

// Server result type using SocketError for structured error handling
type ServerResult<T> = std::result::Result<T, SocketError>;

pub struct SocketServer {
    inner: Arc<ServerInner>,
    listener: UnixListener,
}

impl SocketServer {
    pub fn new<P: AsRef<Path>>(
        socket_path: P,
        model_name: &str,
        idle_timeout_secs: u64,
    ) -> ServerResult<Self> {
        // Remove existing socket file if it exists
        if socket_path.as_ref().exists() {
            std::fs::remove_file(socket_path.as_ref())?;
        }

        let listener = UnixListener::bind(&socket_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                SocketError::Connection(format!(
                    "Service already running at socket: {}. Use 'systemctl --user stop dictate.service' to stop it first.",
                    socket_path.as_ref().display()
                ))
            } else {
                SocketError::Connection(format!("Failed to bind socket: {}", e))
            }
        })?;

        // Set socket permissions to 0600 (owner read/write only) for security
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&socket_path)
                .map_err(|e| {
                    SocketError::Connection(format!("Failed to read socket metadata: {}", e))
                })?
                .permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&socket_path, permissions).map_err(|e| {
                SocketError::Connection(format!("Failed to set socket permissions: {}", e))
            })?;
        }

        let model_manager = ModelManager::new().map_err(|e| {
            SocketError::Connection(format!("Failed to create model manager: {}", e))
        })?;

        // Get the model path
        let model_path = model_manager
            .get_model_path(model_name)
            .ok_or_else(|| {
                SocketError::Connection(format!(
                    "Model '{}' not found. Download it with: dictate models download {}",
                    model_name, model_name
                ))
            })?
            .to_string_lossy()
            .to_string();

        // Preload the model at startup
        eprintln!("Preloading model: {}", model_path);
        let mut engine = TranscriptionEngine::new();
        engine
            .load_model(&model_path)
            .map_err(|e| SocketError::Connection(format!("Failed to preload model: {}", e)))?;
        eprintln!("Model loaded successfully");

        let now = Instant::now();
        let idle_timeout = Duration::from_secs(idle_timeout_secs);

        let inner = Arc::new(ServerInner::new(
            engine,
            model_manager,
            now,
            idle_timeout,
            model_name.to_string(),
            socket_path.as_ref().to_string_lossy().to_string(),
        ));

        Ok(Self { inner, listener })
    }

    pub async fn run(&mut self) -> ServerResult<()> {
        println!("Socket server listening for connections...");
        if self.inner.idle_timeout.as_secs() == 0 {
            println!("Idle timeout disabled - model will stay loaded");
        } else {
            println!(
                "Idle timeout set to {} seconds",
                self.inner.idle_timeout.as_secs()
            );
        }

        let idle_monitor = tokio::spawn(Self::idle_monitor(Arc::clone(&self.inner)));
        let heartbeat = tokio::spawn(Self::heartbeat_monitor(Arc::clone(&self.inner)));

        let shutdown_notify = Arc::clone(&self.inner);

        tokio::select! {
            _ = shutdown_notify.shutdown_notify.notified() => {
                println!("Shutdown signal received, stopping server...");
                idle_monitor.abort();
                heartbeat.abort();
                self.cleanup().await?;
                Ok(())
            }
            result = self.accept_loop() => {
                idle_monitor.abort();
                heartbeat.abort();
                result
            }
        }
    }

    async fn idle_monitor(inner: Arc<ServerInner>) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            // Skip idle monitoring if timeout is 0 (disabled)
            if inner.idle_timeout.as_secs() == 0 {
                continue;
            }

            let idle_time = inner.get_idle_time().await;
            if idle_time <= inner.idle_timeout {
                continue;
            }

            let mut engine = inner.transcription_engine.write().await;

            if engine.is_model_loaded() {
                println!(
                    "Idle timeout reached ({} seconds), unloading model",
                    idle_time.as_secs()
                );
                engine.unload_model();
            }
        }
    }

    async fn heartbeat_monitor(inner: Arc<ServerInner>) {
        loop {
            // Simple 2-second keep-alive heartbeat
            // No throttling - spectrum updates broadcast immediately from spectrum task
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Broadcast current status (without spectrum - that comes from spectrum task)
            inner.broadcast_status().await;
        }
    }

    async fn accept_loop(&mut self) -> ServerResult<()> {
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    let inner = Arc::clone(&self.inner);
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, inner).await {
                            eprintln!("Error handling connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    async fn cleanup(&self) -> ServerResult<()> {
        let addr = self.listener.local_addr().ok();

        if let Some(path) = addr
            .as_ref()
            .and_then(|a| a.as_pathname())
            .filter(|p| p.exists())
            && let Err(e) = std::fs::remove_file(path)
        {
            eprintln!("Failed to remove socket file: {}", e);
        }
        Ok(())
    }
}

struct SubscriberHandle {
    id: String,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

/// Inner server state with all shared data
struct ServerInner {
    transcription_engine: RwLock<TranscriptionEngine>,
    model_manager: RwLock<ModelManager>,
    last_activity: Mutex<Instant>,
    subscribers: Mutex<Vec<SubscriberHandle>>,
    current_state: Mutex<State>, // Track current state for heartbeat
    last_spectrum: Mutex<Option<Vec<f32>>>, // Track last spectrum for heartbeat
    recording_stop_signal: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
    start_time: Instant,
    idle_timeout: Duration,
    model_name: String,
    socket_path: String,
    shutdown_notify: Notify,
}

impl ServerInner {
    fn new(
        transcription_engine: TranscriptionEngine,
        model_manager: ModelManager,
        start_time: Instant,
        idle_timeout: Duration,
        model_name: String,
        socket_path: String,
    ) -> Self {
        Self {
            transcription_engine: RwLock::new(transcription_engine),
            model_manager: RwLock::new(model_manager),
            last_activity: Mutex::new(start_time),
            subscribers: Mutex::new(Vec::new()),
            current_state: Mutex::new(State::Idle), // Start in Idle state
            last_spectrum: Mutex::new(None),
            recording_stop_signal: Mutex::new(None),
            start_time,
            idle_timeout,
            model_name,
            socket_path,
            shutdown_notify: Notify::new(),
        }
    }

    /// Get monotonic timestamp in milliseconds since server start
    fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Broadcast an event to all subscribers
    async fn broadcast_event(&self, event: ServerMessage) {
        let event_json = encode_server_message(&event).unwrap();
        let bytes = event_json.into_bytes();

        let mut subs = self.subscribers.lock().await;
        subs.retain(|sub| {
            // Try to send, remove if channel is closed
            sub.tx.send(bytes.clone()).is_ok()
        });
    }

    /// Update last activity time
    async fn update_activity(&self) {
        let mut last = self.last_activity.lock().await;
        *last = Instant::now();
    }

    /// Update current state (for heartbeat tracking)
    async fn set_current_state(&self, state: State) {
        let mut current = self.current_state.lock().await;
        *current = state;
    }

    /// Get current state (for heartbeat broadcasting)
    async fn get_current_state(&self) -> State {
        *self.current_state.lock().await
    }

    /// Get current idle time
    async fn get_idle_time(&self) -> Duration {
        let last = self.last_activity.lock().await;
        last.elapsed()
    }

    /// Check if model is loaded (hot idle vs cold idle)
    async fn get_idle_hot(&self) -> bool {
        let engine = self.transcription_engine.read().await;
        engine.is_model_loaded()
    }

    /// Update spectrum data
    async fn update_spectrum(&self, bands: Vec<f32>) {
        let mut spectrum = self.last_spectrum.lock().await;
        *spectrum = Some(bands);
    }

    /// Get last spectrum data
    async fn get_last_spectrum(&self) -> Option<Vec<f32>> {
        let spectrum = self.last_spectrum.lock().await;
        spectrum.clone()
    }

    /// Clear spectrum data (when not recording)
    async fn clear_spectrum(&self) {
        let mut spectrum = self.last_spectrum.lock().await;
        *spectrum = None;
    }

    /// Broadcast unified status event with current state
    async fn broadcast_status(&self) {
        let state = self.get_current_state().await;
        let spectrum = self.get_last_spectrum().await;
        let idle_hot = self.get_idle_hot().await;
        let ts = self.elapsed_ms();

        self.broadcast_event(ServerMessage::new_status_event(
            state, spectrum, idle_hot, ts,
        ))
        .await;
    }

    /// Execute operation with transcription engine
    async fn with_transcription_engine<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut TranscriptionEngine) -> Result<R, String>,
    {
        let mut engine = self.transcription_engine.write().await;
        f(&mut engine)
    }

    /// Execute operation with model manager
    async fn with_model_manager<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut ModelManager) -> Result<R, String>,
    {
        let mut manager = self.model_manager.write().await;
        f(&mut manager)
    }

    /// Get server status fields
    async fn get_status(&self) -> (bool, bool, String, String, u64, u64, State) {
        let uptime = self.start_time.elapsed().as_secs();
        let idle_time = self.get_idle_time().await.as_secs();
        let current_state = self.get_current_state().await;

        let engine = self.transcription_engine.read().await;
        let model_loaded = engine.is_model_loaded();
        let model_path = engine
            .get_model_path()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        (
            true,                  // service_running
            model_loaded,          // model_loaded
            model_path,            // model_path
            "default".to_string(), // audio_device
            uptime,                // uptime_seconds
            idle_time,             // last_activity_seconds_ago
            current_state,         // state
        )
    }

    /// Set recording stop signal (for Start command)
    async fn set_recording_stop_signal(&self, signal: Arc<std::sync::atomic::AtomicBool>) {
        let mut stop_signal = self.recording_stop_signal.lock().await;
        *stop_signal = Some(signal);
    }

    /// Clear recording stop signal (when recording completes)
    async fn clear_recording_stop_signal(&self) {
        let mut stop_signal = self.recording_stop_signal.lock().await;
        *stop_signal = None;
    }

    /// Get recording stop signal if recording is active
    async fn get_recording_stop_signal(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        let stop_signal = self.recording_stop_signal.lock().await;
        stop_signal.clone()
    }

    /// Check if a recording is currently active
    async fn is_recording_active(&self) -> bool {
        let stop_signal = self.recording_stop_signal.lock().await;
        stop_signal.is_some()
    }
}

async fn handle_connection(
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

                                // Send initial status event with current state (don't change state!)
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
                                        Some(silence_duration),
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
                            ClientMessage::Start { id, max_duration, silence_duration, sample_rate, insert, copy } => {
                                // Check if already recording
                                if inner.is_recording_active().await {
                                    eprintln!("Start request ignored - recording already in progress");
                                    continue; // No-op
                                }

                                // Spawn UI in background thread (OBSERVER MODE)
                                let socket_path_for_ui = inner.socket_path.clone();
                                std::thread::spawn(move || {
                                    let config = crate::ui::TranscriptionConfig {
                                        max_duration,  // Still pass config for display purposes
                                        silence_duration: silence_duration.unwrap_or(0),
                                        sample_rate,
                                        insert,
                                        copy,
                                    };

                                    // Use Observer mode - UI won't send commands, just displays
                                    if let Err(e) = crate::ui::run_osd_observer(&socket_path_for_ui, config) {
                                        eprintln!("UI error: {}", e);
                                    }
                                });

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
                            ClientMessage::Stop { id: _ } => {
                                // Check if recording is active
                                if let Some(stop_signal) = inner.get_recording_stop_signal().await {
                                    eprintln!("Stop request received - stopping active recording");
                                    stop_signal.store(true, Ordering::Release);
                                    // The recording task will handle cleanup and send result
                                    // No response needed here - the Start task will send the result
                                } else {
                                    eprintln!("Stop request ignored - no active recording");
                                    // No-op - no response needed
                                }
                            }
                            ClientMessage::Status { id } => {
                                let (
                                    service_running,
                                    model_loaded,
                                    model_path,
                                    audio_device,
                                    uptime_seconds,
                                    last_activity_seconds_ago,
                                    state,
                                ) = inner.get_status().await;
                                let response = ServerMessage::new_status(
                                    id,
                                    service_running,
                                    model_loaded,
                                    model_path,
                                    audio_device,
                                    uptime_seconds,
                                    last_activity_seconds_ago,
                                    state,
                                );
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
    silence_duration: Option<u64>,
    _sample_rate: u32,
    inner: Arc<ServerInner>,
) -> ServerMessage {
    // Update and broadcast Recording state
    inner
        .set_current_state(State::Recording)
        .await;
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

    // Store stop signal in server state so Stop command can access it
    inner.set_recording_stop_signal(stop_signal.clone()).await;

    let silence_detector = silence_duration.map(|duration| {
        crate::audio::SilenceDetector::new(0.01, Duration::from_secs(duration))
    });

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
    // Note: max_duration of 0 means unlimited - rely entirely on silence detection or manual stop
    let max_duration_time = if max_duration == 0 {
        None
    } else {
        Some(Duration::from_secs(max_duration))
    };
    
    loop {
        if stop_signal.load(Ordering::Acquire) {
            println!("Recording stopped due to silence detection or manual stop");
            break;
        }

        // Check max duration only if it's set (not unlimited)
        if let Some(max_dur) = max_duration_time {
            if start_time.elapsed() >= max_dur {
                println!("Recording stopped due to max duration");
                stop_signal.store(true, Ordering::Release);
                break;
            }
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
    inner
        .set_current_state(State::Transcribing)
        .await;
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

    // TODO: Future enhancement - implement chunking with silence detection
    // - Split long recordings into chunks at silence boundaries
    // - Transcribe each chunk progressively
    // - Insert paragraph breaks where significant silence gaps are detected
    // - This would enable streaming transcription and better formatting for long recordings
    
    // Transcribe using preloaded model
    // First check if we need to reload the model
    let model_loaded = inner
        .with_transcription_engine(|engine| Ok(engine.is_model_loaded()))
        .await
        .unwrap_or(false);

    if !model_loaded {
        println!("Model was unloaded, reloading...");
        // Get the model path from the model manager
        let model_path_result = inner
            .with_model_manager(|manager| {
                manager
                    .get_model_path(&inner.model_name)
                    .ok_or_else(|| format!("Model '{}' not found", &inner.model_name))
                    .map(|p| p.to_string_lossy().to_string())
            })
            .await;

        match model_path_result {
            Ok(model_path) => {
                let reload_result = inner
                    .with_transcription_engine(|engine| {
                        engine
                            .load_model(&model_path)
                            .map_err(|e| format!("Failed to reload model: {}", e))
                    })
                    .await;

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
    let response = match inner
        .with_transcription_engine(|engine| match engine.transcribe_file(&recording_path) {
            Ok(text) => Ok((
                text,
                engine.get_model_path().unwrap_or("unknown").to_string(),
            )),
            Err(e) => Err(format!("Transcription failed: {}", e)),
        })
        .await
    {
        Ok((text, model_path)) => {
            ServerMessage::new_result(id, text, duration.as_secs_f32(), model_path)
        }
        Err(e) => ServerMessage::Error { id, error: e },
    };

    // Update and broadcast Idle state (transcription complete)
    inner.set_current_state(State::Idle).await;
    inner.clear_spectrum().await; // No spectrum when idle
    inner.clear_recording_stop_signal().await; // Clear stop signal
    inner.broadcast_status().await;

    response
}
