//! Dictate - Voice transcription for Linux
//!
//! This is the main application module containing:
//! - State management (recording, transcription)
//! - Event broadcasting
//! - Database operations
//! - Text output (clipboard, insertion)
//! - CLI handling
//! - System tray
//! - All Tauri commands
//! - Application setup

mod audio;
mod osd;
mod settings;
mod transcription;

use crate::audio::{AudioDevice, RecordingSession, SPECTRUM_BANDS};
use crate::settings::{OsdPosition, OutputMode, Settings, SettingsState};
use crate::transcription::{Engine, ModelId, ModelInfo, ModelManager, StorageInfo};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;

// ============================================================================
// State Management
// ============================================================================

/// Recording phase with associated data
enum RecordingPhase {
    Idle,
    Recording { session: RecordingSession },
    Transcribing,
}

/// Snapshot of recording state for external consumption
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingStatus {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl RecordingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::Transcribing => "transcribing",
            Self::Error => "error",
        }
    }
}

/// Thread-safe recording state
pub struct RecordingState(Mutex<RecordingPhase>);

impl RecordingState {
    pub fn new() -> Self {
        Self(Mutex::new(RecordingPhase::Idle))
    }

    pub async fn status(&self) -> RecordingStatus {
        match &*self.0.lock().await {
            RecordingPhase::Idle => RecordingStatus::Idle,
            RecordingPhase::Recording { .. } => RecordingStatus::Recording,
            RecordingPhase::Transcribing => RecordingStatus::Transcribing,
        }
    }

    pub async fn start(&self, session: RecordingSession) {
        *self.0.lock().await = RecordingPhase::Recording { session };
    }

    /// Stop recording and return the session (if recording)
    pub async fn stop(&self) -> Option<RecordingSession> {
        let mut phase = self.0.lock().await;
        if let RecordingPhase::Recording { .. } = &*phase {
            let old = std::mem::replace(&mut *phase, RecordingPhase::Transcribing);
            if let RecordingPhase::Recording { session } = old {
                return Some(session);
            }
        }
        None
    }

    pub async fn finish(&self) {
        *self.0.lock().await = RecordingPhase::Idle;
    }

    pub async fn elapsed_ms(&self) -> u64 {
        match &*self.0.lock().await {
            RecordingPhase::Recording { session } => session.elapsed_ms(),
            _ => 0,
        }
    }
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe transcription engine state
pub struct TranscriptionState(Mutex<Engine>);

impl TranscriptionState {
    pub fn new() -> Self {
        Self(Mutex::new(Engine::new()))
    }

    pub async fn engine(&self) -> tokio::sync::MutexGuard<'_, Engine> {
        self.0.lock().await
    }

    /// Ensure a model is loaded, using preference from settings
    pub async fn ensure_loaded(&self, settings: &Settings) -> Result<()> {
        let mut engine = self.0.lock().await;

        if engine.is_loaded() {
            return Ok(());
        }

        let manager = ModelManager::new()?;
        let (id, path) = transcription::find_available_model(&manager, settings.preferred_model)
            .ok_or_else(|| anyhow::anyhow!("No transcription model available"))?;

        engine.load(id, &path)?;
        Ok(())
    }
}

impl Default for TranscriptionState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Event Broadcasting
// ============================================================================

/// Events broadcast to OSD and frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Recording state changed
    Status {
        state: RecordingStatus,
        spectrum: Option<Vec<f32>>,
        ts: u64,
    },
    /// Transcription completed
    Result {
        id: Uuid,
        text: String,
        duration_secs: f32,
    },
    /// Error occurred
    Error {
        id: Uuid,
        error: String,
    },
    /// OSD position changed
    ConfigUpdate {
        osd_position: OsdPosition,
    },
    /// Model download progress
    ModelProgress {
        model: ModelId,
        downloaded: u64,
        total: u64,
        phase: String,
    },
}

