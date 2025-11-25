use crate::models::ModelId;
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Print,
    Copy,
    Insert,
}

impl OutputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputMode::Print => "print",
            OutputMode::Copy => "copy",
            OutputMode::Insert => "insert",
        }
    }
    
    pub fn deliver(&self, text: &str, app: &tauri::AppHandle) -> anyhow::Result<()> {
        match self {
            OutputMode::Print => {
                println!("{}", text);
                Ok(())
            }
            OutputMode::Copy => {
                use tauri_plugin_clipboard_manager::ClipboardExt;
                app.clipboard()
                    .write_text(text.to_string())
                    .map_err(|e| anyhow::anyhow!("Failed to write to clipboard: {}", e))
            }
            OutputMode::Insert => {
                use crate::platform::linux::TextInserter;
                let inserter = TextInserter::new();
                inserter.insert_text(text)
            }
        }
    }
}

impl std::str::FromStr for OutputMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "print" => Ok(OutputMode::Print),
            "copy" => Ok(OutputMode::Copy),
            "insert" => Ok(OutputMode::Insert),
            other => Err(format!("Invalid output mode: {}", other)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OsdPosition {
    Top,
    Bottom,
}

impl OsdPosition {
    pub fn as_str(&self) -> &'static str {
        match self {
            OsdPosition::Top => "top",
            OsdPosition::Bottom => "bottom",
        }
    }
}

impl std::str::FromStr for OsdPosition {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "top" => Ok(OsdPosition::Top),
            "bottom" => Ok(OsdPosition::Bottom),
            other => Err(format!("Invalid OSD position: {}", other)),
        }
    }
}

impl Default for OsdPosition {
    fn default() -> Self {
        Self::Top
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default)]
    pub output_mode: OutputMode,

    /// Whether to show window decorations (titlebar, borders)
    /// Default is true. Set to false for tiling WM users who prefer no titlebar.
    #[serde(default = "default_decorations")]
    pub window_decorations: bool,

    /// Position of the on-screen display (OSD) overlay
    /// Default is Top
    #[serde(default)]
    pub osd_position: OsdPosition,

    /// Preferred audio input device name
    /// If None, uses system default device
    #[serde(default)]
    pub audio_device: Option<String>,

    /// Audio sample rate in Hz
    /// Default is 16000 (16kHz, optimal for Whisper)
    /// Common values: 16000, 44100, 48000
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Preferred transcription model
    /// If None, the app will fall back to a sensible default.
    #[serde(default)]
    pub preferred_model: Option<ModelId>,

    /// Global keyboard shortcut to start/stop recording
    /// Format: "CommandOrControl+Shift+Space" or similar
    /// If None, no global shortcut is registered
    #[serde(default = "default_shortcut")]
    pub shortcut: Option<String>,
}

fn default_decorations() -> bool {
    true
}

fn default_sample_rate() -> u32 {
    16000 // Optimal for Whisper transcription
}

fn default_shortcut() -> Option<String> {
    Some("CommandOrControl+Shift+Space".to_string())
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_mode: OutputMode::Print,
            window_decorations: true,
            osd_position: OsdPosition::Top,
            audio_device: None,
            sample_rate: 16000,
            preferred_model: None,
            shortcut: default_shortcut(),
        }
    }
}

impl Settings {
    /// Load config from ~/.config/dictate/config.toml
    /// Returns default settings if file doesn't exist or fails to parse
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            eprintln!("[config] Could not determine config directory, using defaults");
            return Self::default();
        };

        match fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(settings) => {
                    eprintln!("[config] Loaded settings from: {}", path.display());
                    settings
                }
                Err(e) => {
                    eprintln!("[config] Failed to parse config: {}, using defaults", e);
                    Self::default()
                }
            },
            Err(_) => {
                eprintln!(
                    "[config] No config file found at {}, using defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Save config to ~/.config/dictate/config.toml
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = config_path() else {
            anyhow::bail!("Could not determine config directory");
        };

        // Create parent dir if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let toml = toml::to_string_pretty(self).context("Failed to serialize settings to TOML")?;

        fs::write(&path, toml)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        eprintln!("[config] Saved settings to: {}", path.display());

        Ok(())
    }
}

/// Get the path to the config file: ~/.config/dictate/config.toml
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "dictate").map(|dirs| dirs.config_dir().join("config.toml"))
}

/// Get the last modification time of the config file
pub fn config_last_modified_at() -> anyhow::Result<SystemTime> {
    let path = config_path().context("Could not determine config path")?;
    let metadata = fs::metadata(&path).context("Could not read config file metadata")?;
    metadata
        .modified()
        .context("Could not get file modification time")
}

