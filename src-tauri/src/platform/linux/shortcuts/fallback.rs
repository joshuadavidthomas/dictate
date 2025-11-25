//! Fallback shortcuts backend for unsupported environments

use super::{BackendCapabilities, ShortcutBackend, ShortcutPlatform};
use crate::platform::display::detect_compositor;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

/// Fallback backend that doesn't actually register shortcuts
/// Used when no suitable backend is available (e.g., Wayland without portal support)
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
