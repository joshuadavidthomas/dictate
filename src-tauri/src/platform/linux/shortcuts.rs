//! Keyboard shortcuts for Linux
//!
//! Provides global keyboard shortcut registration that works across both X11 and Wayland,
//! using the appropriate backend based on the detected display server and available portals.

mod fallback;
mod wayland;
mod x11;

pub use fallback::FallbackBackend;
pub use wayland::WaylandPortalBackend;
pub use x11::X11Backend;

use super::display::{has_global_shortcuts_portal, DisplayServer};
use anyhow::Result;
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;
use tauri::AppHandle;

pub const SHORTCUT_ID: &str = "toggle-recording";
pub const SHORTCUT_DESCRIPTION: &str = "Toggle Recording";

/// The detected shortcut platform type
#[derive(Debug, Clone, Serialize)]
pub enum ShortcutPlatform {
    X11,
    WaylandPortal,
    WaylandFallback,
    Unsupported,
}

/// Capabilities of the current shortcut backend
#[derive(Debug, Clone, Serialize)]
pub struct BackendCapabilities {
    pub platform: ShortcutPlatform,
    pub can_register: bool,
    pub compositor: Option<String>,
}

/// Trait for shortcut backends
pub trait ShortcutBackend: Send + Sync {
    fn register(&self, shortcut: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    fn unregister(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    fn capabilities(&self) -> BackendCapabilities;
}

/// Detect the appropriate shortcut platform for the current environment
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

/// Create the appropriate shortcut backend for the current platform
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
