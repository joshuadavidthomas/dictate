# Backend Simplification Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Consolidate 22+ backend files into 11 files with clear responsibilities, eliminating 3x code duplication.

**Architecture:** Create `recording.rs` (capture + shortcuts + state), expand `transcription.rs` (+ output delivery), flatten `commands/` directory, dissolve `platform/` hierarchy.

**Tech Stack:** Rust, Tauri 2, CPAL audio, tokio async, SQLite

---

## Phase 1: Create recording.rs (New File)

### Task 1.1: Create recording.rs with state types

**Files:**
- Create: `src-tauri/src/recording.rs`
- Reference: `src-tauri/src/state.rs:1-118` (RecordingState, RecordingPhase, RecordingSnapshot)

**Step 1: Create recording.rs with state types moved from state.rs**

```rust
//! Recording pipeline: capture → audio file
//!
//! Handles audio capture, shortcuts, and the recording state machine.
//! Produces audio files that are consumed by transcription.rs.

use crate::transcription::TranscriptionEngine;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tokio::sync::Mutex;

// ============================================================================
// State Machine
// ============================================================================

/// Broadcastable snapshot of recording state
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RecordingSnapshot {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl RecordingSnapshot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Recording => "Recording",
            Self::Transcribing => "Transcribing",
            Self::Error => "Error",
        }
    }
}

/// Internal recording phase with associated data
enum RecordingPhase {
    Idle,
    Recording {
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
        stream: cpal::Stream,
        start_time: Instant,
    },
    Transcribing,
}

/// Manages the current recording state
pub struct RecordingState(Mutex<RecordingPhase>);

impl RecordingState {
    pub fn new() -> Self {
        Self(Mutex::new(RecordingPhase::Idle))
    }

    pub async fn start_recording(
        &self,
        stream: cpal::Stream,
        audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
    ) {
        let mut phase = self.0.lock().await;
        *phase = RecordingPhase::Recording {
            audio_buffer,
            stop_signal,
            stream,
            start_time: Instant::now(),
        };
    }

    pub async fn stop_recording(&self) -> Option<Arc<std::sync::Mutex<Vec<i16>>>> {
        let mut phase = self.0.lock().await;
        if let RecordingPhase::Recording {
            audio_buffer,
            stop_signal,
            stream,
            ..
        } = std::mem::replace(&mut *phase, RecordingPhase::Transcribing)
        {
            stop_signal.store(true, std::sync::atomic::Ordering::Release);
            drop(stream);
            Some(audio_buffer)
        } else {
            None
        }
    }

    pub async fn finish_transcription(&self) {
        let mut phase = self.0.lock().await;
        *phase = RecordingPhase::Idle;
    }

    pub async fn snapshot(&self) -> RecordingSnapshot {
        let phase = self.0.lock().await;
        match &*phase {
            RecordingPhase::Idle => RecordingSnapshot::Idle,
            RecordingPhase::Recording { .. } => RecordingSnapshot::Recording,
            RecordingPhase::Transcribing => RecordingSnapshot::Transcribing,
        }
    }

    pub async fn elapsed_ms(&self) -> u64 {
        let phase = self.0.lock().await;
        if let RecordingPhase::Recording { start_time, .. } = &*phase {
            start_time.elapsed().as_millis() as u64
        } else {
            0
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check 2>&1 | head -20`

Expected: Compiles (may have unused warnings)

**Step 3: Commit**

```bash
git add src-tauri/src/recording.rs
git commit -m "feat(recording): create recording.rs with state types"
```

---

### Task 1.2: Add display server detection to recording.rs

**Files:**
- Modify: `src-tauri/src/recording.rs`
- Reference: `src-tauri/src/platform/linux/display.rs:1-127`

**Step 1: Add display detection code to recording.rs**

Append to `recording.rs`:

