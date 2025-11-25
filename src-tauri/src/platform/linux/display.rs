//! Display server detection for Linux
//!
//! Provides unified detection of whether the system is running X11, Wayland,
//! or an unknown display server. Also includes detection of Wayland-specific
//! capabilities like the GlobalShortcuts portal.

use std::env;
use std::process::Command;

/// The detected display server type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayServer {
    /// Wayland compositor with optional portal support info
    Wayland,
    /// X11/Xorg display server
    X11,
    /// Could not determine display server
    Unknown,
}

impl DisplayServer {
    /// Detect which display server is currently running
    pub fn detect() -> Self {
        // Check for Wayland first
        if env::var("WAYLAND_DISPLAY").is_ok()
            || env::var("XDG_SESSION_TYPE")
                .as_ref()
                .map(|s| s.as_str())
                == Ok("wayland")
        {
            return DisplayServer::Wayland;
        }

        // Check for X11
        if env::var("DISPLAY").is_ok() {
            return DisplayServer::X11;
        }

        // Check XDG_SESSION_TYPE for x11
        if env::var("XDG_SESSION_TYPE")
            .as_ref()
            .map(|s| s.to_lowercase())
            == Ok("x11".to_string())
        {
            return DisplayServer::X11;
        }

        // Fallback: check running processes
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

    /// Get the text insertion command for this display server
    pub fn get_insert_command(&self, text: &str) -> Command {
        match self {
            DisplayServer::X11 => {
                let mut cmd = Command::new("xdotool");
                cmd.args(["type", "--"]).arg(text);
                cmd
            }
            DisplayServer::Wayland => {
                let mut cmd = Command::new("wtype");
                cmd.arg(text);
                cmd
            }
            DisplayServer::Unknown => {
                // Fallback to echo (won't actually insert text)
                let mut cmd = Command::new("echo");
                cmd.args(["-n"]).arg(text);
                cmd
            }
        }
    }
}

/// Check if the GlobalShortcuts portal is available (for Wayland)
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

/// Detect the compositor/desktop environment
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