/// Settings wrapper with config file change detection
pub struct SettingsState {
    settings: RwLock<Settings>,
    last_modified_at: Mutex<Option<SystemTime>>,
}

impl SettingsState {
    pub fn new() -> Self {
        let settings = Settings::load();
        let last_modified_at = config_last_modified_at().ok();

        Self {
            settings: RwLock::new(settings),
            last_modified_at: Mutex::new(last_modified_at),
        }
    }

    pub async fn get(&self) -> Settings {
        self.settings.read().await.clone()
    }

    pub async fn update<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Settings) -> R,
    {
        let mut settings = self.settings.write().await;
        f(&mut settings)
    }

    pub async fn save(&self) -> Result<(), String> {
        {
            let settings = self.settings.read().await;
            settings.save().map_err(|e| e.to_string())?;
        }

        let file_last_modified_at = config_last_modified_at().map_err(|e| e.to_string())?;
        *self.last_modified_at.lock().await = Some(file_last_modified_at);

        Ok(())
    }

    pub async fn set_output_mode(&self, mode: OutputMode) -> Result<(), String> {
        self.update(|s| s.output_mode = mode).await;

        if let Err(e) = self.save().await {
            eprintln!("[settings] Failed to save config: {}", e);
        }

        Ok(())
    }

    pub async fn set_window_decorations(&self, enabled: bool) -> Result<(), String> {
        self.update(|s| s.window_decorations = enabled).await;
        self.save()
            .await
            .map_err(|e| format!("Failed to save settings: {}", e))
    }

    pub async fn set_osd_position(&self, position: OsdPosition) -> Result<(), String> {
        self.update(|s| s.osd_position = position).await;
        self.save()
            .await
            .map_err(|e| format!("Failed to save settings: {}", e))
    }

    pub async fn set_audio_device(&self, device_name: Option<String>) -> Result<(), String> {
        self.update(|s| s.audio_device = device_name).await;
        self.save()
            .await
            .map_err(|e| format!("Failed to save settings: {}", e))
    }

    pub async fn set_sample_rate(&self, sample_rate: u32) -> Result<(), String> {
        self.update(|s| s.sample_rate = sample_rate).await;
        self.save()
            .await
            .map_err(|e| format!("Failed to save settings: {}", e))
    }

    pub async fn set_preferred_model(&self, model: Option<ModelId>) -> Result<(), String> {
        self.update(|s| s.preferred_model = model).await;
        self.save()
            .await
            .map_err(|e| format!("Failed to save settings: {}", e))
    }

    pub async fn set_shortcut(&self, shortcut: Option<String>) -> Result<(), String> {
        self.update(|s| s.shortcut = shortcut).await;
        self.save()
            .await
            .map_err(|e| format!("Failed to save settings: {}", e))
    }

    /// Returns true if the config file on disk has changed
    /// since we last considered settings and file to be in sync.
    pub async fn check_config_changed(&self) -> Result<bool, String> {
        let file_last_modified_at = config_last_modified_at().map_err(|e| e.to_string())?;
        let last_seen_modified_at = self.last_modified_at.lock().await;
        Ok(match *last_seen_modified_at {
            Some(last_seen) => file_last_modified_at > last_seen,
            None => false,
        })
    }

    /// Mark the in-memory settings as synced with the
    /// current config file on disk.
    pub async fn mark_config_synced(&self) -> Result<(), String> {
        let file_last_modified_at = config_last_modified_at().map_err(|e| e.to_string())?;
        *self.last_modified_at.lock().await = Some(file_last_modified_at);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.output_mode, OutputMode::Print);
    }

    #[test]
    fn test_serialize_deserialize() {
        let settings = Settings {
            output_mode: OutputMode::Copy,
            window_decorations: false,
            osd_position: OsdPosition::Bottom,
            audio_device: Some("Test Device".to_string()),
            sample_rate: 48000,
            preferred_model: None,
            shortcut: Some("Ctrl+Shift+R".to_string()),
        };

        let toml = toml::to_string(&settings).unwrap();
        let deserialized: Settings = toml::from_str(&toml).unwrap();

        assert_eq!(deserialized.output_mode, OutputMode::Copy);
        assert!(!deserialized.window_decorations);
        assert_eq!(deserialized.osd_position, OsdPosition::Bottom);
        assert_eq!(deserialized.audio_device, Some("Test Device".to_string()));
        assert_eq!(deserialized.sample_rate, 48000);
        assert_eq!(deserialized.shortcut, Some("Ctrl+Shift+R".to_string()));
    }
}
