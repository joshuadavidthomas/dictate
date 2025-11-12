use crate::audio::AudioRecorder;
use crate::get_recording_path;
use crate::models::ModelManager;
use crate::socket::{Message, Response, SocketError};
use crate::transcription::TranscriptionEngine;
use cpal::traits::StreamTrait;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

// Server result type using SocketError for structured error handling
type ServerResult<T> = std::result::Result<T, SocketError>;

/// Handle for a subscriber connection
struct SubscriberHandle {
    id: String,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

/// State for level throttling
struct LevelThrottler {
    last_level: f32,
    last_time: Instant,
}

impl LevelThrottler {
    fn new() -> Self {
        Self {
            last_level: 0.0,
            last_time: Instant::now(),
        }
    }

    fn should_send(&mut self, level: f32) -> bool {
        let delta = (level - self.last_level).abs();
        let elapsed = self.last_time.elapsed();
        
        // Throttle: send if delta >= 0.03 OR 250ms heartbeat elapsed
        if delta >= 0.03 || elapsed >= Duration::from_millis(250) {
            self.last_level = level;
            self.last_time = Instant::now();
            true
        } else {
            false
        }
    }
}

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
                    "Service already running at socket: {}. Use 'dictate stop' to stop it first.",
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
        ));

        Ok(Self {
            inner,
            listener,
        })
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

        let shutdown_notify = Arc::clone(&self.inner);

        // Spawn idle monitor task
        let idle_monitor = tokio::spawn(Self::idle_monitor(Arc::clone(&self.inner)));
        
        // Spawn heartbeat task (broadcasts status every 2 seconds to keep OSD alive)
        let heartbeat = tokio::spawn(Self::heartbeat_monitor(Arc::clone(&self.inner)));

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

            let idle_time = inner.get_idle_time();
            if idle_time <= inner.idle_timeout {
                continue;
            }

            let Ok(mut engine) = inner.transcription_engine.lock() else {
                continue;
            };

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
            // Wait 2 seconds between heartbeats
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Broadcast current state (whatever it is) to keep OSD alive
            let current_state = inner.get_current_state();
            inner.broadcast_event("status", serde_json::json!({
                "state": current_state,
                "level": 0.0,
                "idle_hot": inner.get_idle_hot(),
                "ts": inner.elapsed_ms(),
                "ver": 1
            }));
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

async fn handle_connection(
    mut stream: UnixStream,
    inner: Arc<ServerInner>,
) -> ServerResult<()> {
    inner.update_activity();
    let mut buffer = vec![0u8; 4096];
    
    // Track if this connection is a subscriber
    let mut subscriber_id: Option<String> = None;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

    loop {
        tokio::select! {
            // Read from client
            result = stream.read(&mut buffer) => {
                let n = result?;
                
                if n == 0 {
                    // Connection closed
                    break;
                }

                let message_str = String::from_utf8_lossy(&buffer[..n]);
                let message: Message = serde_json::from_str(&message_str)?;

                println!("Received message: {:?}", message);

                match message.message_type {
                    crate::socket::MessageType::Subscribe => {
                        // Add to subscribers
                        subscriber_id = Some(message.id.to_string());
                        if let Ok(mut subs) = inner.subscribers.lock() {
                            subs.push(SubscriberHandle {
                                id: message.id.to_string(),
                                tx: event_tx.clone(),
                            });
                        }
                        
                        // Send initial status event (set state to Idle)
                        inner.set_current_state("Idle");
                        inner.broadcast_event("status", serde_json::json!({
                            "state": "Idle",
                            "level": 0.0,
                            "idle_hot": inner.get_idle_hot(),
                            "ts": inner.elapsed_ms(),
                            "cap": ["idle_hot"],
                            "ver": 1
                        }));
                        
                        // Send acknowledgment
                        let response = Response::result(
                            message.id,
                            serde_json::json!({"subscribed": true}),
                        );
                        let response_json = serde_json::to_string(&response)?;
                        stream.write_all(response_json.as_bytes()).await?;
                        stream.write_all(b"\n").await?;
                        stream.flush().await?;
                    }
                    _ => {
                        // Regular request-response
                        let response = process_message(message, Arc::clone(&inner)).await;
                        let response_json = serde_json::to_string(&response)?;
                        stream.write_all(response_json.as_bytes()).await?;
                        stream.write_all(b"\n").await?;
                        stream.flush().await?;
                    }
                }
            }
            
            // Send events to subscriber
            Some(event_data) = event_rx.recv() => {
                stream.write_all(&event_data).await?;
                stream.flush().await?;
            }
        }
    }
    
    // Clean up subscriber on disconnect
    if let Some(id) = subscriber_id {
        if let Ok(mut subs) = inner.subscribers.lock() {
            subs.retain(|s| s.id != id);
        }
    }

    Ok(())
}