impl Event {
    /// Tauri event name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status { state, .. } => match state {
                RecordingStatus::Recording => "recording-started",
                RecordingStatus::Transcribing => "recording-stopped",
                RecordingStatus::Idle => "transcription-complete",
                RecordingStatus::Error => "error",
            },
            Self::Result { .. } => "transcription-result",
            Self::Error { .. } => "error",
            Self::ConfigUpdate { .. } => "config-update",
            Self::ModelProgress { .. } => "model-download-progress",
        }
    }
}

/// Event broadcaster
#[derive(Clone)]
pub struct Broadcaster {
    tx: broadcast::Sender<Event>,
}

impl Broadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    /// Send recording status update
    pub fn status(&self, state: RecordingStatus, spectrum: Option<Vec<f32>>, ts: u64) {
        self.send(Event::Status { state, spectrum, ts });
    }

    /// Send transcription result
    pub fn result(&self, text: String, duration_secs: f32) {
        self.send(Event::Result {
            id: Uuid::new_v4(),
            text,
            duration_secs,
        });
    }

    /// Send error
    pub fn error(&self, error: String) {
        self.send(Event::Error {
            id: Uuid::new_v4(),
            error,
        });
    }

    /// Send OSD position update
    pub fn osd_position(&self, position: OsdPosition) {
        self.send(Event::ConfigUpdate {
            osd_position: position,
        });
    }

    /// Send model download progress
    pub fn model_progress(&self, model: ModelId, downloaded: u64, total: u64, phase: &str) {
        self.send(Event::ModelProgress {
            model,
            downloaded,
            total,
            phase: phase.to_string(),
        });
    }

    /// Bridge to Tauri events
    pub fn bridge_to_tauri(&self, app: AppHandle) {
        let mut rx = self.subscribe();

        tauri::async_runtime::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Err(e) = app.emit(event.name(), &event) {
                            eprintln!("[events] Failed to emit: {}", e);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("[events] Lagged {} messages", n);
                    }
                }
            }
        });
    }

    /// Drain pending messages (for OSD polling)
    pub fn drain(rx: &mut broadcast::Receiver<Event>) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }
        events
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Database
// ============================================================================

/// Database connection wrapper
#[derive(Clone)]
pub struct Database(SqlitePool);

impl Database {
    pub fn new(pool: SqlitePool) -> Self {
        Self(pool)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }
}

/// Transcription history record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    pub id: i64,
    pub text: String,
    pub created_at: i64,
    pub duration_ms: Option<i64>,
    pub model_name: Option<String>,
    pub audio_path: Option<String>,
    pub output_mode: Option<String>,
}

fn db_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "dictate", "dictate")
        .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?;
    Ok(dirs.data_dir().join("dictate.db"))
}

async fn init_database() -> Result<SqlitePool> {
    let path = db_path()?;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let url = format!("sqlite://{}", path.display());
    let options = SqliteConnectOptions::from_str(&url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Run migrations
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS transcriptions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            text TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            duration_ms INTEGER,
            model_name TEXT,
            audio_path TEXT,
            output_mode TEXT
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_transcriptions_created_at
        ON transcriptions(created_at DESC)
        "#,
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}

async fn save_transcription(
    pool: &SqlitePool,
    text: &str,
    duration_ms: Option<i64>,
    model_name: Option<&str>,
    audio_path: Option<&str>,
    output_mode: Option<&str>,
) -> Result<i64> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;

    let result = sqlx::query(
        r#"
        INSERT INTO transcriptions (text, created_at, duration_ms, model_name, audio_path, output_mode)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(text)
    .bind(created_at)
    .bind(duration_ms)
    .bind(model_name)
    .bind(audio_path)
    .bind(output_mode)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

async fn list_transcriptions(pool: &SqlitePool, limit: i64, offset: i64) -> Result<Vec<Transcription>> {
    let rows = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_name, audio_path, output_mode
        FROM transcriptions
        ORDER BY created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Transcription {
            id: row.get(0),
            text: row.get(1),
            created_at: row.get(2),
            duration_ms: row.get(3),
            model_name: row.get(4),
            audio_path: row.get(5),
            output_mode: row.get(6),
        })
        .collect())
}

