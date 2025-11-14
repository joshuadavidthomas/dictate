use crate::state::OutputMode;
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default)]
    pub output_mode: OutputMode,
    
    // Future settings:
    // pub audio_device: Option<String>,
    // pub preferred_model: Option<String>,
    // pub hotkey: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_mode: OutputMode::Print,
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
            Ok(contents) => {
                match toml::from_str(&contents) {
                    Ok(settings) => {
                        eprintln!("[config] Loaded settings from: {}", path.display());
                        settings
                    }
                    Err(e) => {
                        eprintln!("[config] Failed to parse config: {}, using defaults", e);
                        Self::default()
                    }
                }
            }
            Err(_) => {
                eprintln!("[config] No config file found at {}, using defaults", path.display());
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
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
        }
        
        let toml = toml::to_string_pretty(self)
            .context("Failed to serialize settings to TOML")?;
        
        fs::write(&path, toml)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        
        eprintln!("[config] Saved settings to: {}", path.display());
        
        Ok(())
    }
}

/// Get the path to the config file: ~/.config/dictate/config.toml
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "dictate")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

/// Get the modification time of the config file
pub fn config_mtime() -> anyhow::Result<SystemTime> {
    let path = config_path().context("Could not determine config path")?;
    let metadata = fs::metadata(&path).context("Could not read config file metadata")?;
    metadata.modified().context("Could not get file modification time")
}

// File watcher removed - using window focus detection instead

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
        };
        
        let toml = toml::to_string(&settings).unwrap();
        let deserialized: Settings = toml::from_str(&toml).unwrap();
        
        assert_eq!(deserialized.output_mode, OutputMode::Copy);
    }
}