async fn process_message(
    message: Message,
    inner: Arc<ServerInner>,
) -> Response {
    match message.message_type {
        crate::socket::MessageType::Transcribe => {
            // Update and broadcast Recording state
            inner.set_current_state("Recording");
            inner.broadcast_event("state", serde_json::json!({
                "state": "Recording",
                "idle_hot": inner.get_idle_hot(),
                "ts": inner.elapsed_ms(),
                "ver": 1
            }));
            // Get max_duration from params (default 30 seconds)
            let max_duration = message
                .params
                .get("max_duration")
                .and_then(|v| v.as_u64())
                .unwrap_or(30);

            // Get silence_duration from params (default 2 seconds)
            let silence_duration = message
                .params
                .get("silence_duration")
                .and_then(|v| v.as_u64())
                .unwrap_or(2);

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

            let silence_detector = Some(crate::audio::SilenceDetector::new(
                0.01,
                Duration::from_secs(silence_duration),
            ));

            // Create level channel for OSD updates
            let (level_tx, mut level_rx) = tokio::sync::mpsc::unbounded_channel();

            // Spawn task to broadcast levels to subscribers with throttling
            let inner_clone = Arc::clone(&inner);
            tokio::spawn(async move {
                let mut throttler = LevelThrottler::new();
                
                while let Some(level) = level_rx.recv().await {
                    if throttler.should_send(level) {
                        inner_clone.broadcast_event("level", serde_json::json!({
                            "v": level,
                            "ts": inner_clone.elapsed_ms(),
                            "ver": 1
                        }));
                    }
                }
            });

            let stream = match recorder.start_recording_background(
                audio_buffer.clone(),
                stop_signal.clone(),
                silence_detector,
                Some(level_tx),
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
                return Response::error(message.id, format!("Failed to start audio stream: {}", e));
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

            // Get audio buffer
            let buffer = match audio_buffer.lock() {
                Ok(buffer) => buffer.clone(),
                Err(e) => {
                    return Response::error(
                        message.id,
                        format!("Failed to access audio buffer: {}", e),
                    );
                }
            };

            let duration = start_time.elapsed();

            if buffer.is_empty() {
                return Response::error(message.id, "No audio recorded".to_string());
            }

            // Write buffer to recording file in app data directory
            let recording_path = match get_recording_path() {
                Ok(path) => path,
                Err(e) => {
                    return Response::error(
                        message.id,
                        format!("Failed to get recording path: {}", e),
                    );
                }
            };

            if let Err(e) = AudioRecorder::buffer_to_wav(&buffer, &recording_path, 16000) {
                return Response::error(message.id, format!("Failed to write audio file: {}", e));
            }

            // Update and broadcast Transcribing state
            inner.set_current_state("Transcribing");
            inner.broadcast_event("state", serde_json::json!({
                "state": "Transcribing",
                "idle_hot": inner.get_idle_hot(),
                "ts": inner.elapsed_ms(),
                "ver": 1
            }));

            // Small delay to ensure Transcribing state is visible in OSD
            // even for very fast transcriptions
            std::thread::sleep(std::time::Duration::from_millis(100));

            // Transcribe using preloaded model
            // First check if we need to reload the model
            let model_loaded = inner.with_transcription_engine(|engine| {
                Ok(engine.is_model_loaded())
            }).unwrap_or(false);
            
            if !model_loaded {
                println!("Model was unloaded, reloading...");
                // Get the model path from the model manager
                let model_path_result = inner.with_model_manager(|manager| {
                    manager.get_model_path(&inner.model_name)
                        .ok_or_else(|| format!("Model '{}' not found", &inner.model_name))
                        .map(|p| p.to_string_lossy().to_string())
                });
                
                match model_path_result {
                    Ok(model_path) => {
                        let reload_result = inner.with_transcription_engine(|engine| {
                            engine.load_model(&model_path)
                                .map_err(|e| format!("Failed to reload model: {}", e))
                        });
                        
                        match reload_result {
                            Ok(_) => println!("Model reloaded successfully"),
                            Err(e) => return Response::error(message.id, e),
                        }
                    }
                    Err(e) => return Response::error(message.id, format!("Failed to get model path: {}", e)),
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
            }) {
                Ok((text, model_path)) => Response::result(
                    message.id,
                    serde_json::json!({
                        "text": text,
                        "duration": duration.as_secs_f32(),
                        "model": model_path,
                    }),
                ),
                Err(e) => Response::error(message.id, e),
            };

            // Update and broadcast Idle state (transcription complete)
            inner.set_current_state("Idle");
            inner.broadcast_event("state", serde_json::json!({
                "state": "Idle",
                "idle_hot": inner.get_idle_hot(),
                "ts": inner.elapsed_ms(),
                "ver": 1
            }));

            response
        }

        crate::socket::MessageType::Status => Response::status(message.id, inner.get_status()),

        crate::socket::MessageType::Subscribe => {
            // This should never be reached as Subscribe is handled in handle_connection
            Response::error(message.id, "Subscribe should be handled at connection level".to_string())
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
        let mut stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            match e.kind() {
                std::io::ErrorKind::ConnectionRefused => SocketError::Connection(
                    "Service is not running. Use 'dictate service' to start the service."
                        .to_string(),
                ),
                std::io::ErrorKind::NotFound => SocketError::Connection(format!(
                    "Service socket not found at {}. Use 'dictate service' to start the service.",
                    self.socket_path
                )),
                _ => SocketError::Connection(format!(
                    "Failed to connect to service at {}: {}",
                    self.socket_path, e
                )),
            }
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
                return Err(SocketError::Io(e));
            }
            Err(_) => {
                return Err(SocketError::Connection(
                    "Request timed out after 2 minutes".to_string(),
                ));
            }
        };

        if n == 0 {
            return Err(SocketError::Connection(
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
        silence_duration: u64,
        sample_rate: u32,
    ) -> ServerResult<Response> {
        let params = serde_json::json!({
            "max_duration": max_duration,
            "silence_duration": silence_duration,
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

/// Inner server state with all shared data
struct ServerInner {
    // Shared mutable state
    transcription_engine: std::sync::Mutex<TranscriptionEngine>,
    model_manager: std::sync::Mutex<ModelManager>,
    last_activity: std::sync::Mutex<Instant>,
    subscribers: std::sync::Mutex<Vec<SubscriberHandle>>,
    current_state: std::sync::Mutex<String>,  // Track current state for heartbeat

    // Shared immutable state
    start_time: Instant,
    idle_timeout: Duration,
    model_name: String,

    // Async coordination
    shutdown_notify: Notify,
}

impl ServerInner {
    fn new(
        transcription_engine: TranscriptionEngine,
        model_manager: ModelManager,
        start_time: Instant,
        idle_timeout: Duration,
        model_name: String,
    ) -> Self {
        Self {
            transcription_engine: std::sync::Mutex::new(transcription_engine),
            model_manager: std::sync::Mutex::new(model_manager),
            last_activity: std::sync::Mutex::new(start_time),
            subscribers: std::sync::Mutex::new(Vec::new()),
            current_state: std::sync::Mutex::new("Idle".to_string()),  // Start in Idle state
            start_time,
            idle_timeout,
            model_name,
            shutdown_notify: Notify::new(),
        }
    }

    /// Get monotonic timestamp in milliseconds since server start
    fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Broadcast an event to all subscribers
    fn broadcast_event(&self, event_name: &str, data: serde_json::Value) {
        let response = Response::event(event_name, data);
        let mut json_str = serde_json::to_string(&response).unwrap();
        json_str.push('\n'); // NDJSON
        let bytes = json_str.into_bytes();

        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|sub| {
                // Try to send, remove if channel is closed
                sub.tx.send(bytes.clone()).is_ok()
            });
        }
    }

    /// Update last activity time
    fn update_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    /// Update current state (for heartbeat tracking)
    fn set_current_state(&self, state: &str) {
        if let Ok(mut current) = self.current_state.lock() {
            *current = state.to_string();
        }
    }

    /// Get current state (for heartbeat broadcasting)
    fn get_current_state(&self) -> String {
        self.current_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "Idle".to_string())
    }

    /// Get current idle time
    fn get_idle_time(&self) -> Duration {
        self.last_activity
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or_default()
    }

    /// Check if model is loaded (hot idle vs cold idle)
    fn get_idle_hot(&self) -> bool {
        self.transcription_engine
            .lock()
            .map(|e| e.is_model_loaded())
            .unwrap_or(false)
    }

    /// Execute operation with transcription engine
    fn with_transcription_engine<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut TranscriptionEngine) -> Result<R, String>,
    {
        let mut engine = self
            .transcription_engine
            .lock()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        f(&mut engine)
    }

    /// Execute operation with model manager
    fn with_model_manager<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut ModelManager) -> Result<R, String>,
    {
        let mut manager = self
            .model_manager
            .lock()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        f(&mut manager)
    }

    /// Get server status as JSON
    fn get_status(&self) -> serde_json::Value {
        let uptime = self.start_time.elapsed().as_secs();
        let idle_time = self.get_idle_time().as_secs();

        let model_loaded = self
            .transcription_engine
            .lock()
            .map(|e| e.is_model_loaded())
            .unwrap_or(false);

        let model_path = self
            .transcription_engine
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
        })
    }
}