```rust
// ============================================================================
// Display Server Detection
// ============================================================================

use std::env;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayServer {
    Wayland,
    X11,
    Unknown,
}

impl DisplayServer {
    pub fn detect() -> Self {
        if env::var("WAYLAND_DISPLAY").is_ok()
            || env::var("XDG_SESSION_TYPE")
                .as_ref()
                .map(|s| s.as_str())
                == Ok("wayland")
        {
            return DisplayServer::Wayland;
        }

        if env::var("DISPLAY").is_ok() {
            return DisplayServer::X11;
        }

        if env::var("XDG_SESSION_TYPE")
            .as_ref()
            .map(|s| s.to_lowercase())
            == Ok("x11".to_string())
        {
            return DisplayServer::X11;
        }

        if let Ok(output) = Command::new("ps").args(["-e"]).output() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            if output_str.contains("wayland") || output_str.contains("wlroots") {
                return DisplayServer::Wayland;
            }
            if output_str.contains("Xorg") || output_str.contains("Xwayland") {
                return DisplayServer::X11;
            }
        }

        DisplayServer::Unknown
    }
}

pub fn has_global_shortcuts_portal() -> bool {
    let output = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.DBus.Introspectable",
            "Introspect",
        ])
        .output();

    if let Ok(output) = output {
        let result = String::from_utf8_lossy(&output.stdout);
        return result.contains("org.freedesktop.portal.GlobalShortcuts");
    }

    false
}

pub fn detect_compositor() -> Option<String> {
    if env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        return Some("hyprland".to_string());
    }

    if let Ok(desktop) = env::var("XDG_CURRENT_DESKTOP") {
        let lower = desktop.to_lowercase();
        if lower.contains("hyprland") {
            return Some("hyprland".to_string());
        } else if lower.contains("sway") {
            return Some("sway".to_string());
        } else if lower.contains("gnome") {
            return Some("gnome".to_string());
        } else if lower.contains("kde") || lower.contains("plasma") {
            return Some("kde".to_string());
        }
        return Some(lower);
    }

    None
}
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check 2>&1 | head -20`

**Step 3: Commit**

```bash
git add src-tauri/src/recording.rs
git commit -m "feat(recording): add display server detection"
```

---

### Task 1.3: Add shortcut backend trait and implementations to recording.rs

**Files:**
- Modify: `src-tauri/src/recording.rs`
- Reference: `src-tauri/src/platform/linux/shortcuts.rs`, `src-tauri/src/platform/linux/shortcuts/*.rs`

**Step 1: Add shortcut trait and implementations**

Append to `recording.rs`:

