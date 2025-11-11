use anyhow::{Result, anyhow};
use std::env;
use std::process::Command;

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

    /// Get the clipboard copy command for this display server
    pub fn get_clipboard_command(&self) -> std::process::Command {
        match self {
            DisplayServer::X11 => {
                let mut cmd = std::process::Command::new("xclip");
                cmd.args(["-selection", "clipboard"])
                    .stdin(std::process::Stdio::piped());
                cmd
            }
            DisplayServer::Wayland => {
                let mut cmd = std::process::Command::new("wl-copy");
                cmd.args(["--type", "text/plain;charset=utf-8"])
                    .stdin(std::process::Stdio::piped());
                cmd
            }
            DisplayServer::Unknown => std::process::Command::new("echo"),
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

    pub fn copy_to_clipboard(&self, text: &str) -> Result<()> {
        let mut cmd = self.display_server.get_clipboard_command();

        // Check tool availability
        let tool = cmd.get_program().to_string_lossy().to_string();
        if !Command::new("which").arg(&tool).output()?.status.success() {
            return Err(anyhow!("{} not found", tool));
        }

        // Build and run command with stdin
        let mut child = cmd.stdin(std::process::Stdio::piped()).spawn()?;

        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(text.as_bytes())?;
        }

        let output = child.wait_with_output()?;

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
