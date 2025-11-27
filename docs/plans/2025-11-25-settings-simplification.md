# Settings Simplification Plan

**Date:** 2025-11-25  
**Status:** Design Phase  
**Goal:** Simplify settings handling in Tauri backend by eliminating duplication and clarifying the Settings/SettingsState architecture

## Current Problems

### 1. Duplicate Path Management
- `config_path()` and `config_last_modified_at()` are module-level functions (conf.rs:135-148)
- `SettingsState` has its own `last_modified_at` field but not the path
- Path logic is scattered between module functions and state methods

### 2. Inconsistent Settings Access Pattern
Commands mix two patterns:
- Direct loading: `Settings::load()` (used in 8+ commands)
- Managed state: `State<'_, SettingsState>` (used in setters)

This creates potential sync issues where in-memory state differs from disk.

### 3. Boilerplate Setter Methods
Seven nearly-identical `set_*` methods on `SettingsState` (lines 257-307):
- `set_output_mode`
- `set_window_decorations` 
- `set_osd_position`
- `set_audio_device`
- `set_sample_rate`
- `set_preferred_model`
- `set_shortcut`

Each follows the same pattern:
```rust
pub async fn set_field(&self, value: T) -> Result<(), String> {
    self.update(|s| s.field = value).await;
    self.save().await
}
```

Inconsistent error handling - some log errors, some don't.

### 4. Unclear Separation of Concerns
- `Settings` = serializable config (should be pure data)
- `SettingsState` = runtime wrapper (should manage lifecycle)

But they're not consistently used this way - some runtime concerns leak into Settings.

## Proposed Solution: Move All Logic to SettingsState

### Architecture Changes

**Core principle:** Keep `Settings` as pure data, move ALL logic to `SettingsState`. No renaming, just relocation of methods.

**1. Settings remains pure data (no changes needed)**

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub output_mode: OutputMode,
    pub window_decorations: bool,
    pub osd_position: OsdPosition,
    pub audio_device: Option<String>,
    pub sample_rate: u32,
    pub preferred_model: Option<ModelId>,
    pub shortcut: Option<String>,
}

impl Default for Settings { ... }  // Keep as-is
```

**2. SettingsState owns the path and contains ALL logic**

```rust
pub struct SettingsState {
    /// The actual configuration data
    data: RwLock<Settings>,
    
    /// Path to config.toml file (moved from module-level)
    config_path: PathBuf,
    
    /// Last known modification time for change detection
    last_modified: Mutex<Option<SystemTime>>,
}

impl SettingsState {
    pub fn new() -> Self {
        let config_path = get_project_dirs()
            .ok()
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .expect("Could not determine config directory");
        
        let settings = Self::load_from(&config_path).unwrap_or_default();
        let last_modified = Self::get_file_modified(&config_path).ok();
        
        Self {
            data: RwLock::new(settings),
            config_path,
            last_modified: Mutex::new(last_modified),
        }
    }
    
    /// Get a clone of current settings
    pub async fn get(&self) -> Settings {
        self.data.read().await.clone()
    }
    
    /// Update settings with a closure and auto-save
    pub async fn update<F>(&self, f: F) -> Result<(), String>
    where F: FnOnce(&mut Settings)
    {
        // Update in-memory
        {
            let mut data = self.data.write().await;
            f(&mut data);
        }
        
        // Persist to disk
        self.save().await
    }
    
    /// Private: Load settings from disk
    fn load_from(path: &Path) -> anyhow::Result<Settings> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(toml::from_str(&contents)?),
            Err(_) => Ok(Settings::default()),
        }
    }
    
    /// Private: Save current settings to disk
    async fn save(&self) -> Result<(), String> {
        let data = self.data.read().await;
        
        // Create parent dir if needed
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
        
        let toml = toml::to_string_pretty(&*data)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;
        
        fs::write(&self.config_path, toml)
            .map_err(|e| format!("Failed to write config file: {}", e))?;
        
        // Update modification time
        let modified = Self::get_file_modified(&self.config_path)?;
        *self.last_modified.lock().await = Some(modified);
        
        Ok(())
    }
    
    /// Private: Get file modification time
    fn get_file_modified(path: &Path) -> Result<SystemTime, String> {
        let metadata = fs::metadata(path)
            .map_err(|e| format!("Could not read config file metadata: {}", e))?;
        metadata
            .modified()
            .map_err(|e| format!("Could not get file modification time: {}", e))
    }
    
    /// Check if config file has changed on disk
    pub async fn check_config_changed(&self) -> Result<bool, String> {
        let file_modified = Self::get_file_modified(&self.config_path)?;
        let last_known = self.last_modified.lock().await;
        Ok(match *last_known {
            Some(known) => file_modified > known,
            None => false,
        })
    }
    
    /// Mark the in-memory settings as synced with disk
    pub async fn mark_config_synced(&self) -> Result<(), String> {
        let file_modified = Self::get_file_modified(&self.config_path)?;
        *self.last_modified.lock().await = Some(file_modified);
        Ok(())
    }
}
```

### Command Updates

**Before (inconsistent):**
```rust
#[tauri::command]
pub async fn get_output_mode() -> Result<String, String> {
    let settings = Settings::load();  // Direct load!
    Ok(settings.output_mode.as_str().to_string())
}

