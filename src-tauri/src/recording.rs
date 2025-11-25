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