```rust
// ============================================================================
// Shortcut Backends
// ============================================================================

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use tauri::AppHandle;

pub const SHORTCUT_ID: &str = "toggle";
pub const SHORTCUT_DESCRIPTION: &str = "Toggle Recording";

#[derive(Debug, Clone, Serialize)]
pub enum ShortcutPlatform {
    X11,
    WaylandPortal,
    WaylandFallback,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendCapabilities {
    pub platform: ShortcutPlatform,
    pub can_register: bool,
    pub compositor: Option<String>,
}

pub trait ShortcutBackend: Send + Sync {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    fn capabilities(&self) -> BackendCapabilities;
}

pub fn detect_platform() -> ShortcutPlatform {
    match DisplayServer::detect() {
        DisplayServer::Wayland => {
            if has_global_shortcuts_portal() {
                ShortcutPlatform::WaylandPortal
            } else {
                ShortcutPlatform::WaylandFallback
            }
        }
        DisplayServer::X11 => ShortcutPlatform::X11,
        DisplayServer::Unknown => ShortcutPlatform::Unsupported,
    }
}

pub fn create_backend(app: AppHandle) -> Box<dyn ShortcutBackend> {
    let platform = detect_platform();
    match platform {
        ShortcutPlatform::X11 => Box::new(X11Backend::new(app)),
        ShortcutPlatform::WaylandPortal => Box::new(WaylandPortalBackend::new(app)),
        ShortcutPlatform::WaylandFallback | ShortcutPlatform::Unsupported => {
            Box::new(FallbackBackend::new())
        }
    }
}

/// Manages keyboard shortcuts state
pub struct ShortcutState {
    backend: Arc<Mutex<Option<Box<dyn ShortcutBackend>>>>,
}

impl ShortcutState {
    pub fn new() -> Self {
        Self {
            backend: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn set_backend(&self, backend: Box<dyn ShortcutBackend>) {
        *self.backend.lock().await = Some(backend);
    }

    pub async fn backend(&self) -> tokio::sync::MutexGuard<'_, Option<Box<dyn ShortcutBackend>>> {
        self.backend.lock().await
    }
}

// --- X11 Backend ---

use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

pub struct X11Backend {
    app: AppHandle,
}

impl X11Backend {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    async fn register_impl(&self, shortcut: &str) -> Result<()> {
        let parsed = shortcut
            .parse::<Shortcut>()
            .map_err(|e| anyhow::anyhow!("Invalid shortcut format: {}", e))?;

        let app_handle = self.app.clone();

        self.app
            .global_shortcut()
            .on_shortcut(parsed, move |_app, _shortcut, _event| {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::recording::toggle_recording(&app).await {
                        eprintln!("[shortcut] toggle_recording failed: {}", e);
                    }
                });
            })
            .map_err(|e| anyhow::anyhow!("Failed to register shortcut: {}", e))?;

        Ok(())
    }
}

impl ShortcutBackend for X11Backend {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let shortcut = shortcut.to_string();
        Box::pin(async move { self.register_impl(&shortcut).await })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::X11,
            can_register: true,
            compositor: detect_compositor(),
        }
    }
}

// --- Wayland Portal Backend ---

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

pub struct WaylandPortalBackend {
    app: AppHandle,
    proxy: Arc<Mutex<Option<GlobalShortcuts<'static>>>>,
    session: Arc<Mutex<Option<ashpd::desktop::Session<'static, GlobalShortcuts<'static>>>>>,
    listener_started: Arc<Mutex<bool>>,
}

impl WaylandPortalBackend {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            proxy: Arc::new(Mutex::new(None)),
            session: Arc::new(Mutex::new(None)),
            listener_started: Arc::new(Mutex::new(false)),
        }
    }

    fn convert_shortcut_format(shortcut: &str) -> String {
        let mut result = String::new();
        let parts: Vec<&str> = shortcut.split('+').collect();

        for (i, part) in parts.iter().enumerate() {
            let normalized = match part.trim() {
                "CommandOrControl" | "Ctrl" | "Control" => "<Control>",
                "Command" | "Super" | "Meta" => "<Super>",
                "Alt" => "<Alt>",
                "Shift" => "<Shift>",
                key => {
                    if i == parts.len() - 1 {
                        &key.to_lowercase()
                    } else {
                        continue;
                    }
                }
            };
            result.push_str(normalized);
        }

        result
    }

    async fn register_impl(&self, shortcut: &str) -> Result<()> {
        use anyhow::Context;
        
        let portal_shortcut = Self::convert_shortcut_format(shortcut);

        let mut proxy_guard = self.proxy.lock().await;
        if proxy_guard.is_none() {
            let proxy = GlobalShortcuts::new()
                .await
                .context("Failed to create GlobalShortcuts proxy")?;
            *proxy_guard = Some(proxy);
        }
        let proxy = proxy_guard.as_ref().unwrap();

        let mut session_guard = self.session.lock().await;
        if session_guard.is_none() {
            let session = proxy
                .create_session()
                .await
                .context("Failed to create session")?;
            *session_guard = Some(session);
        }
        let session = session_guard.as_ref().unwrap();

        let new_shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION)
            .preferred_trigger(Some(portal_shortcut.as_str()));

        let request = proxy
            .bind_shortcuts(session, &[new_shortcut], None)
            .await
            .context("Failed to create bind request")?;

        request
            .response()
            .context("Failed to get portal response")?;

        drop(session_guard);
        drop(proxy_guard);

        self.start_listener_if_needed().await;

        Ok(())
    }

    async fn start_listener_if_needed(&self) {
        let mut listener_started = self.listener_started.lock().await;
        if *listener_started {
            return;
        }
        *listener_started = true;

        let app_handle = self.app.clone();
        tokio::spawn(async move {
            let Ok(listener_proxy) = GlobalShortcuts::new().await else {
                return;
            };

            let Ok(mut stream) = listener_proxy.receive_activated().await else {
                return;
            };

            while let Some(_activated) = stream.next().await {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::recording::toggle_recording(&app).await {
                        eprintln!("[shortcut] toggle_recording failed: {}", e);
                    }
                });
            }
        });
    }
}

impl ShortcutBackend for WaylandPortalBackend {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let shortcut = shortcut.to_string();
        Box::pin(async move { self.register_impl(&shortcut).await })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut proxy_guard = self.proxy.lock().await;
            *proxy_guard = None;

            let mut session_guard = self.session.lock().await;
            if let Some(session) = session_guard.take() {
                drop(session);
            }

            Ok(())
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::WaylandPortal,
            can_register: true,
            compositor: detect_compositor(),
        }
    }
}

// --- Fallback Backend ---

pub struct FallbackBackend;

impl FallbackBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FallbackBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ShortcutBackend for FallbackBackend {
    fn register(&self, _shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            platform: ShortcutPlatform::WaylandFallback,
            can_register: false,
            compositor: detect_compositor(),
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check 2>&1 | head -30`

