//! Recording pipeline: capture → audio file
//!
//! Handles audio capture, shortcuts, and the recording state machine.
//! Produces audio files that are consumed by transcription.rs.

use serde::{Deserialize, Serialize};
use std::env;
use std::process::Command;
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

// ============================================================================
// Display Server Detection
// ============================================================================

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

// ============================================================================
// Shortcut Backends
// ============================================================================

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use tauri::AppHandle;

pub const SHORTCUT_ID: &str = "toggle-recording";
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