async fn get_transcription(pool: &SqlitePool, id: i64) -> Result<Option<Transcription>> {
    let row = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_name, audio_path, output_mode
        FROM transcriptions
        WHERE id = ?
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| Transcription {
        id: row.get(0),
        text: row.get(1),
        created_at: row.get(2),
        duration_ms: row.get(3),
        model_name: row.get(4),
        audio_path: row.get(5),
        output_mode: row.get(6),
    }))
}

async fn delete_transcription(pool: &SqlitePool, id: i64) -> Result<bool> {
    let result = sqlx::query("DELETE FROM transcriptions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn search_transcriptions(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<Transcription>> {
    let pattern = format!("%{}%", query);
    let rows = sqlx::query(
        r#"
        SELECT id, text, created_at, duration_ms, model_name, audio_path, output_mode
        FROM transcriptions
        WHERE text LIKE ?
        ORDER BY created_at DESC
        LIMIT ?
        "#,
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Transcription {
            id: row.get(0),
            text: row.get(1),
            created_at: row.get(2),
            duration_ms: row.get(3),
            model_name: row.get(4),
            audio_path: row.get(5),
            output_mode: row.get(6),
        })
        .collect())
}

async fn count_transcriptions(pool: &SqlitePool) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) FROM transcriptions")
        .fetch_one(pool)
        .await?;
    Ok(row.get(0))
}

// ============================================================================
// Text Output
// ============================================================================

/// Detected display server
enum DisplayServer {
    Wayland,
    X11,
    Unknown,
}

impl DisplayServer {
    fn detect() -> Self {
        if env::var("WAYLAND_DISPLAY").is_ok()
            || env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        {
            return Self::Wayland;
        }

        if env::var("DISPLAY").is_ok() {
            return Self::X11;
        }

        Self::Unknown
    }
}

/// Insert text at cursor position using display server tools
fn insert_text(text: &str) -> Result<()> {
    let (tool, args) = match DisplayServer::detect() {
        DisplayServer::Wayland => ("wtype", vec![text.to_string()]),
        DisplayServer::X11 => ("xdotool", vec!["type".into(), "--".into(), text.into()]),
        DisplayServer::Unknown => return Err(anyhow::anyhow!("Unknown display server")),
    };

    // Check tool exists
    if !Command::new("which")
        .arg(tool)
        .output()?
        .status
        .success()
    {
        return Err(anyhow::anyhow!("{} not found", tool));
    }

    let output = Command::new(tool).args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("{} failed: {}", tool, stderr));
    }

    Ok(())
}

/// Copy text to clipboard
fn copy_to_clipboard(app: &AppHandle, text: &str) -> Result<()> {
    app.clipboard()
        .write_text(text.to_string())
        .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))
}

/// Handle text output based on mode
fn output_text(app: &AppHandle, text: &str, mode: OutputMode) {
    match mode {
        OutputMode::Print => println!("{}", text),
        OutputMode::Copy => {
            if let Err(e) = copy_to_clipboard(app, text) {
                eprintln!("[output] Clipboard failed: {}, falling back to print", e);
                println!("{}", text);
            }
        }
        OutputMode::Insert => {
            if let Err(e) = insert_text(text) {
                eprintln!("[output] Insert failed: {}, falling back to print", e);
                println!("{}", text);
            }
        }
    }
}

// ============================================================================
// Recording Workflow
// ============================================================================

async fn start_recording(
    recording: &RecordingState,
    settings: &SettingsState,
    broadcast: &Broadcaster,
) -> Result<()> {
    let config = settings.get().await;

    // Create spectrum channel for OSD visualization
    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();

    let session = RecordingSession::start(
        config.audio_device.as_deref(),
        config.sample_rate,
        Some(spectrum_tx),
    )?;

    // Forward spectrum data to broadcast
    let bc = broadcast.clone();
    let start = std::time::Instant::now();
    tokio::spawn(async move {
        while let Some(bands) = spectrum_rx.recv().await {
            let ts = start.elapsed().as_millis() as u64;
            bc.status(RecordingStatus::Recording, Some(bands.to_vec()), ts);
        }
    });

    recording.start(session).await;
    eprintln!("[recording] Started");

    Ok(())
}

