# Consolidate to Two Commands Plan

**Date:** 2025-11-25  
**Status:** Ready for Implementation  
**Goal:** Complete the consolidation from 5 commands to 2 commands, with dispatch logic in commands.rs and SettingsState staying pure

## Current State (After Initial Refactor)

We currently have **5 Tauri commands**:

**Generic commands (2):**
- `get_setting(key)` - handles all 7 getters, delegates to `SettingsState::get_setting()`
- `set_setting(key, value)` - handles only 4 pure settings, delegates to `SettingsState::set_setting()`

**Dedicated commands (3):**
- `set_window_decorations(enabled)` - has side effect: updates window via AppHandle
- `set_osd_position(position)` - has side effect: broadcasts to OSD via BroadcastServer
- `set_shortcut(shortcut)` - has side effect: OS registration via ShortcutState

**Current implementation complects SettingsState:**
- `SettingsState::get_setting()` has JSON serialization logic (~15 lines)
- `SettingsState::set_setting()` has validation + rejection logic (~65 lines)
- SettingsState should be a pure state container, not know about JSON/validation/side-effects

## Goal State

**2 Tauri commands total:**
- `get_setting(key)` - handles all 7 getters
- `set_setting(key, value)` - handles ALL 7 setters with side effects

**Match logic lives in commands.rs, NOT conf.rs.**

SettingsState stays pure: just `get()`, `update()`, `save()`.

## The Design Principle

> "Design is about taking things apart."

SettingsState should do ONE thing: manage settings state. It shouldn't know about:
- AppHandle, BroadcastServer, ShortcutState
- JSON serialization/deserialization
- Validation logic (audio device exists, model known, etc.)

The match in commands.rs is the **composition point** - it brings together the pieces (settings state + dependencies) for each setting key.

## Changes Required

### 1. Delete Methods from SettingsState (conf.rs)

Remove these methods that complect SettingsState:
- `get_setting()` (~15 lines)
- `set_setting()` (~65 lines)

SettingsState stays pure with just: `new()`, `get()`, `update()`, `save()`, `check_config_changed()`, `mark_config_synced()`

### 2. Move Match Logic to commands.rs

**get_setting command:**
```rust
#[tauri::command]
pub async fn get_setting(
    settings: State<'_, SettingsState>,
    key: String,
) -> Result<serde_json::Value, String> {
    let data = settings.get().await;
    
    match key.as_str() {
        "output_mode" => Ok(serde_json::to_value(data.output_mode.as_str()).unwrap()),
        "audio_device" => Ok(serde_json::to_value(&data.audio_device).unwrap()),
        "sample_rate" => Ok(serde_json::to_value(data.sample_rate).unwrap()),
        "preferred_model" => Ok(serde_json::to_value(&data.preferred_model).unwrap()),
        "window_decorations" => Ok(serde_json::to_value(data.window_decorations).unwrap()),
        "osd_position" => Ok(serde_json::to_value(data.osd_position.as_str()).unwrap()),
        "shortcut" => Ok(serde_json::to_value(&data.shortcut).unwrap()),
        _ => Err(format!("Unknown setting: {}", key))
    }
}
```

