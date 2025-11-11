use crate::audio::AudioRecorder;
use cpal::traits::StreamTrait;

use crate::models::ModelManager;
use crate::socket::{Message, Response, SocketError};
use crate::transcription::TranscriptionEngine;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

// Server-specific result type that still uses SocketError for compatibility
type ServerResult<T> = std::result::Result<T, SocketError>;

/// Active recording session for push-to-talk
struct RecordingSession {
    audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
    start_time: Instant,
    #[allow(dead_code)]
    stream: cpal::Stream,  // Must keep alive to continue recording
    stop_signal: Arc<AtomicBool>,
}

/// Inner server state with all shared data
struct ServerInner {
    // Shared mutable state
    transcription_engine: std::sync::Mutex<TranscriptionEngine>,
    model_manager: std::sync::Mutex<ModelManager>,
    last_activity: std::sync::Mutex<Instant>,
    recording_session: std::sync::Mutex<Option<RecordingSession>>,
    
    // Shared immutable state
    start_time: Instant,
    idle_timeout: Duration,
    
    // Async coordination
    shutdown_notify: Notify,
}

impl ServerInner {
    fn new(
        transcription_engine: TranscriptionEngine,
        model_manager: ModelManager,
        start_time: Instant,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            transcription_engine: std::sync::Mutex::new(transcription_engine),
            model_manager: std::sync::Mutex::new(model_manager),
            last_activity: std::sync::Mutex::new(start_time),
            recording_session: std::sync::Mutex::new(None),
            start_time,
            idle_timeout,
            shutdown_notify: Notify::new(),
        }
    }
    
    /// Update last activity time
    fn update_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }
    
    /// Get current idle time
    fn get_idle_time(&self) -> Duration {
        self.last_activity
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or_default()
    }
    
    /// Start a new recording session
    fn start_recording(&self, session: RecordingSession) -> Result<(), String> {
        let mut guard = self.recording_session
            .lock()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        
        if guard.is_some() {
            return Err("Recording already in progress".to_string());
        }
        
        *guard = Some(session);
        Ok(())
    }
    
    /// Stop current recording session
    fn stop_recording(&self) -> Result<RecordingSession, String> {
        let mut guard = self.recording_session
            .lock()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        
        guard.take()
            .ok_or_else(|| "No recording in progress".to_string())
    }
    
    /// Check if recording is active
    fn is_recording(&self) -> bool {
        self.recording_session
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
    
    /// Execute operation with transcription engine
    fn with_transcription_engine<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut TranscriptionEngine) -> Result<R, String>,
    {
        let mut engine = self.transcription_engine
            .lock()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        f(&mut engine)
    }
    
    /// Execute operation with model manager
    fn with_model_manager<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut ModelManager) -> Result<R, String>,
    {
        let mut manager = self.model_manager
            .lock()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        f(&mut manager)
    }
    
    /// Get server status as JSON
    fn get_status(&self) -> serde_json::Value {
        let uptime = self.start_time.elapsed().as_secs();
        let idle_time = self.get_idle_time().as_secs();
        
        let model_loaded = self.transcription_engine
            .lock()
            .map(|e| e.is_model_loaded())
            .unwrap_or(false);
        
        let model_path = self.transcription_engine
            .lock()
            .ok()
            .and_then(|e| e.get_model_path().map(|p| p.to_string()))
            .unwrap_or_else(|| "unknown".to_string());
        
        serde_json::json!({
            "service_running": true,
            "model_loaded": model_loaded,
            "model_path": model_path,
            "audio_device": "default",
            "uptime_seconds": uptime,
            "last_activity_seconds_ago": idle_time,
            "recording_active": self.is_recording(),
        })
    }
}

pub struct SocketServer {
    inner: Arc<ServerInner>,
    listener: UnixListener,
}

