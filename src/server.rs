use crate::audio::{AudioRecorder, buffer_to_wav};
use crate::get_recording_path;
use crate::models::ModelManager;
use crate::protocol::Response;
use crate::socket::SocketError;
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

/// State for spectrum throttling
struct SpectrumThrottler {
    last_bands: Vec<f32>,
    last_time: Instant,
}

impl SpectrumThrottler {
    fn new() -> Self {
        Self {
            last_bands: vec![0.0; 8],
            last_time: Instant::now(),
        }
    }

    fn should_send(&mut self, bands: &Vec<f32>) -> bool {
        let elapsed = self.last_time.elapsed();

        // Calculate max delta across all bands
        let max_delta = bands
            .iter()
            .zip(self.last_bands.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);

        // Throttle: send if max delta >= 0.05 OR 250ms heartbeat elapsed
        if max_delta >= 0.05 || elapsed >= Duration::from_millis(250) {
            self.last_bands = bands.clone();
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
            inner.broadcast_typed_event(crate::protocol::Event::new_status(
                current_state,
                0.0,
                inner.get_idle_hot(),
                inner.elapsed_ms(),
            ));
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

async fn handle_connection(mut stream: UnixStream, inner: Arc<ServerInner>) -> ServerResult<()> {
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
                // Try to parse as new Request type first, fall back to old Message type
                let request: crate::protocol::Request = serde_json::from_str(&message_str)?;

                println!("Received request: {:?}", request);

                match request {
                    crate::protocol::Request::Subscribe { id } => {
                        // Add to subscribers
                        subscriber_id = Some(id.to_string());
                        if let Ok(mut subs) = inner.subscribers.lock() {
                            subs.push(SubscriberHandle {
                                id: id.to_string(),
                                tx: event_tx.clone(),
                            });
                        }

                        // Send initial status event (set state to Idle)
                        inner.set_current_state(crate::protocol::State::Idle);
                        inner.broadcast_typed_event(crate::protocol::Event::new_status(
                            crate::protocol::State::Idle,
                            0.0,
                            inner.get_idle_hot(),
                            inner.elapsed_ms(),
                        ));

                        // Send acknowledgment using Response type
                        let response = Response::new_subscribed(id);
                        let message_json = crate::transport::codec::encode_response(&response)?;
                        stream.write_all(message_json.as_bytes()).await?;
                        stream.flush().await?;
                    }
                    crate::protocol::Request::Transcribe { id, max_duration, silence_duration, sample_rate } => {
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
                                    let message_json = crate::transport::codec::encode_response(&response)?;
                                    stream.write_all(message_json.as_bytes()).await?;
                                    stream.flush().await?;
                                    break;
                                }

                                // Continue processing events while waiting for result
                                Some(event_data) = event_rx.recv() => {
                                    stream.write_all(&event_data).await?;
                                    stream.flush().await?;
                                }
                            }
                        }
                    }
                    _ => {
                        // Regular request-response (Status, etc.)
                        let response = process_message(request, Arc::clone(&inner)).await;
                        let message_json = crate::transport::codec::encode_response(&response)?;
                        stream.write_all(message_json.as_bytes()).await?;
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

/// Handle transcribe request in background task
async fn handle_transcribe_request(
    id: uuid::Uuid,
    max_duration: u64,
    silence_duration: u64,
    _sample_rate: u32,
    inner: Arc<ServerInner>,
) -> Response {
    // Update and broadcast Recording state
    inner.set_current_state(crate::protocol::State::Recording);
    inner.broadcast_typed_event(crate::protocol::Event::new_state(
        crate::protocol::State::Recording,
        inner.get_idle_hot(),
        inner.elapsed_ms(),
    ));

    let recorder = match AudioRecorder::new() {
        Ok(recorder) => recorder,
        Err(e) => {
            return Response::Error {
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

    // Spawn task to broadcast spectrum to subscribers with throttling
    let inner_clone = Arc::clone(&inner);
    tokio::spawn(async move {
        let mut throttler = SpectrumThrottler::new();

        while let Some(bands) = spectrum_rx.recv().await {
            if throttler.should_send(&bands) {
                inner_clone.broadcast_typed_event(crate::protocol::Event::new_spectrum(
                    bands,
                    inner_clone.elapsed_ms(),
                ));
            }
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
            return Response::Error {
                id,
                error: format!("Failed to start recording: {}", e),
            };
        }
    };

    // Start the stream
    if let Err(e) = stream.play() {
        return Response::Error {
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
    inner.set_current_state(crate::protocol::State::Transcribing);
    inner.broadcast_typed_event(crate::protocol::Event::new_state(
        crate::protocol::State::Transcribing,
        inner.get_idle_hot(),
        inner.elapsed_ms(),
    ));

    // Get audio buffer
    let buffer = match audio_buffer.lock() {
        Ok(buffer) => buffer.clone(),
        Err(e) => {
            return Response::Error {
                id,
                error: format!("Failed to access audio buffer: {}", e),
            };
        }
    };

    let duration = start_time.elapsed();

    if buffer.is_empty() {
        return Response::Error {
            id,
            error: "No audio recorded".to_string(),
        };
    }

    // Write buffer to recording file in app data directory
    let recording_path = match get_recording_path() {
        Ok(path) => path,
        Err(e) => {
            return Response::Error {
                id,
                error: format!("Failed to get recording path: {}", e),
            };
        }
    };

    if let Err(e) = buffer_to_wav(&buffer, &recording_path, 16000) {
        return Response::Error {
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
        .unwrap_or(false);

    if !model_loaded {
        println!("Model was unloaded, reloading...");
        // Get the model path from the model manager
        let model_path_result = inner.with_model_manager(|manager| {
            manager
                .get_model_path(&inner.model_name)
                .ok_or_else(|| format!("Model '{}' not found", &inner.model_name))
                .map(|p| p.to_string_lossy().to_string())
        });

        match model_path_result {
            Ok(model_path) => {
                let reload_result = inner.with_transcription_engine(|engine| {
                    engine
                        .load_model(&model_path)
                        .map_err(|e| format!("Failed to reload model: {}", e))
                });

                match reload_result {
                    Ok(_) => println!("Model reloaded successfully"),
                    Err(e) => {
                        return Response::Error { id, error: e };
                    }
                }
            }
            Err(e) => {
                return Response::Error {
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
    }) {
        Ok((text, model_path)) => {
            Response::new_result(id, text, duration.as_secs_f32(), model_path)
        }
        Err(e) => Response::Error { id, error: e },
    };

    // Update and broadcast Idle state (transcription complete)
    inner.set_current_state(crate::protocol::State::Idle);
    inner.broadcast_typed_event(crate::protocol::Event::new_state(
        crate::protocol::State::Idle,
        inner.get_idle_hot(),
        inner.elapsed_ms(),
    ));

    response
}

async fn process_message(request: crate::protocol::Request, inner: Arc<ServerInner>) -> Response {
    match request {
        crate::protocol::Request::Transcribe { id, .. } => {
            // Transcribe requests are now handled directly in handle_connection
            // This shouldn't be reached
            Response::Error {
                id,
                error: "Transcribe requests should be handled in background task".to_string(),
            }
        }

        crate::protocol::Request::Status { id } => {
            let status_json = inner.get_status();
            Response::new_status(
                id,
                status_json["service_running"].as_bool().unwrap_or(false),
                status_json["model_loaded"].as_bool().unwrap_or(false),
                status_json["model_path"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string(),
                status_json["audio_device"]
                    .as_str()
                    .unwrap_or("default")
                    .to_string(),
                status_json["uptime_seconds"].as_u64().unwrap_or(0),
                status_json["last_activity_seconds_ago"]
                    .as_u64()
                    .unwrap_or(0),
            )
        }

        crate::protocol::Request::Subscribe { id } => {
            // This should never be reached as Subscribe is handled in handle_connection
            Response::Error {
                id,
                error: "Subscribe should be handled at connection level".to_string(),
            }
        }
    }
}

pub struct SocketClient {
    transport: crate::transport::AsyncTransport,
}

impl SocketClient {
    pub fn new(socket_path: String) -> Self {
        Self {
            transport: crate::transport::AsyncTransport::new(socket_path),
        }
    }

    pub async fn status(&self) -> ServerResult<Response> {
        let request = crate::protocol::Request::new_status();
        self.transport.send_request(&request).await
    }
}

/// Inner server state with all shared data
struct ServerInner {
    // Shared mutable state
    transcription_engine: std::sync::Mutex<TranscriptionEngine>,
    model_manager: std::sync::Mutex<ModelManager>,
    last_activity: std::sync::Mutex<Instant>,
    subscribers: std::sync::Mutex<Vec<SubscriberHandle>>,
    current_state: std::sync::Mutex<crate::protocol::State>, // Track current state for heartbeat

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
            current_state: std::sync::Mutex::new(crate::protocol::State::Idle), // Start in Idle state
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

    /// Broadcast a typed event to all subscribers
    fn broadcast_typed_event(&self, event: crate::protocol::Event) {
        let event_json = crate::transport::codec::encode_event(&event).unwrap();
        let bytes = event_json.into_bytes();

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
    fn set_current_state(&self, state: crate::protocol::State) {
        if let Ok(mut current) = self.current_state.lock() {
            *current = state;
        }
    }

    /// Get current state (for heartbeat broadcasting)
    fn get_current_state(&self) -> crate::protocol::State {
        self.current_state
            .lock()
            .map(|s| *s)
            .unwrap_or(crate::protocol::State::Idle)
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