**set_setting command:**
```rust
#[tauri::command]
pub async fn set_setting(
    app: AppHandle,
    broadcast: State<'_, BroadcastServer>,
    shortcut_state: State<'_, ShortcutState>,
    settings: State<'_, SettingsState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    match key.as_str() {
        "output_mode" => {
            let mode = serde_json::from_value::<String>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            let parsed = OutputMode::from_str(&mode)?;
            settings.update(|s| s.output_mode = parsed).await
        }
        
        "audio_device" => {
            let device_name = serde_json::from_value::<Option<String>>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            
            // Validation
            if let Some(ref name) = device_name {
                let devices = AudioRecorder::list_devices()
                    .map_err(|e| format!("Failed to list devices: {}", e))?;
                if !devices.iter().any(|d| &d.name == name) {
                    return Err(format!("Audio device '{}' not found", name));
                }
            }
            
            settings.update(|s| s.audio_device = device_name).await
        }
        
        "sample_rate" => {
            let rate = serde_json::from_value::<u32>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            SampleRate::try_from(rate).map_err(|e| e.to_string())?;
            settings.update(|s| s.sample_rate = rate).await
        }
        
        "preferred_model" => {
            let model = serde_json::from_value::<Option<ModelId>>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            
            if let Some(m) = model {
                let manager = ModelManager::new().map_err(|e| e.to_string())?;
                if manager.get_model_info(m).is_none() {
                    return Err(format!("Unknown model: {:?}", m));
                }
            }
            
            settings.update(|s| s.preferred_model = model).await
        }
        
        "window_decorations" => {
            let enabled = serde_json::from_value::<bool>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            
            settings.update(|s| s.window_decorations = enabled).await?;
            
            // Side effect
            if let Some(window) = app.get_webview_window("main") {
                window
                    .set_decorations(enabled)
                    .map_err(|e| format!("Failed to set decorations: {}", e))?;
            }
            
            Ok(())
        }
        
        "osd_position" => {
            let position = serde_json::from_value::<String>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            let parsed = OsdPosition::from_str(&position)?;
            
            settings.update(|s| s.osd_position = parsed).await?;
            
            // Side effect
            broadcast.osd_position_updated(parsed).await;
            
            Ok(())
        }
        
        "shortcut" => {
            let shortcut = serde_json::from_value::<Option<String>>(value)
                .map_err(|e| format!("Invalid value: {}", e))?;
            
            // Unregister existing
            let backend_guard = shortcut_state.backend().await;
            if let Some(backend) = backend_guard.as_ref() {
                backend
                    .unregister()
                    .await
                    .map_err(|e| format!("Failed to unregister shortcut: {}", e))?;
            }
            
            settings.update(|s| s.shortcut = shortcut.clone()).await?;
            
            // Register new if provided
            if let Some(new_shortcut) = &shortcut {
                if let Some(backend) = backend_guard.as_ref() {
                    backend
                        .register(new_shortcut)
                        .await
                        .map_err(|e| format!("Failed to register shortcut: {}", e))?;
                    eprintln!("[shortcut] Registered new shortcut: {}", new_shortcut);
                }
            }
            
            Ok(())
        }
        
        _ => Err(format!("Unknown setting: {}", key))
    }
}
```

### 3. Remove Dedicated Commands

Remove from commands.rs:
- `set_window_decorations()`
- `set_osd_position()`
- `set_shortcut()`

Unregister from lib.rs.

### 4. Update Frontend

Update `src/lib/api/settings.ts`:
- `setWindowDecorations()` → `invoke('set_setting', { key: 'window_decorations', value: enabled })`
- `setOsdPosition()` → `invoke('set_setting', { key: 'osd_position', value: position })`
- `setShortcut()` → `invoke('set_setting', { key: 'shortcut', value: shortcut })`

## Implementation Steps

### Phase 1: Refactor commands.rs
- [ ] Inline `get_setting` match logic (move from conf.rs)
- [ ] Inline `set_setting` match logic with all 7 settings (move from conf.rs + add side effects)
- [ ] Add dependencies to `set_setting` signature (app, broadcast, shortcut_state as first args)
- [ ] Remove dedicated `set_window_decorations`, `set_osd_position`, `set_shortcut` commands

### Phase 2: Clean up conf.rs
- [ ] Delete `get_setting()` method from SettingsState
- [ ] Delete `set_setting()` method from SettingsState
- [ ] Remove unused imports

### Phase 3: Update lib.rs
- [ ] Unregister `set_window_decorations`, `set_osd_position`, `set_shortcut`

### Phase 4: Update Frontend
- [ ] Update `setWindowDecorations()` to use `set_setting`
- [ ] Update `setOsdPosition()` to use `set_setting`
- [ ] Update `setShortcut()` to use `set_setting`

### Phase 5: Verify
- [ ] Run `cargo check`
- [ ] Test all 7 settings via UI

## Line Count

**Deleting from conf.rs:** ~80 lines (get_setting + set_setting methods)
**Adding to commands.rs:** ~90 lines (expanded match with side effects)
**Deleting from commands.rs:** ~60 lines (3 dedicated commands)

**Net:** ~50 lines removed

More importantly: SettingsState is now pure, and there are only 2 commands.

## Files to Modify

1. `src-tauri/src/commands.rs` - expand match logic, remove 3 commands
2. `src-tauri/src/conf.rs` - delete 2 methods
3. `src-tauri/src/lib.rs` - unregister 3 commands
4. `src/lib/api/settings.ts` - update 3 setters