#[tauri::command]
pub async fn set_output_mode(
    settings: State<'_, SettingsState>,
    mode: String,
) -> Result<String, String> {
    let parsed = OutputMode::from_str(&mode)?;
    settings.set_output_mode(parsed).await?;  // Dedicated setter
    Ok(format!("Output mode set to: {}", parsed.as_str()))
}
```

**After (consistent):**
```rust
#[tauri::command]
pub async fn get_output_mode(
    settings: State<'_, SettingsState>
) -> Result<String, String> {
    let data = settings.get().await;
    Ok(data.output_mode.as_str().to_string())
}

#[tauri::command]
pub async fn set_output_mode(
    settings: State<'_, SettingsState>,
    mode: String,
) -> Result<String, String> {
    let parsed = OutputMode::from_str(&mode)?;
    settings.update(|s| s.output_mode = parsed).await?;
    Ok(format!("Output mode set to: {}", parsed.as_str()))
}
```

## Implementation Steps

### Phase 1: Refactor SettingsState Structure
- [ ] Add `config_path: PathBuf` field to `SettingsState`
- [ ] Update `SettingsState::new()` to initialize `config_path` from `get_project_dirs()`
- [ ] Move `Settings::load()` logic into `SettingsState::load_from()` (private method)
- [ ] Move `Settings::save()` logic into `SettingsState::save()` (private method)
- [ ] Add `get_file_modified()` private helper to SettingsState
- [ ] Update existing `update()` method to call `self.save()` after mutation
- [ ] Keep `check_config_changed()` and `mark_config_synced()` as-is (already on SettingsState)

### Phase 2: Remove Boilerplate Setters
- [ ] Delete `set_output_mode()` from SettingsState
- [ ] Delete `set_window_decorations()` from SettingsState
- [ ] Delete `set_osd_position()` from SettingsState
- [ ] Delete `set_audio_device()` from SettingsState
- [ ] Delete `set_sample_rate()` from SettingsState
- [ ] Delete `set_preferred_model()` from SettingsState
- [ ] Delete `set_shortcut()` from SettingsState

### Phase 3: Update Commands to Use Generic update()
- [ ] Update `set_output_mode` command to use `settings.update(|s| s.output_mode = ...)`
- [ ] Update `set_window_decorations` command
- [ ] Update `set_osd_position` command
- [ ] Update `set_audio_device` command
- [ ] Update `set_sample_rate` command
- [ ] Update `set_preferred_model` command
- [ ] Update `set_shortcut` command

### Phase 4: Unify Getter Commands
- [ ] Update `get_audio_device` to use `State<SettingsState>` instead of `Settings::load()`
- [ ] Update `get_sample_rate` to use `State<SettingsState>`
- [ ] Update `get_preferred_model` to use `State<SettingsState>`
- [ ] Update `get_output_mode` to use `State<SettingsState>`
- [ ] Update `get_window_decorations` to use `State<SettingsState>`
- [ ] Update `get_osd_position` to use `State<SettingsState>`
- [ ] Update `get_shortcut` to use `State<SettingsState>`

### Phase 5: Cleanup Module-Level Functions
- [ ] Remove `config_path()` module function (logic now in SettingsState)
- [ ] Remove `config_last_modified_at()` module function (logic now in SettingsState)
- [ ] Keep `get_project_dirs()` as it's used elsewhere
- [ ] Remove `Settings::load()` and `Settings::save()` public methods (logic moved to SettingsState)

### Phase 6: Check for Other Usages
- [ ] Search codebase for `Settings::load()` calls outside commands.rs
- [ ] Search for `config_path()` calls
- [ ] Search for `config_last_modified_at()` calls
- [ ] Update any found usages

### Phase 7: Testing & Validation
- [ ] Run existing unit tests in conf.rs
- [ ] Build the project to catch compilation errors
- [ ] Manual test: change each setting via UI
- [ ] Manual test: verify settings persist across app restart
- [ ] Manual test: verify external config file changes are detected

## Benefits

1. **Single Source of Truth:** All access goes through `SettingsState`
2. **Less Duplication:** Generic `update()` replaces 7 setter methods  
3. **Clearer Ownership:** Path management lives with the state
4. **Consistent Patterns:** All commands use the same access pattern
5. **Better Encapsulation:** Settings file I/O is internal to the module

## Migration Safety

- All changes are internal to `conf.rs` and `commands.rs`
- Frontend API (Tauri commands) remains unchanged
- Database schema unaffected
- Can be done incrementally

## Files to Modify

1. `src-tauri/src/conf.rs` - Core refactoring
2. `src-tauri/src/commands.rs` - Update command implementations
3. Potentially other files that import `Settings::load()` (check with grep)

## Open Questions

1. Should we keep `check_config_changed()` / `mark_config_synced()` or auto-reload on access?
2. Do we need explicit reload hooks for the frontend?
3. Should `Settings::default()` be the only public constructor?