**Step 3: Commit**

```bash
git add src-tauri/src/recording.rs
git commit -m "feat(recording): add shortcut backend trait and implementations"
```

---

### Task 1.4: Add audio capture and spectrum analysis to recording.rs

**Files:**
- Modify: `src-tauri/src/recording.rs`
- Reference: `src-tauri/src/audio/recorder.rs`, `src-tauri/src/audio/spectrum.rs`

**Step 1: Add audio capture code**

This is a large addition (~600 lines from recorder.rs + spectrum.rs). Append the contents of both files to `recording.rs`, adjusting module paths.

Key imports to add at top:
```rust
use anyhow::anyhow;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, StreamConfig};
use hound::{WavSpec, WavWriter};
use rustfft::{FftPlanner, num_complex::Complex};
use std::path::Path;
use std::sync::atomic::Ordering;
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Run existing tests**

Run: `cd src-tauri && cargo test`

Expected: 18 tests pass (spectrum tests now in recording.rs)

**Step 4: Commit**

```bash
git add src-tauri/src/recording.rs
git commit -m "feat(recording): add audio capture and spectrum analysis"
```

---

### Task 1.5: Add toggle_recording entry point to recording.rs

**Files:**
- Modify: `src-tauri/src/recording.rs`
- Reference: `src-tauri/src/audio/recording.rs`, `src-tauri/src/commands/recording.rs`

**Step 1: Add the unified toggle_recording function**

Append to `recording.rs`:

```rust
// ============================================================================
// Public API
// ============================================================================

use crate::broadcast::BroadcastServer;
use crate::conf::SettingsState;
use crate::db::Database;
use directories::ProjectDirs;

/// Result of stopping a recording
pub struct RecordedAudio {
    pub buffer: Vec<i16>,
    pub path: std::path::PathBuf,
    pub sample_rate: u32,
}

/// Toggle recording state - the main entry point
/// 
/// - If idle: starts recording
/// - If recording: stops, transcribes, and delivers output
/// - If transcribing: returns busy
pub async fn toggle_recording(app: &AppHandle) -> Result<String> {
    let recording: tauri::State<RecordingState> = app.state();
    let snapshot = recording.snapshot().await;

    match snapshot {
        RecordingSnapshot::Idle => {
            start_recording(app).await?;
            Ok("started".into())
        }
        RecordingSnapshot::Recording => {
            let broadcast: tauri::State<BroadcastServer> = app.state();
            broadcast
                .recording_status(
                    RecordingSnapshot::Transcribing,
                    None,
                    false,
                    recording.elapsed_ms().await,
                )
                .await;

            let app_clone = app.clone();
            tokio::spawn(async move {
                if let Err(e) = complete_recording(&app_clone).await {
                    eprintln!("[toggle_recording] Failed to complete recording: {}", e);
                    
                    let recording: tauri::State<RecordingState> = app_clone.state();
                    let broadcast: tauri::State<BroadcastServer> = app_clone.state();
                    recording.finish_transcription().await;
                    broadcast
                        .recording_status(RecordingSnapshot::Error, None, false, 0)
                        .await;
                }
            });

            Ok("stopping".into())
        }
        RecordingSnapshot::Transcribing | RecordingSnapshot::Error => Ok("busy".into()),
    }
}