async fn stop_and_transcribe(
    recording: &RecordingState,
    transcription: &TranscriptionState,
    settings: &SettingsState,
    broadcast: &Broadcaster,
    db: Option<&Database>,
    app: &AppHandle,
) -> Result<()> {
    // Stop recording
    let session = recording.stop().await.ok_or_else(|| anyhow::anyhow!("Not recording"))?;
    let elapsed_ms = session.elapsed_ms() as i64;

    // Small delay for final samples
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let samples = session.stop();

    if samples.is_empty() {
        recording.finish().await;
        return Err(anyhow::anyhow!("No audio recorded"));
    }

    eprintln!("[recording] Stopped, {} samples", samples.len());

    // Save WAV file
    let recordings_dir = {
        let dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?;
        let dir = dirs.data_dir().join("recordings");
        tokio::fs::create_dir_all(&dir).await?;
        dir
    };

    let timestamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    let audio_path = recordings_dir.join(format!("{}.wav", timestamp));
    audio::write_wav(&samples, &audio_path, 16000)?;

    eprintln!("[recording] Saved: {}", audio_path.display());

    // Transcribe
    let config = settings.get().await;
    transcription.ensure_loaded(&config).await?;

    let text = {
        let mut engine = transcription.engine().await;
        engine.transcribe(&audio_path)?
    };

    eprintln!("[transcription] Result: {}", text);

    // Broadcast result
    broadcast.result(text.clone(), elapsed_ms as f32 / 1000.0);

    // Save to database
    if !text.trim().is_empty() {
        if let Some(database) = db {
            let model_name = {
                let engine = transcription.engine().await;
                engine.loaded_model().map(|m| m.storage_name().to_string())
            };

            if let Err(e) = save_transcription(
                database.pool(),
                &text,
                Some(elapsed_ms),
                model_name.as_deref(),
                Some(&audio_path.to_string_lossy()),
                Some(config.output_mode.as_str()),
            )
            .await
            {
                eprintln!("[database] Save failed: {}", e);
            }
        }
    }

    // Output text
    output_text(app, &text, config.output_mode);

    // Return to idle
    recording.finish().await;
    broadcast.status(RecordingStatus::Idle, None, 0);

    Ok(())
}

// ============================================================================
// CLI
// ============================================================================

#[derive(Debug, Parser)]
#[command(name = "dictate", about = "Voice transcription for Linux", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Subcommand, Clone, Copy)]
enum CliCommand {
    /// Toggle recording
    Toggle,
    /// Start recording
    Start,
    /// Stop recording
    Stop,
}

#[cfg(desktop)]
fn parse_cli() -> Option<CliCommand> {
    Cli::parse().command
}

fn handle_cli_command(app: &AppHandle, command: CliCommand) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let recording = app.state::<RecordingState>();
        let transcription = app.state::<TranscriptionState>();
        let settings = app.state::<SettingsState>();
        let broadcast = app.state::<Broadcaster>();
        let db = app.try_state::<Database>();

        let result = match command {
            CliCommand::Toggle => {
                let status = recording.status().await;
                match status {
                    RecordingStatus::Idle => {
                        start_recording(&recording, &settings, &broadcast).await
                    }
                    RecordingStatus::Recording => {
                        stop_and_transcribe(
                            &recording,
                            &transcription,
                            &settings,
                            &broadcast,
                            db.as_deref(),
                            &app,
                        )
                        .await
                    }
                    _ => Err(anyhow::anyhow!("Busy")),
                }
            }
            CliCommand::Start => {
                if recording.status().await == RecordingStatus::Idle {
                    start_recording(&recording, &settings, &broadcast).await
                } else {
                    Err(anyhow::anyhow!("Already recording"))
                }
            }
            CliCommand::Stop => {
                if recording.status().await == RecordingStatus::Recording {
                    stop_and_transcribe(
                        &recording,
                        &transcription,
                        &settings,
                        &broadcast,
                        db.as_deref(),
                        &app,
                    )
                    .await
                } else {
                    Err(anyhow::anyhow!("Not recording"))
                }
            }
        };

        if let Err(e) = result {
            eprintln!("[cli] Command failed: {}", e);
        }
    });
}

