//! Socket server for handling transcription requests
//!
//! This module provides the main server that listens for client connections
//! and coordinates transcription requests.

mod handler;

use crate::models::ModelManager;
use crate::protocol::ServerMessage;
use crate::transcription::TranscriptionEngine;
use crate::transport::{SocketError, encode_server_message};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UnixListener;
use tokio::sync::Notify;

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
            // Simple 2-second keep-alive heartbeat
            // No throttling - spectrum updates broadcast immediately from spectrum task
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Broadcast current status (without spectrum - that comes from spectrum task)
            inner.broadcast_status();
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
    // Shared mutable state
    pub transcription_engine: std::sync::Mutex<TranscriptionEngine>,
    pub model_manager: std::sync::Mutex<ModelManager>,
    pub last_activity: std::sync::Mutex<Instant>,
    pub subscribers: std::sync::Mutex<Vec<SubscriberHandle>>,
    pub current_state: std::sync::Mutex<crate::protocol::State>, // Track current state for heartbeat
    pub last_spectrum: std::sync::Mutex<Option<Vec<f32>>>, // Track last spectrum for heartbeat

    // Shared immutable state
    pub start_time: Instant,
    pub idle_timeout: Duration,
    pub model_name: String,

    // Async coordination
    pub shutdown_notify: Notify,
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
            last_spectrum: std::sync::Mutex::new(None),
            start_time,
            idle_timeout,
            model_name,
            shutdown_notify: Notify::new(),
        }
    }

    /// Get monotonic timestamp in milliseconds since server start
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Broadcast a typed event to all subscribers
    fn broadcast_event(&self, event: ServerMessage) {
        let subscribers: &std::sync::Mutex<Vec<SubscriberHandle>> = &self.subscribers;
        let event_json = encode_server_message(&event).unwrap();
        let bytes = event_json.into_bytes();

        if let Ok(mut subs) = subscribers.lock() {
            subs.retain(|sub| {
                // Try to send, remove if channel is closed
                sub.tx.send(bytes.clone()).is_ok()
            });
        }
    }

    /// Update last activity time
    pub fn update_activity(&self) {
        if let Ok(mut last) = self.last_activity.lock() {
            *last = Instant::now();
        }
    }

    /// Update current state (for heartbeat tracking)
    pub fn set_current_state(&self, state: crate::protocol::State) {
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

    /// Update spectrum data
    pub fn update_spectrum(&self, bands: Vec<f32>) {
        if let Ok(mut spectrum) = self.last_spectrum.lock() {
            *spectrum = Some(bands);
        }
    }

    /// Get last spectrum data
    fn get_last_spectrum(&self) -> Option<Vec<f32>> {
        self.last_spectrum.lock().ok().and_then(|s| s.clone())
    }

    /// Clear spectrum data (when not recording)
    pub fn clear_spectrum(&self) {
        if let Ok(mut spectrum) = self.last_spectrum.lock() {
            *spectrum = None;
        }
    }

    /// Broadcast unified status event with current state
    pub fn broadcast_status(&self) {
        let state = self.get_current_state();
        let spectrum = self.get_last_spectrum();
        let idle_hot = self.get_idle_hot();
        let ts = self.elapsed_ms();

        self.broadcast_event(ServerMessage::new_status_event(
            state, spectrum, idle_hot, ts,
        ));
    }

    /// Execute operation with transcription engine
    pub fn with_transcription_engine<F, R>(&self, f: F) -> Result<R, String>
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
    pub fn with_model_manager<F, R>(&self, f: F) -> Result<R, String>
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
    pub fn get_status(&self) -> serde_json::Value {
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