async fn start_recording(app: &AppHandle) -> Result<()> {
    let settings: tauri::State<SettingsState> = app.state();
    let recording: tauri::State<RecordingState> = app.state();
    let broadcast: tauri::State<BroadcastServer> = app.state();

    let settings_data = settings.get().await;
    let device_name = settings_data.audio_device.clone();
    let sample_rate = settings_data.sample_rate;

    let recorder = AudioRecorder::new_with_device(device_name.as_deref(), sample_rate)?;

    let audio_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stop_signal = Arc::new(AtomicBool::new(false));

    let (spectrum_tx, mut spectrum_rx) = tokio::sync::mpsc::unbounded_channel();

    let stream = recorder.start_recording_background(
        audio_buffer.clone(),
        stop_signal.clone(),
        Some(spectrum_tx),
    )?;

    stream.play().map_err(|e| anyhow!("Failed to play stream: {}", e))?;

    // Spawn spectrum broadcaster
    let broadcast_clone = broadcast.inner().clone();
    let start_time = std::time::Instant::now();
    tokio::spawn(async move {
        while let Some(spectrum) = spectrum_rx.recv().await {
            let ts = start_time.elapsed().as_millis() as u64;
            broadcast_clone
                .recording_status(RecordingSnapshot::Recording, Some(spectrum), false, ts)
                .await;
        }
    });

    recording
        .start_recording(stream, audio_buffer, stop_signal)
        .await;

    eprintln!("[recording] Recording started");
    Ok(())
}

async fn stop_recording(app: &AppHandle) -> Result<RecordedAudio> {
    let recording: tauri::State<RecordingState> = app.state();

    let audio_buffer = recording
        .stop_recording()
        .await
        .ok_or_else(|| anyhow!("No active recording"))?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let buffer = audio_buffer.lock().unwrap().clone();

    if buffer.is_empty() {
        recording.finish_transcription().await;
        return Err(anyhow!("No audio recorded"));
    }

    eprintln!("[recording] Recorded {} samples", buffer.len());

    let recordings_dir = {
        let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
            .ok_or_else(|| anyhow!("Failed to get project directories"))?;
        let dir = project_dirs.data_dir().join("recordings");
        tokio::fs::create_dir_all(&dir).await?;
        dir
    };

    let timestamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H-%M-%S");
    let audio_path = recordings_dir.join(format!("{}.wav", timestamp));
    buffer_to_wav(&buffer, &audio_path, 16000)?;

    eprintln!("[recording] Audio saved to: {:?}", audio_path);

    Ok(RecordedAudio {
        buffer,
        path: audio_path,
        sample_rate: 16000,
    })
}

async fn complete_recording(app: &AppHandle) -> Result<()> {
    let recording: tauri::State<RecordingState> = app.state();
    let settings: tauri::State<SettingsState> = app.state();
    let broadcast: tauri::State<BroadcastServer> = app.state();
    let db = app.try_state::<Database>();

    // Step 1: Stop and get audio
    let recorded_audio = stop_recording(app).await?;

    // Step 2: Transcribe and deliver
    let transcription = crate::transcription::transcribe_and_deliver(
        &recorded_audio.path,
        &recorded_audio.buffer,
        recorded_audio.sample_rate,
        app,
    ).await?;

    // Step 3: Broadcast completion
    let duration_secs = transcription.duration_ms.unwrap_or(0) as f32 / 1000.0;
    let model = transcription
        .model_id
        .map(|id| format!("{:?}", id))
        .unwrap_or_else(|| "unknown".to_string());

    broadcast
        .transcription_result(transcription.text.clone(), duration_secs, model)
        .await;

    recording.finish_transcription().await;

    broadcast
        .recording_status(RecordingSnapshot::Idle, None, true, 0)
        .await;

    Ok(())
}
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add src-tauri/src/recording.rs
git commit -m "feat(recording): add toggle_recording entry point"
```

---

## Phase 2: Expand transcription.rs

### Task 2.1: Add output delivery to transcription.rs

**Files:**
- Modify: `src-tauri/src/transcription.rs`
- Reference: `src-tauri/src/conf.rs:28-46` (OutputMode::deliver), `src-tauri/src/platform/linux/text.rs`

**Step 1: Add output delivery code**

Add to `transcription.rs`:

```rust
// ============================================================================
// Output Delivery
// ============================================================================

use std::process::Command;
use tauri_plugin_clipboard_manager::ClipboardExt;

/// Insert text at cursor position using appropriate tool for display server
fn insert_text(text: &str) -> Result<()> {
    let display_server = crate::recording::DisplayServer::detect();
    
    let mut cmd = match display_server {
        crate::recording::DisplayServer::X11 => {
            let mut cmd = Command::new("xdotool");
            cmd.args(["type", "--"]).arg(text);
            cmd
        }
        crate::recording::DisplayServer::Wayland => {
            let mut cmd = Command::new("wtype");
            cmd.arg(text);
            cmd
        }
        crate::recording::DisplayServer::Unknown => {
            return Err(anyhow!("Unknown display server, cannot insert text"));
        }
    };

    let tool = cmd.get_program().to_string_lossy().to_string();
    if !Command::new("which").arg(&tool).output()?.status.success() {
        return Err(anyhow!("{} not found", tool));
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("{} failed: {}", tool, stderr));
    }

    Ok(())
}

