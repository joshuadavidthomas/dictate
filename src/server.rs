//! Socket server for handling transcription requests
//!
//! This module provides the main server that listens for client connections
//! and coordinates transcription requests.

mod handler;

use crate::models::ModelManager;
use crate::protocol::{ServerMessage, State};
use crate::transcription::TranscriptionEngine;
use crate::transport::{SocketError, encode_server_message};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UnixListener;
use tokio::sync::{Mutex, Notify, RwLock};

use handler::handle_connection;

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
    start_time: Instant,
    idle_timeout: Duration,
    model_name: String,
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
            transcription_engine: RwLock::new(transcription_engine),
            model_manager: RwLock::new(model_manager),
            last_activity: Mutex::new(start_time),
            subscribers: Mutex::new(Vec::new()),
            current_state: Mutex::new(State::Idle), // Start in Idle state
            last_spectrum: Mutex::new(None),
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
    async fn get_status(&self) -> (bool, bool, String, String, u64, u64) {
        let uptime = self.start_time.elapsed().as_secs();
        let idle_time = self.get_idle_time().await.as_secs();

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
        )
    }
}