impl SocketServer {
    pub fn new<P: AsRef<Path>>(socket_path: P, idle_timeout_secs: u64) -> ServerResult<Self> {
        // Remove existing socket file if it exists
        if socket_path.as_ref().exists() {
            std::fs::remove_file(socket_path.as_ref())?;
        }

        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    SocketError::ConnectionError(
                        format!("Service already running at socket: {}. Use 'dictate stop' to stop it first.", 
                        socket_path.as_ref().display())
                    )
                } else {
                    SocketError::ConnectionError(format!("Failed to bind socket: {}", e))
                }
            })?;

        // Set socket permissions to 0600 (owner read/write only) for security
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&socket_path)
                .map_err(|e| SocketError::ConnectionError(format!("Failed to read socket metadata: {}", e)))?
                .permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&socket_path, permissions)
                .map_err(|e| SocketError::ConnectionError(format!("Failed to set socket permissions: {}", e)))?;
        }

        let model_manager = ModelManager::new()
            .map_err(|e| SocketError::ConnectionError(format!("Failed to create model manager: {}", e)))?;

        let now = Instant::now();
        let idle_timeout = Duration::from_secs(idle_timeout_secs);
        
        let inner = Arc::new(ServerInner::new(
            TranscriptionEngine::new(),
            model_manager,
            now,
            idle_timeout,
        ));
        
        Ok(Self { inner, listener })
    }

    pub async fn run(&mut self) -> ServerResult<()> {
        println!("Socket server listening for connections...");
        println!("Idle timeout set to {} seconds", self.inner.idle_timeout.as_secs());

        let shutdown_notify = Arc::clone(&self.inner);
        
        // Spawn idle monitor task
        let idle_monitor = tokio::spawn(Self::idle_monitor(
            Arc::clone(&self.inner),
        ));

        tokio::select! {
            _ = shutdown_notify.shutdown_notify.notified() => {
                println!("Shutdown signal received, stopping server...");
                idle_monitor.abort();
                self.cleanup().await?;
                Ok(())
            }
            result = self.accept_loop() => {
                idle_monitor.abort();
                result
            }
        }
    }
    
    async fn idle_monitor(inner: Arc<ServerInner>) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            
            let idle_time = inner.get_idle_time();
            if idle_time > inner.idle_timeout {
                if let Ok(mut engine) = inner.transcription_engine.lock() {
                    if engine.is_model_loaded() {
                        println!(
                            "Idle timeout reached ({} seconds), unloading model",
                            idle_time.as_secs()
                        );
                        engine.unload_model();
                    }
                }
            }
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
        // Clean up socket file if it exists
        if let Ok(addr) = self.listener.local_addr() {
            if let Some(path) = addr.as_pathname() {
                if path.exists() {
                    if let Err(e) = std::fs::remove_file(path) {
                        eprintln!("Failed to remove socket file: {}", e);
                    }
                }
            }
        }
        Ok(())
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    inner: Arc<ServerInner>,
) -> ServerResult<()> {
    // Update last activity time
    inner.update_activity();
    let mut buffer = vec![0u8; 4096];

    loop {
        let n = stream.read(&mut buffer).await?;

        if n == 0 {
            // Connection closed
            break;
        }

        let message_str = String::from_utf8_lossy(&buffer[..n]);
        let message: Message = serde_json::from_str(&message_str)?;

        println!("Received message: {:?}", message);

        // Process message and generate response
        let response = process_message(message, Arc::clone(&inner)).await;

        // Send response back
        let response_json = serde_json::to_string(&response)?;
        stream.write_all(response_json.as_bytes()).await?;
        stream.flush().await?;
    }

    Ok(())
}