fn deliver_output(text: &str, output_mode: crate::conf::OutputMode, app: &AppHandle) -> Result<()> {
    match output_mode {
        crate::conf::OutputMode::Print => {
            println!("{}", text);
            Ok(())
        }
        crate::conf::OutputMode::Copy => {
            app.clipboard()
                .write_text(text.to_string())
                .map_err(|e| anyhow!("Failed to write to clipboard: {}", e))
        }
        crate::conf::OutputMode::Insert => {
            insert_text(text)
        }
    }
}
```

**Step 2: Add public transcribe_and_deliver function**

```rust
/// Transcribe audio and deliver output - the main entry point
pub async fn transcribe_and_deliver(
    audio_path: &Path,
    audio_buffer: &[i16],
    sample_rate: u32,
    app: &AppHandle,
) -> Result<Transcription> {
    let transcription_state: tauri::State<TranscriptionState> = app.state();
    let settings: tauri::State<crate::conf::SettingsState> = app.state();
    let db = app.try_state::<crate::db::Database>();

    let context = TranscriptionContext {
        engine_state: &transcription_state,
        settings: &settings,
        database: db.as_deref(),
    };

    let transcription = Transcription::from_audio_path(
        audio_path,
        audio_buffer,
        sample_rate,
        context,
    ).await?;

    // Deliver output
    let output_mode = settings.get().await.output_mode;
    deliver_output(&transcription.text, output_mode, app)?;

    Ok(transcription)
}
```

**Step 3: Move TranscriptionState from state.rs**

Add to `transcription.rs`:

```rust
/// Manages transcription engine state
pub struct TranscriptionState {
    engine: Mutex<Option<TranscriptionEngine>>,
}