fn handle_second_instance(app: &AppHandle, args: Vec<String>) {
    eprintln!("[cli] Second instance: {:?}", args);

    if let Ok(cli) = Cli::try_parse_from(args) {
        if let Some(command) = cli.command {
            handle_cli_command(app, command);
        }
    }
}

// ============================================================================
// System Tray
// ============================================================================

fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::new()
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

// ============================================================================
// Tauri Commands
// ============================================================================

// --- Recording ---

#[tauri::command]
async fn toggle_recording(
    recording: State<'_, RecordingState>,
    transcription: State<'_, TranscriptionState>,
    settings: State<'_, SettingsState>,
    broadcast: State<'_, Broadcaster>,
    app: AppHandle,
) -> Result<String, String> {
    let status = recording.status().await;

    match status {
        RecordingStatus::Idle => {
            let app_clone = app.clone();
            tokio::spawn(async move {
                let recording = app_clone.state::<RecordingState>();
                let settings = app_clone.state::<SettingsState>();
                let broadcast = app_clone.state::<Broadcaster>();

                if let Err(e) = start_recording(&recording, &settings, &broadcast).await {
                    eprintln!("[command] Start failed: {}", e);
                }
            });
            Ok("started".into())
        }
        RecordingStatus::Recording => {
            broadcast
                .status(RecordingStatus::Transcribing, None, recording.elapsed_ms().await);

            let app_clone = app.clone();
            tokio::spawn(async move {
                let recording = app_clone.state::<RecordingState>();
                let transcription = app_clone.state::<TranscriptionState>();
                let settings = app_clone.state::<SettingsState>();
                let broadcast = app_clone.state::<Broadcaster>();
                let db = app_clone.try_state::<Database>();

                if let Err(e) = stop_and_transcribe(
                    &recording,
                    &transcription,
                    &settings,
                    &broadcast,
                    db.as_deref(),
                    &app_clone,
                )
                .await
                {
                    eprintln!("[command] Transcribe failed: {}", e);
                }
            });
            Ok("stopping".into())
        }
        _ => Ok("busy".into()),
    }
}

#[tauri::command]
async fn get_status(recording: State<'_, RecordingState>) -> Result<String, String> {
    Ok(recording.status().await.as_str().into())
}

// --- Settings ---

#[tauri::command]
async fn get_output_mode(settings: State<'_, SettingsState>) -> Result<String, String> {
    Ok(settings.get().await.output_mode.as_str().into())
}

#[tauri::command]
async fn set_output_mode(settings: State<'_, SettingsState>, mode: String) -> Result<String, String> {
    let parsed = OutputMode::parse(&mode).ok_or_else(|| format!("Invalid mode: {}", mode))?;
    settings.set_output_mode(parsed).await.map_err(|e| e.to_string())?;
    Ok(format!("Output mode set to: {}", mode))
}

#[tauri::command]
fn get_version() -> String {
    format!("{}-{}", env!("CARGO_PKG_VERSION"), env!("GIT_SHA"))
}

#[tauri::command]
async fn check_config_changed(settings: State<'_, SettingsState>) -> Result<bool, String> {
    Ok(settings.has_external_changes().await)
}

