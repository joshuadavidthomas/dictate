use anyhow::{Result, anyhow};
use std::env;
use std::process::Command;
use tauri_plugin_clipboard_manager::ClipboardExt;

#[derive(Debug, Clone)]
pub enum DisplayServer {
    Wayland,
    X11,
    Unknown,
}

impl DisplayServer {
    /// Detect which display server is currently running
    pub fn detect() -> Self {
        if env::var("WAYLAND_DISPLAY").is_ok()
            || env::var("XDG_SESSION_TYPE").as_ref().map(|s| s.as_str()) == Ok("wayland")
        {
            return DisplayServer::Wayland;
        }

        if env::var("DISPLAY").is_ok() {
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

    /// Get the text insertion command for this display server
    pub fn get_insert_command(&self, text: &str) -> std::process::Command {
        match self {
            DisplayServer::X11 => {
                let mut cmd = std::process::Command::new("xdotool");
                cmd.args(["type", "--"]).arg(text);
                cmd
            }
            DisplayServer::Wayland => {
                let mut cmd = std::process::Command::new("wtype");
                cmd.arg(text);
                cmd
            }
            DisplayServer::Unknown => {
                let mut cmd = std::process::Command::new("echo");
                cmd.args(["-n"]).arg(text);
                cmd
            }
        }
    }


}

#[derive(Debug)]
pub struct TextInserter {
    display_server: DisplayServer,
}

impl TextInserter {
    pub fn new() -> Self {
        let display_server = DisplayServer::detect();
        Self { display_server }
    }

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

    pub fn copy_to_clipboard(&self, app: &tauri::AppHandle, text: &str) -> Result<()> {
        app.clipboard()
            .write_text(text.to_string())
            .map_err(|e| anyhow!("Failed to write to clipboard: {}", e))
    }
}

impl Default for TextInserter {
    fn default() -> Self {
        Self::new()
    }
}