impl TranscriptionState {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(None),
        }
    }

    pub async fn engine(&self) -> tokio::sync::MutexGuard<'_, Option<TranscriptionEngine>> {
        self.engine.lock().await
    }
}
```

**Step 4: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 5: Commit**

```bash
git add src-tauri/src/transcription.rs
git commit -m "feat(transcription): add output delivery and transcribe_and_deliver entry point"
```

---

## Phase 3: Flatten commands.rs

### Task 3.1: Consolidate all commands into commands.rs

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Reference: `src-tauri/src/commands/*.rs` (5 files)

**Step 1: Replace commands.rs with consolidated version**

Replace the entire `commands.rs` file with all commands inlined. Remove the submodule imports and paste all command functions directly.

The file should:
1. Remove `mod audio; mod history; mod models; mod recording; mod settings;`
2. Remove `pub use audio::*; pub use history::*;` etc.
3. Inline all command functions from the 5 submodules
4. Update imports to use new module paths (`crate::recording::*`, `crate::transcription::*`)

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add src-tauri/src/commands.rs
git commit -m "refactor(commands): flatten all commands into single file"
```

---

### Task 3.2: Delete commands/ directory

**Files:**
- Delete: `src-tauri/src/commands/audio.rs`
- Delete: `src-tauri/src/commands/history.rs`
- Delete: `src-tauri/src/commands/models.rs`
- Delete: `src-tauri/src/commands/recording.rs`
- Delete: `src-tauri/src/commands/settings.rs`

**Step 1: Remove files**

```bash
rm -r src-tauri/src/commands/
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add -A
git commit -m "refactor(commands): delete commands/ directory"
```

---

## Phase 4: Update lib.rs and cli.rs

### Task 4.1: Update lib.rs module declarations

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Step 1: Update module declarations**

Replace:
```rust
mod audio;
mod platform;
mod state;
```

With:
```rust
mod recording;
```

Update state imports:
```rust
use crate::recording::{RecordingState, ShortcutState};
use crate::transcription::TranscriptionState;
```

Update shortcut creation:
```rust
let backend = crate::recording::create_backend(app_clone.clone());
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "refactor(lib): update module declarations for new structure"
```

---

### Task 4.2: Simplify cli.rs

**Files:**
- Modify: `src-tauri/src/cli.rs`

**Step 1: Replace duplicated orchestration with single call**

The CLI currently has the stop→transcribe→output flow duplicated twice. Replace with:

```rust
Command::Toggle => {
    if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
        eprintln!("[cli] toggle_recording failed: {}", e);
    }
}
Command::Start => {
    let recording = app_clone.state::<crate::recording::RecordingState>();
    if recording.snapshot().await == crate::recording::RecordingSnapshot::Idle {
        if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
            eprintln!("[cli] start failed: {}", e);
        }
    } else {
        eprintln!("[cli] Cannot start - already recording or transcribing");
    }
}
Command::Stop => {
    let recording = app_clone.state::<crate::recording::RecordingState>();
    if recording.snapshot().await == crate::recording::RecordingSnapshot::Recording {
        if let Err(e) = crate::recording::toggle_recording(&app_clone).await {
            eprintln!("[cli] stop failed: {}", e);
        }
    } else {
        eprintln!("[cli] Cannot stop - not currently recording");
    }
}
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add src-tauri/src/cli.rs
git commit -m "refactor(cli): use recording::toggle_recording, remove duplication"
```

---

## Phase 5: Remove OutputMode::deliver from conf.rs

### Task 5.1: Remove deliver method from OutputMode

**Files:**
- Modify: `src-tauri/src/conf.rs`

**Step 1: Remove the deliver method**

Delete lines 28-46 (the `deliver` method on `OutputMode`).

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add src-tauri/src/conf.rs
git commit -m "refactor(conf): remove OutputMode::deliver (moved to transcription.rs)"
```

---

## Phase 6: Delete old files

### Task 6.1: Delete dissolved modules

**Files:**
- Delete: `src-tauri/src/audio.rs`
- Delete: `src-tauri/src/audio/` directory
- Delete: `src-tauri/src/platform.rs`
- Delete: `src-tauri/src/platform/` directory
- Delete: `src-tauri/src/state.rs`

**Step 1: Remove files**

```bash
rm src-tauri/src/audio.rs
rm -r src-tauri/src/audio/
rm src-tauri/src/platform.rs
rm -r src-tauri/src/platform/
rm src-tauri/src/state.rs
```

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Run all tests**

Run: `cd src-tauri && cargo test`

Expected: All 18 tests pass (now running from recording.rs)

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor: delete dissolved modules (audio/, platform/, state.rs)"
```

---

## Phase 7: Simplify broadcast.rs

### Task 7.1: Remove TauriEvent duplication

**Files:**
- Modify: `src-tauri/src/broadcast.rs`

**Step 1: Remove TauriEvent enum and from_message conversion**

Delete `TauriEvent` enum (lines 11-53) and `TauriEvent::from_message` (lines 55-91).

Update `spawn_tauri_bridge` to emit `Message` directly with appropriate event names.

**Step 2: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 3: Commit**

```bash
git add src-tauri/src/broadcast.rs
git commit -m "refactor(broadcast): remove TauriEvent duplication, emit Message directly"
```

---

## Phase 8: Final Verification

### Task 8.1: Full build and test

**Step 1: Clean build**

Run: `cd src-tauri && cargo clean && cargo build`

**Step 2: Run all tests**

Run: `cd src-tauri && cargo test`

Expected: All tests pass

**Step 3: Check for warnings**

Run: `cd src-tauri && cargo clippy`

Fix any warnings.

**Step 4: Final commit**

```bash
git add -A
git commit -m "refactor: complete backend simplification"
```

---

## Summary

**Files after refactoring:**
```
src-tauri/src/
├── lib.rs
├── main.rs
├── recording.rs      # NEW (~800 lines)
├── transcription.rs  # EXPANDED (~500 lines)
├── models.rs         # Unchanged
├── conf.rs           # Simplified
├── broadcast.rs      # Simplified
├── commands.rs       # Flattened (~400 lines)
├── cli.rs            # Simplified
├── db.rs             # Unchanged
├── tray.rs           # Unchanged
├── osd.rs            # Unchanged
└── osd/              # Unchanged
```

**Verification checklist:**
- [ ] `cargo check` passes
- [ ] `cargo test` passes (18 tests)
- [ ] `cargo clippy` has no warnings
- [ ] App launches and recording works
- [ ] Global shortcuts work (X11 and Wayland)
- [ ] All three output modes work (print/copy/insert)