#[tauri::command]
async fn mark_config_synced(settings: State<'_, SettingsState>) -> Result<(), String> {
    settings.mark_synced().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_window_decorations(settings: State<'_, SettingsState>) -> Result<bool, String> {
    Ok(settings.get().await.window_decorations)
}

#[tauri::command]
async fn set_window_decorations(
    settings: State<'_, SettingsState>,
    app: AppHandle,
    enabled: bool,
) -> Result<String, String> {
    settings.set_window_decorations(enabled).await.map_err(|e| e.to_string())?;

    if let Some(window) = app.get_webview_window("main") {
        window.set_decorations(enabled).map_err(|e| e.to_string())?;
    }

    Ok(format!("Window decorations: {}", enabled))
}

#[tauri::command]
async fn get_osd_position(settings: State<'_, SettingsState>) -> Result<String, String> {
    Ok(settings.get().await.osd_position.as_str().into())
}

#[tauri::command]
async fn set_osd_position(
    settings: State<'_, SettingsState>,
    broadcast: State<'_, Broadcaster>,
    position: String,
) -> Result<String, String> {
    let parsed = OsdPosition::parse(&position).ok_or_else(|| format!("Invalid position: {}", position))?;
    settings.set_osd_position(parsed).await.map_err(|e| e.to_string())?;
    broadcast.osd_position(parsed);
    Ok(format!("OSD position: {}", position))
}

// --- Audio ---

#[tauri::command]
async fn list_audio_devices() -> Result<Vec<AudioDevice>, String> {
    audio::list_devices().map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_audio_device(settings: State<'_, SettingsState>) -> Result<Option<String>, String> {
    Ok(settings.get().await.audio_device)
}

#[tauri::command]
async fn set_audio_device(
    settings: State<'_, SettingsState>,
    device_name: Option<String>,
) -> Result<String, String> {
    // Validate device exists
    if let Some(ref name) = device_name {
        let devices = audio::list_devices().map_err(|e| e.to_string())?;
        if !devices.iter().any(|d| &d.name == name) {
            return Err(format!("Device not found: {}", name));
        }
    }

    settings.set_audio_device(device_name.clone()).await.map_err(|e| e.to_string())?;

    Ok(match device_name {
        Some(name) => format!("Audio device: {}", name),
        None => "Audio device: system default".into(),
    })
}

#[tauri::command]
async fn get_sample_rate(settings: State<'_, SettingsState>) -> Result<u32, String> {
    Ok(settings.get().await.sample_rate)
}

#[tauri::command]
async fn get_sample_rate_options() -> Result<Vec<audio::SampleRateOption>, String> {
    Ok(audio::sample_rate_options())
}

#[tauri::command]
async fn set_sample_rate(settings: State<'_, SettingsState>, rate: u32) -> Result<String, String> {
    audio::validate_sample_rate(rate).map_err(|e| e.to_string())?;
    settings.set_sample_rate(rate).await.map_err(|e| e.to_string())?;
    Ok(format!("Sample rate: {} Hz", rate))
}

#[tauri::command]
async fn test_audio_device(device_name: Option<String>) -> Result<bool, String> {
    let config = Settings::load();
    RecordingSession::start(device_name.as_deref(), config.sample_rate, None)
        .map(|_| true)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_audio_level(device_name: Option<String>) -> Result<f32, String> {
    let config = Settings::load();
    audio::get_audio_level(device_name.as_deref(), config.sample_rate).map_err(|e| e.to_string())
}

// --- Models ---

#[tauri::command]
async fn list_models() -> Result<Vec<ModelInfo>, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    Ok(manager.list_models())
}

#[tauri::command]
async fn get_model_storage_info() -> Result<StorageInfo, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    manager.storage_info().map_err(|e| e.to_string())
}

#[tauri::command]
async fn download_model(model: ModelId, broadcast: State<'_, Broadcaster>) -> Result<(), String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    let bc = broadcast.inner().clone();

    manager
        .download(model, move |downloaded, total, phase| {
            bc.model_progress(model, downloaded, total, phase);
        })
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_model(model: ModelId) -> Result<(), String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    manager.remove(model).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_preferred_model(settings: State<'_, SettingsState>) -> Result<Option<ModelId>, String> {
    Ok(settings.get().await.preferred_model)
}

#[tauri::command]
async fn set_preferred_model(
    settings: State<'_, SettingsState>,
    model: Option<ModelId>,
) -> Result<(), String> {
    settings.set_preferred_model(model).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_model_sizes() -> Result<Vec<(ModelId, u64)>, String> {
    let manager = ModelManager::new().map_err(|e| e.to_string())?;
    let sizes = manager.fetch_model_sizes().await.map_err(|e| e.to_string())?;
    Ok(sizes.into_iter().collect())
}

// --- History ---

#[tauri::command]
async fn get_transcription_history(
    db: State<'_, Database>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Transcription>, String> {
    list_transcriptions(db.pool(), limit.unwrap_or(50), offset.unwrap_or(0))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_transcription_by_id(db: State<'_, Database>, id: i64) -> Result<Option<Transcription>, String> {
    get_transcription(db.pool(), id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_transcription_by_id(db: State<'_, Database>, id: i64) -> Result<bool, String> {
    delete_transcription(db.pool(), id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn search_transcription_history(
    db: State<'_, Database>,
    query: String,
    limit: Option<i64>,
) -> Result<Vec<Transcription>, String> {
    search_transcriptions(db.pool(), &query, limit.unwrap_or(50))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_transcription_count(db: State<'_, Database>) -> Result<i64, String> {
    count_transcriptions(db.pool()).await.map_err(|e| e.to_string())
}

// ============================================================================
// Application Entry Point
// ============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(desktop)]
    let initial_command = parse_cli();

    #[cfg(not(desktop))]
    let initial_command: Option<CliCommand> = None;

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            handle_second_instance(app, args);
        }))
        .setup(move |app| {
            // Create tray
            create_tray(app.handle())?;

            // Initialize state
            let settings = SettingsState::new();
            let broadcast = Broadcaster::new();

            app.manage(RecordingState::new());
            app.manage(TranscriptionState::new());
            app.manage(settings);
            app.manage(broadcast.clone());

            // Bridge broadcast to Tauri events
            broadcast.bridge_to_tauri(app.handle().clone());

            // Initialize database async
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    match init_database().await {
                        Ok(pool) => {
                            app_handle.manage(Database::new(pool));
                            eprintln!("[setup] Database initialized");
                        }
                        Err(e) => eprintln!("[setup] Database failed: {}", e),
                    }
                });
            }

            // Apply window decorations
            {
                let settings: State<SettingsState> = app.state();
                if let Some(window) = app.get_webview_window("main") {
                    tauri::async_runtime::block_on(async {
                        let config = settings.get().await;
                        let _ = window.set_decorations(config.window_decorations);
                    });
                }
            }

            // Handle initial CLI command
            #[cfg(desktop)]
            if let Some(command) = initial_command {
                handle_cli_command(app.handle(), command);
            }

            // Spawn OSD
            {
                let settings: State<SettingsState> = app.state();
                let broadcast: State<Broadcaster> = app.state();
                let rx = broadcast.subscribe();
                let position = tauri::async_runtime::block_on(async { settings.get().await.osd_position });

                std::thread::spawn(move || {
                    if let Err(e) = osd::run(rx, position) {
                        eprintln!("[setup] OSD failed: {}", e);
                    }
                });
            }

            // Preload transcription model
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        let transcription = app_handle.state::<TranscriptionState>();
                        let settings = app_handle.state::<SettingsState>();
                        let config = settings.get().await;

                        if let Err(e) = transcription.ensure_loaded(&config).await {
                            eprintln!("[setup] Model preload failed: {}", e);
                        } else {
                            eprintln!("[setup] Model preloaded");
                        }
                    });
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Recording
            toggle_recording,
            get_status,
            // Settings
            get_output_mode,
            set_output_mode,
            get_version,
            check_config_changed,
            mark_config_synced,
            get_window_decorations,
            set_window_decorations,
            get_osd_position,
            set_osd_position,
            // Audio
            list_audio_devices,
            get_audio_device,
            set_audio_device,
            get_sample_rate,
            get_sample_rate_options,
            set_sample_rate,
            test_audio_device,
            get_audio_level,
            // Models
            list_models,
            get_model_storage_info,
            download_model,
            remove_model,
            get_preferred_model,
            set_preferred_model,
            get_model_sizes,
            // History
            get_transcription_history,
            get_transcription_by_id,
            delete_transcription_by_id,
            search_transcription_history,
            get_transcription_count,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                #[cfg(desktop)]
                {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
