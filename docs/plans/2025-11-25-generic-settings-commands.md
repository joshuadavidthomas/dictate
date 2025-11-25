# Generic Settings Commands Plan

**Date:** 2025-11-25  
**Status:** Ready for Implementation  
**Goal:** Replace 14 individual setting commands with 5 commands (2 generic + 3 dedicated)

## Current Problem

We have 14 separate Tauri commands for settings:

**Getters (7):**
- `get_output_mode()`
- `get_window_decorations()`
- `get_osd_position()`
- `get_audio_device()`
- `get_sample_rate()`
- `get_preferred_model()`
- `get_shortcut()`

**Setters (7):**
- `set_output_mode(mode: String)` - pure persistence
- `set_window_decorations(enabled: bool)` - has side effect: updates window
- `set_osd_position(position: String)` - has side effect: broadcasts to OSD
- `set_audio_device(device_name: Option<String>)` - pure persistence + validation
- `set_sample_rate(sample_rate: u32)` - pure persistence + validation
- `set_preferred_model(model: Option<ModelId>)` - pure persistence + validation
- `set_shortcut(shortcut: Option<String>)` - has side effect: OS registration

This creates boilerplate and makes adding new settings require touching commands.rs.

## Proposed Solution: Two-Tier Commands

After brainstorming, we rejected the original "2 generic commands for everything" approach because it complects the command signature - `set_setting` would need `Option<AppHandle>`, `Option<BroadcastServer>`, and `Option<ShortcutState>` even though most settings don't need them.

Instead, we use a **two-tier approach** that is honest about which settings are special:

### Tier 1: Generic Commands (pure persistence + validation)

```rust
#[tauri::command]
pub async fn get_setting(
    settings: State<'_, SettingsState>,
    key: String
) -> Result<serde_json::Value, String> {
    settings.get_setting(&key).await
}

#[tauri::command]
pub async fn set_setting(
    settings: State<'_, SettingsState>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    settings.set_setting(&key, value).await
}
```

Handles these settings:
- `output_mode` - no validation needed
- `audio_device` - validates device exists
- `sample_rate` - validates rate is supported
- `preferred_model` - validates model is known

### Tier 2: Dedicated Commands (side effects)

Keep existing commands for settings that need external state:

```rust
set_window_decorations(enabled: bool)  // needs AppHandle
set_osd_position(position: String)     // needs BroadcastServer
set_shortcut(shortcut: Option<String>) // needs ShortcutState
```

These stay as dedicated commands because they genuinely need different dependencies.

## End State

**Before:** 14 commands (7 getters + 7 setters)
**After:** 5 commands (2 generic + 3 dedicated setters)

### Commands to Delete (11)
- `get_output_mode` → use `get_setting("output_mode")`
- `get_audio_device` → use `get_setting("audio_device")`
- `get_sample_rate` → use `get_setting("sample_rate")`
- `get_preferred_model` → use `get_setting("preferred_model")`
- `get_window_decorations` → use `get_setting("window_decorations")`
- `get_osd_position` → use `get_setting("osd_position")`
- `get_shortcut` → use `get_setting("shortcut")`
- `set_output_mode` → use `set_setting("output_mode", value)`
- `set_audio_device` → use `set_setting("audio_device", value)`
- `set_sample_rate` → use `set_setting("sample_rate", value)`
- `set_preferred_model` → use `set_setting("preferred_model", value)`

### Commands to Keep (3)
- `set_window_decorations` - needs AppHandle for side effect
- `set_osd_position` - needs BroadcastServer for side effect
- `set_shortcut` - needs ShortcutState for side effect

### Commands to Add (2)
- `get_setting`
- `set_setting`

## Implementation

### conf.rs - Add Methods to SettingsState

```rust
impl SettingsState {
    pub async fn get_setting(&self, key: &str) -> Result<serde_json::Value, String> {
        let data = self.get().await;
        
        match key {
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
    
    pub async fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        match key {
            "output_mode" => {
                let mode = serde_json::from_value::<String>(value)
                    .map_err(|e| format!("Invalid value: {}", e))?;
                let parsed = OutputMode::from_str(&mode)?;
                self.update(|s| s.output_mode = parsed).await
            }
            
            "audio_device" => {
                let device_name = serde_json::from_value::<Option<String>>(value)
                    .map_err(|e| format!("Invalid value: {}", e))?;
                
                if let Some(ref name) = device_name {
                    let devices = AudioRecorder::list_devices()
                        .map_err(|e| format!("Failed to list devices: {}", e))?;
                    if !devices.iter().any(|d| &d.name == name) {
                        return Err(format!("Audio device '{}' not found", name));
                    }
                }
                
                self.update(|s| s.audio_device = device_name).await
            }
            
            "sample_rate" => {
                let rate = serde_json::from_value::<u32>(value)
                    .map_err(|e| format!("Invalid value: {}", e))?;
                SampleRate::try_from(rate).map_err(|e| e.to_string())?;
                self.update(|s| s.sample_rate = rate).await
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
                
                self.update(|s| s.preferred_model = model).await
            }
            
            // Side-effect settings - reject, must use dedicated commands
            "window_decorations" | "osd_position" | "shortcut" => {
                Err(format!("Setting '{}' requires dedicated command", key))
            }
            
            _ => Err(format!("Unknown setting: {}", key))
        }
    }
}
```

### Frontend Updates

Update `src/lib/api/settings.ts` and `src/lib/api/audio.ts`:

```typescript
// Generic setting access
async getSetting<T>(key: string): Promise<T> {
  return invoke('get_setting', { key });
}

async setSetting(key: string, value: unknown): Promise<void> {
  return invoke('set_setting', { key, value });
}

// Dedicated commands for side-effect settings (unchanged)
async setWindowDecorations(enabled: boolean): Promise<void> { ... }
async setOsdPosition(position: OsdPosition): Promise<void> { ... }
async setShortcut(shortcut: string | null): Promise<void> { ... }
```

## Line Count Estimate

**Before:** ~210 lines (14 commands × 15 lines avg)
**After:** ~140 lines
- `get_setting` in conf.rs: ~15 lines
- `set_setting` in conf.rs: ~50 lines  
- 2 command wrappers: ~15 lines
- 3 dedicated setters: ~60 lines (existing, unchanged)

**Net reduction: ~70 lines**

## Implementation Steps

### Phase 1: Add Generic Infrastructure
- [ ] Add `get_setting()` method to SettingsState in conf.rs
- [ ] Add `set_setting()` method to SettingsState in conf.rs
- [ ] Add `get_setting` and `set_setting` commands to commands.rs
- [ ] Register new commands in lib.rs

### Phase 2: Update Frontend
- [ ] Update `src/lib/api/settings.ts` to use generic commands
- [ ] Update `src/lib/api/audio.ts` to use generic commands
- [ ] Update any stores that call these APIs directly

### Phase 3: Remove Old Commands
- [ ] Remove 7 getter commands from commands.rs
- [ ] Remove 4 setter commands from commands.rs (keep 3 dedicated)
- [ ] Unregister old commands from lib.rs

### Phase 4: Verify
- [ ] Run `cargo check`
- [ ] Run `cargo test`  
- [ ] Manual UI testing for each setting
- [ ] Verify side effects still work

## Why This Design

1. **Honest about differences** - Settings with side effects get dedicated commands
2. **No complecting** - Generic commands don't carry unused state
3. **Maximum reduction** - 14 → 5 commands (64% reduction)
4. **Simple to understand** - Two clear tiers with obvious rules
5. **Easy to extend** - New pure settings just add a match arm