async fn process_message(
    message: Message,
    inner: Arc<ServerInner>,
) -> Response {
    match message.message_type {
        crate::socket::MessageType::Transcribe => {
            // TOGGLE LOGIC: Check if recording is already active
            if !inner.is_recording() {
                // START RECORDING (non-blocking)
                let recorder = match AudioRecorder::new() {
                    Ok(recorder) => recorder,
                    Err(e) => {
                        return Response::error(
                            message.id,
                            format!("Failed to create audio recorder: {}", e),
                        );
                    }
                };

                let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
                let stop_signal = Arc::new(AtomicBool::new(false));

                let stream = match recorder.start_recording_background(
                    audio_buffer.clone(),
                    stop_signal.clone(),
                ) {
                    Ok(stream) => stream,
                    Err(e) => {
                        return Response::error(
                            message.id,
                            format!("Failed to start recording: {}", e),
                        );
                    }
                };

                // Start the stream
                if let Err(e) = stream.play() {
                    return Response::error(
                        message.id,
                        format!("Failed to start audio stream: {}", e),
                    );
                }

                let session = RecordingSession {
                    audio_buffer,
                    start_time: Instant::now(),
                    stream,
                    stop_signal,
                };

                if let Err(e) = inner.start_recording(session) {
                    return Response::error(
                        message.id,
                        format!("Failed to start recording: {}", e),
                    );
                }

                Response::recording(
                    message.id,
                    serde_json::json!({
                        "status": "recording_started",
                        "message": "Recording... Press again to stop"
                    }),
                )
            } else {
                // STOP RECORDING and transcribe
                let session = match inner.stop_recording() {
                    Ok(session) => session,
                    Err(e) => {
                        return Response::error(
                            message.id,
                            format!("Failed to stop recording: {}", e),
                        );
                    }
                };

                // Signal stop
                session.stop_signal.store(true, Ordering::Release);

                // Drop stream to stop recording
                drop(session.stream);

                // Small delay to ensure last samples are written
                std::thread::sleep(std::time::Duration::from_millis(100));

                // Get audio buffer
                let buffer = match session.audio_buffer.lock() {
                    Ok(buffer) => buffer.clone(),
                    Err(e) => {
                        return Response::error(
                            message.id,
                            format!("Failed to access audio buffer: {}", e),
                        );
                    }
                };

                let duration = session.start_time.elapsed();

                if buffer.is_empty() {
                    return Response::error(
                        message.id,
                        "No audio recorded".to_string(),
                    );
                }

                // Write buffer to temporary WAV file
                let temp_path = "/tmp/recording.wav";
                if let Err(e) = AudioRecorder::buffer_to_wav(&buffer, temp_path, 16000) {
                    return Response::error(
                        message.id,
                        format!("Failed to write audio file: {}", e),
                    );
                }

                // Transcribe the audio using helper method
                let model_name = "base";
                let model_path = match inner.with_model_manager(|manager| {
                    Ok(manager
                        .get_model_path(model_name)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| {
                            eprintln!("Model '{}' not found in model manager", model_name);
                            "whisper-base".to_string()
                        }))
                }) {
                    Ok(path) => path,
                    Err(e) => {
                        return Response::error(
                            message.id,
                            format!("Failed to access model manager: {}", e),
                        );
                    }
                };

                // Load model and transcribe using helper method
                match inner.with_transcription_engine(|engine| {
                    // Load model if not already loaded
                    if !engine.is_model_loaded() {
                        if let Err(e) = engine.load_model(&model_path) {
                            return Err(format!("Failed to load transcription model: {}", e));
                        }
                    }

                    // Transcribe
                    match engine.transcribe_file(temp_path) {
                        Ok(text) => Ok((text, engine.get_model_path().unwrap_or("unknown").to_string())),
                        Err(e) => Err(format!("Transcription failed: {}", e)),
                    }
                }) {
                    Ok((text, model_path)) => Response::result(
                        message.id,
                        serde_json::json!({
                            "text": text,
                            "duration": duration.as_secs_f32(),
                            "model": model_path,
                        }),
                    ),
                    Err(e) => Response::error(
                        message.id,
                        e,
                    ),
                }
            }
        }

        crate::socket::MessageType::Status => {
            Response::status(
                message.id,
                inner.get_status(),
            )
        }

        crate::socket::MessageType::Stop => {
            // Trigger shutdown
            inner.shutdown_notify.notify_waiters();
            Response::result(
                message.id,
                serde_json::json!({
                    "message": "Service stopping"
                }),
            )
        }
    }
}

pub struct SocketClient {
    socket_path: String,
}

impl SocketClient {
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    pub async fn send_message(&self, message: Message) -> ServerResult<Response> {
        let mut stream =
            UnixStream::connect(&self.socket_path)
                .await
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => SocketError::ConnectionError(
                        "Service is not running. Use 'dictate service' to start the service."
                            .to_string(),
                    ),
                    std::io::ErrorKind::NotFound => SocketError::ConnectionError(format!(
                    "Service socket not found at {}. Use 'dictate service' to start the service.",
                    self.socket_path
                )),
                    _ => SocketError::ConnectionError(format!(
                        "Failed to connect to service at {}: {}",
                        self.socket_path, e
                    )),
                })?;

        let message_json = serde_json::to_string(&message)?;
        stream.write_all(message_json.as_bytes()).await?;
        stream.flush().await?;

        // Read response with timeout
        let mut buffer = vec![0u8; 4096];

        let read_result = tokio::time::timeout(
            std::time::Duration::from_secs(120), // 2 minute timeout
            stream.read(&mut buffer),
        )
        .await;

        let n = match read_result {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                return Err(SocketError::IoError(e));
            }
            Err(_) => {
                return Err(SocketError::ConnectionError(
                    "Request timed out after 2 minutes".to_string(),
                ));
            }
        };

        if n == 0 {
            return Err(SocketError::ConnectionError(
                "No response from server".to_string(),
            ));
        }

        let response_str = String::from_utf8_lossy(&buffer[..n]);
        let response: Response = serde_json::from_str(&response_str)?;

        Ok(response)
    }

    pub async fn transcribe(
        &self,
        max_duration: u64,
        sample_rate: u32,
) -> ServerResult<Response> {
        let params = serde_json::json!({
            "max_duration": max_duration,
            "sample_rate": sample_rate
        });

        let message = Message::transcribe(params);
        self.send_message(message).await
    }

    pub async fn status(&self) -> ServerResult<Response> {
        let params = serde_json::json!({});
        let message = Message::status(params);
        self.send_message(message).await
    }

    pub async fn stop(&self) -> ServerResult<Response> {
        let params = serde_json::json!({});
        let message = Message::stop(params);
        self.send_message(message).await
    }
}

