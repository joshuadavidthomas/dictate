//! Text insertion for Linux
//!
//! Provides text insertion functionality that works across both X11 and Wayland,
//! using the appropriate tool (xdotool or wtype) based on the detected display server.

use anyhow::{Result, anyhow};
use std::process::Command;
use tauri_plugin_clipboard_manager::ClipboardExt;

use super::display::DisplayServer;

/// Text insertion handler that detects and uses the appropriate method
/// based on the current display server
#[derive(Debug)]
pub struct TextInserter {
    display_server: DisplayServer,
}

impl TextInserter {
    /// Create a new TextInserter, detecting the display server automatically
    pub fn new() -> Self {
        let display_server = DisplayServer::detect();
        Self { display_server }
    }

    /// Insert text at the current cursor position using the appropriate tool
    pub fn insert_text(&self, text: &str) -> Result<()> {
        let mut cmd = self.display_server.get_insert_command(text);

        // Check tool availability
        let tool = cmd.get_program().to_string_lossy().to_string();
        if !Command::new("which").arg(&tool).output()?.status.success() {
            return Err(anyhow!("{} not found", tool));
        }

        // Run command
        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("{} failed: {}", tool, stderr));
        }

        Ok(())
    }
}

impl Default for TextInserter {
    fn default() -> Self {
        Self::new()
    }
}
