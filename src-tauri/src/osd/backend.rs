//! Backend trait for cross-platform OSD window management

use crate::conf::OsdPosition;
use iced_layershell::build_pattern::MainSettings;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings};
use iced_layershell::settings::{LayerShellSettings, StartMode};
use std::sync::Arc;

use super::theme::dimensions;

// ============================================================================
// Trait Definition
// ============================================================================

/// Backend-agnostic OSD window operations
/// 
/// Note: Currently returns LayerShell-specific types. This will be generalized
/// when WinitBackend is implemented (requires different iced integration).
pub trait OsdBackend: Send + Sync {
    /// Check if this backend is available on the current platform
    fn is_available(&self) -> bool;
    
    /// Get backend name for logging
    fn name(&self) -> &'static str;
    
    /// Get settings for creating a new OSD window
    fn create_window_settings(&self, position: OsdPosition) -> NewLayerShellSettings;
    
    /// Get initial app settings for the daemon pattern
    fn app_settings(&self, position: OsdPosition) -> MainSettings;
}

// ============================================================================
// Backend Detection
// ============================================================================

/// Detect and return the appropriate backend for the current platform
pub fn detect_backend() -> Arc<dyn OsdBackend> {
    #[cfg(target_os = "linux")]
    {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        
        if session == "wayland" && !desktop.to_lowercase().contains("gnome") {
            log::info!("Detected Wayland session (non-GNOME) - using LayerShell backend");
            return Arc::new(LayerShellBackend::new());
        }
        
        // TODO: Fall back to WinitBackend for X11 or GNOME
        log::warn!("X11 or GNOME detected - LayerShell may not work correctly");
        Arc::new(LayerShellBackend::new())
    }
    
    #[cfg(not(target_os = "linux"))]
    {
        // TODO: Use WinitBackend for Windows/macOS
        log::warn!("Non-Linux platform - OSD backend not fully supported");
        Arc::new(LayerShellBackend::new())
    }
}

// ============================================================================
// LayerShellBackend
// ============================================================================

/// Backend for Linux Wayland compositors with layer-shell support
/// (Sway, Hyprland, KDE, COSMIC, etc.)
pub struct LayerShellBackend {
    window_size: (u32, u32),
    margin: i32,
}

impl LayerShellBackend {
    pub fn new() -> Self {
        Self {
            window_size: dimensions::WINDOW_SIZE,
            margin: 10,
        }
    }
    
    fn anchor_and_margin(&self, position: OsdPosition) -> (Anchor, (i32, i32, i32, i32)) {
        match position {
            OsdPosition::Top => (
                Anchor::Top | Anchor::Left | Anchor::Right,
                (self.margin, 0, 0, 0),
            ),
            OsdPosition::Bottom => (
                Anchor::Bottom | Anchor::Left | Anchor::Right,
                (0, 0, self.margin, 0),
            ),
        }
    }
}

impl Default for LayerShellBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl OsdBackend for LayerShellBackend {
    fn is_available(&self) -> bool {
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        session == "wayland"
    }
    
    fn name(&self) -> &'static str {
        "LayerShell"
    }
    
    fn create_window_settings(&self, position: OsdPosition) -> NewLayerShellSettings {
        let (anchor, margin) = self.anchor_and_margin(position);
        NewLayerShellSettings {
            size: Some(self.window_size),
            exclusive_zone: None,
            anchor,
            layer: Layer::Overlay,
            margin: Some(margin),
            keyboard_interactivity: KeyboardInteractivity::None,
            use_last_output: false,
            ..Default::default()
        }
    }
    
    fn app_settings(&self, position: OsdPosition) -> MainSettings {
        let (anchor, margin) = self.anchor_and_margin(position);
        MainSettings {
            layer_settings: LayerShellSettings {
                size: None,
                exclusive_zone: 0,
                anchor,
                layer: Layer::Overlay,
                margin,
                start_mode: StartMode::Background,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

// ============================================================================
// WinitBackend (Stub)
// ============================================================================

/// Future backend for Windows, macOS, Linux X11/GNOME
/// 
/// Not yet implemented - requires different iced integration
/// (regular winit instead of iced_layershell)
pub struct WinitBackend;

impl WinitBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WinitBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl OsdBackend for WinitBackend {
    fn is_available(&self) -> bool {
        false // Not implemented yet
    }
    
    fn name(&self) -> &'static str {
        "Winit"
    }
    
    fn create_window_settings(&self, position: OsdPosition) -> NewLayerShellSettings {
        // Placeholder - WinitBackend won't actually use LayerShell settings
        // This will be refactored when implemented
        LayerShellBackend::new().create_window_settings(position)
    }
    
    fn app_settings(&self, position: OsdPosition) -> MainSettings {
        LayerShellBackend::new().app_settings(position)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_layershell_anchor_top() {
        let backend = LayerShellBackend::new();
        let (anchor, margin) = backend.anchor_and_margin(OsdPosition::Top);
        assert!(anchor.contains(Anchor::Top));
        assert_eq!(margin.0, 10); // top margin
    }
    
    #[test]
    fn test_layershell_anchor_bottom() {
        let backend = LayerShellBackend::new();
        let (anchor, margin) = backend.anchor_and_margin(OsdPosition::Bottom);
        assert!(anchor.contains(Anchor::Bottom));
        assert_eq!(margin.2, 10); // bottom margin
    }
    
    #[test]
    fn test_detect_backend_returns_something() {
        let backend = detect_backend();
        assert!(!backend.name().is_empty());
    }
}
