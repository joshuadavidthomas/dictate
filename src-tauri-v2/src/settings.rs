//! Application settings
//!
//! Handles loading, saving, and runtime management of user preferences.

use crate::transcription::ModelId;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::sync::RwLock;

// ============================================================================
// Types
// ============================================================================

/// How transcribed text should be output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Print,
    Copy,
    Insert,
}

impl OutputMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Print => "print",
            Self::Copy => "copy",
            Self::Insert => "insert",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "print" => Some(Self::Print),
            "copy" => Some(Self::Copy),
            "insert" => Some(Self::Insert),
            _ => None,
        }
    }
}

/// Position of the on-screen display overlay
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OsdPosition {
    #[default]
    Top,
    Bottom,
}

impl OsdPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

// ============================================================================
// Settings
// ============================================================================

/// User-configurable settings persisted to TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub output_mode: OutputMode,

    #[serde(default = "default_true")]
    pub window_decorations: bool,

    #[serde(default)]
    pub osd_position: OsdPosition,

    #[serde(default)]
    pub audio_device: Option<String>,

    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    #[serde(default)]
    pub preferred_model: Option<ModelId>,
}

fn default_true() -> bool {
    true
}

fn default_sample_rate() -> u32 {
    16000
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
        }
    }
}

impl Settings {
    /// Load settings from disk, returning defaults if file doesn't exist or is invalid
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            eprintln!("[settings] Could not determine config path, using defaults");
            return Self::default();
        };

        match fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(settings) => {
                    eprintln!("[settings] Loaded from: {}", path.display());
                    settings
                }
                Err(e) => {
                    eprintln!("[settings] Parse error: {}, using defaults", e);
                    Self::default()
                }
            },
            Err(_) => {
                eprintln!("[settings] No config at {}, using defaults", path.display());
                Self::default()
            }
        }
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<()> {
        let path = config_path().context("Could not determine config path")?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
        }

        let toml = toml::to_string_pretty(self).context("Failed to serialize settings")?;

        fs::write(&path, toml)
            .with_context(|| format!("Failed to write config: {}", path.display()))?;

        eprintln!("[settings] Saved to: {}", path.display());
        Ok(())
    }
}

/// Get the config file path: ~/.config/dictate/config.toml
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "dictate").map(|dirs| dirs.config_dir().join("config.toml"))
}

/// Get the last modification time of the config file
pub fn config_modified_time() -> Result<SystemTime> {
    let path = config_path().context("Could not determine config path")?;
    let meta = fs::metadata(&path).context("Could not read config metadata")?;
    meta.modified().context("Could not get modification time")
}

// ============================================================================
// Runtime Settings State
// ============================================================================

/// Thread-safe settings wrapper for Tauri state management
///
/// Provides async access to settings with change detection for
/// notifying the frontend when config file changes externally.
pub struct SettingsState {
    inner: RwLock<Settings>,
    last_sync: RwLock<Option<SystemTime>>,
}

impl SettingsState {
    pub fn new() -> Self {
        let settings = Settings::load();
        let last_sync = config_modified_time().ok();

        Self {
            inner: RwLock::new(settings),
            last_sync: RwLock::new(last_sync),
        }
    }

    /// Get a clone of current settings
    pub async fn get(&self) -> Settings {
        self.inner.read().await.clone()
    }

    /// Update settings and save to disk
    pub async fn update<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Settings),
    {
        {
            let mut settings = self.inner.write().await;
            f(&mut settings);
            settings.save()?;
        }

        // Update sync time
        if let Ok(time) = config_modified_time() {
            *self.last_sync.write().await = Some(time);
        }

        Ok(())
    }

    /// Check if config file changed externally since last sync
    pub async fn has_external_changes(&self) -> bool {
        let Ok(current) = config_modified_time() else {
            return false;
        };

        let last = self.last_sync.read().await;
        match *last {
            Some(last_time) => current > last_time,
            None => false,
        }
    }

    /// Mark settings as synced with current file state
    pub async fn mark_synced(&self) -> Result<()> {
        let time = config_modified_time()?;
        *self.last_sync.write().await = Some(time);
        Ok(())
    }

    // Convenience methods for common operations

    pub async fn set_output_mode(&self, mode: OutputMode) -> Result<()> {
        self.update(|s| s.output_mode = mode).await
    }

    pub async fn set_window_decorations(&self, enabled: bool) -> Result<()> {
        self.update(|s| s.window_decorations = enabled).await
    }

    pub async fn set_osd_position(&self, position: OsdPosition) -> Result<()> {
        self.update(|s| s.osd_position = position).await
    }

    pub async fn set_audio_device(&self, device: Option<String>) -> Result<()> {
        self.update(|s| s.audio_device = device).await
    }

    pub async fn set_sample_rate(&self, rate: u32) -> Result<()> {
        self.update(|s| s.sample_rate = rate).await
    }

    pub async fn set_preferred_model(&self, model: Option<ModelId>) -> Result<()> {
        self.update(|s| s.preferred_model = model).await
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.output_mode, OutputMode::Print);
        assert!(settings.window_decorations);
        assert_eq!(settings.sample_rate, 16000);
    }

    #[test]
    fn test_serialize_roundtrip() {
        let settings = Settings {
            output_mode: OutputMode::Copy,
            window_decorations: false,
            osd_position: OsdPosition::Bottom,
            audio_device: Some("Test Device".into()),
            sample_rate: 48000,
            preferred_model: None,
        };

        let toml = toml::to_string(&settings).unwrap();
        let parsed: Settings = toml::from_str(&toml).unwrap();

        assert_eq!(parsed.output_mode, OutputMode::Copy);
        assert!(!parsed.window_decorations);
        assert_eq!(parsed.osd_position, OsdPosition::Bottom);
        assert_eq!(parsed.sample_rate, 48000);
    }
}
