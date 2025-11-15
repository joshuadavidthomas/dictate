# Refactoring Implementation Plan - Current Status & Next Steps

## What We've Completed So Far ✅

### 1. Frontend Refactoring (100% Complete)
- ✅ Created `src/lib/api/` - typed API layer for all Tauri commands
  - `types.ts` - shared TypeScript types
  - `recording.ts` - recording commands
  - `settings.ts` - settings commands
  - `transcriptions.ts` - history commands
  - `audio.ts` - audio device commands
  - `index.ts` - re-exports

- ✅ Created `src/lib/stores/` - Svelte 5 reactive stores
  - `recording.svelte.ts` - recording state + event listeners
  - `settings.svelte.ts` - settings state
  - `transcriptions.svelte.ts` - history state
  - `index.ts` - re-exports

- ✅ Migrated all 3 Svelte pages to use new API/stores
  - `+page.svelte` (main recording) - 50 lines → 18 lines
  - `history/+page.svelte` - cleaner, reactive
  - `settings/+page.svelte` - simplified

### 2. Backend State Refactoring (60% Complete)
- ✅ Moved `OutputMode` from `state.rs` to `conf.rs`
- ✅ Created new state structs in `state.rs`:
  - `RecordingSession` - groups recording state + current_recording + elapsed_ms
  - `TranscriptionState` - groups engine + model_manager
  - `Database` - wraps SqlitePool
  - **NEEDS:** `SettingsState` - wrapper for Settings + config_mtime tracking
- ✅ Updated `lib.rs` to manage separate states instead of monolithic AppState
- ❌ `commands.rs` - NOT UPDATED (still uses old AppState)

## Current Problem

We got sidetracked doing mechanical refactoring when we should be doing ARCHITECTURAL refactoring:
- ❌ Was going to just replace `State<'_, AppState>` with specific states
- ✅ **SHOULD** extract logic out of commands.rs to live near related code

## The Right Plan: Extract Logic + Thin Commands

### Phase 0: Fix State Structures (REQUIRED FIRST)

**Update `state.rs` to add:**

1. **SettingsState wrapper** - for config change detection:
```rust
use std::time::SystemTime;

pub struct SettingsState {
    settings: Arc<Mutex<Settings>>,
    config_mtime: Arc<Mutex<Option<SystemTime>>>,
}

impl SettingsState {
    pub fn new(settings: Settings) -> Self {
        let mtime = crate::conf::config_mtime().ok();
        Self {
            settings: Arc::new(Mutex::new(settings)),
            config_mtime: Arc::new(Mutex::new(mtime)),
        }
    }
    
    pub fn settings(&self) -> &Arc<Mutex<Settings>> {
        &self.settings
    }
    
    pub async fn check_config_changed(&self) -> Result<bool, String> {
        let current_mtime = crate::conf::config_mtime().map_err(|e| e.to_string())?;
        let stored_mtime = self.config_mtime.lock().await;
        Ok(match *stored_mtime {
            Some(stored) => current_mtime > stored,
            None => false,
        })
    }
    
    pub async fn update_config_mtime(&self) -> Result<(), String> {
        let mtime = crate::conf::config_mtime().map_err(|e| e.to_string())?;
        *self.config_mtime.lock().await = Some(mtime);
        Ok(())
    }
}
```

2. **RecordingSession - add elapsed_ms tracking**:
```rust
pub struct RecordingSession {
    state: Arc<Mutex<RecordingState>>,
    current_recording: Arc<Mutex<Option<ActiveRecording>>>,
    start_time: std::time::Instant, // ADD THIS
}

impl RecordingSession {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            current_recording: Arc::new(Mutex::new(None)),
            start_time: std::time::Instant::now(), // ADD THIS
        }
    }
    
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}
```

**Update `lib.rs` to use SettingsState:**
```rust
app.manage(SettingsState::new(conf::Settings::load()));
// Instead of: app.manage(Arc::new(Mutex::new(conf::Settings::load())));
```

### Phase A: Extract Recording Logic

**Create: `src-tauri/src/audio/recording.rs`**

```rust
// All recording orchestration logic moves here
use crate::state::{RecordingSession, TranscriptionState, ActiveRecording};
use crate::conf::{Settings, OutputMode};
use crate::broadcast::BroadcastServer;
use tauri::{AppHandle, Emitter};
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn start(
    session: &RecordingSession,
    settings: &Arc<Mutex<Settings>>,
    broadcast: &BroadcastServer,
    app: &AppHandle,
) -> Result<(), String> {
    // MOVE: All 60 lines of start_recording() logic from commands.rs
    // - Get audio settings
    // - Create AudioRecorder
    // - Start recording stream
    // - Spawn spectrum broadcast task
    // - Store ActiveRecording
}

pub async fn stop_and_transcribe(
    session: &RecordingSession,
    transcription_state: &TranscriptionState,
    settings: &Arc<Mutex<Settings>>,
    broadcast: &BroadcastServer,
    db: Option<&Database>,
    app: &AppHandle,
) -> Result<(), String> {
    // MOVE: All 200+ lines of stop_and_transcribe() logic from commands.rs
    // - Stop recording
    // - Save audio file
    // - Transcribe with engine
    // - Save to database
    // - Handle output mode (print/copy/insert)
    // - Emit events
}
```

**Update: `src-tauri/src/audio/mod.rs`**
```rust
mod recorder;
mod detection;
mod spectrum;
pub mod recording; // NEW

pub use recorder::{AudioRecorder, AudioDeviceInfo, SampleRate, SampleRateOption};
pub use recording::{start, stop_and_transcribe}; // Re-export
```

### Phase B: Extract Settings Operations

**Create: `src-tauri/src/conf/operations.rs`**

```rust
// Settings-related operations
use crate::conf::{Settings, OutputMode, OsdPosition};
use crate::broadcast::BroadcastServer;
use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::AppHandle;

pub async fn set_output_mode(
    settings: &Arc<Mutex<Settings>>,
    mode: &str,
) -> Result<(), String> {
    // MOVE: Parsing + saving logic from commands.rs
    let output_mode = match mode {
        "print" => OutputMode::Print,
        "copy" => OutputMode::Copy,
        "insert" => OutputMode::Insert,
        _ => return Err(format!("Invalid output mode: {}", mode)),
    };
    
    let mut s = settings.lock().await;
    s.output_mode = output_mode;
    s.save().map_err(|e| e.to_string())
}

pub async fn set_window_decorations(
    settings: &Arc<Mutex<Settings>>,
    app: &AppHandle,
    enabled: bool,
) -> Result<(), String> {
    // MOVE: Logic from commands.rs
}

pub async fn set_osd_position(
    settings: &Arc<Mutex<Settings>>,
    broadcast: &BroadcastServer,
    position: &str,
) -> Result<(), String> {
    // MOVE: Parsing + saving + broadcasting logic from commands.rs
}
```

**Update: `src-tauri/src/conf/mod.rs`**
```rust
mod operations; // NEW

pub use operations::*; // Re-export
```

### Phase C: Create Thin Commands

**Create: `src-tauri/src/commands_new.rs`**

Each command becomes 3-10 lines that just delegates:

```rust
use tauri::{State, AppHandle};
use crate::state::{RecordingSession, TranscriptionState, Database};
use crate::conf::Settings;
use crate::broadcast::BroadcastServer;
use std::sync::Arc;
use tokio::sync::Mutex;

// ============================================================================
// RECORDING COMMANDS
// ============================================================================

#[tauri::command]
pub async fn toggle_recording(
    session: State<'_, RecordingSession>,
    transcription: State<'_, TranscriptionState>,
    settings: State<'_, Arc<Mutex<Settings>>>,
    broadcast: State<'_, BroadcastServer>,
    app: AppHandle,
) -> Result<String, String> {
    use crate::state::RecordingState;
    
    match session.get_state().await {
        RecordingState::Idle => {
            crate::audio::recording::start(&session, &settings, &broadcast, &app).await?;
            Ok("started".into())
        }
        RecordingState::Recording => {
            let db: Option<State<Database>> = app.try_state();
            crate::audio::recording::stop_and_transcribe(
                &session, 
                &transcription, 
                &settings, 
                &broadcast,
                db.as_ref().map(|d| d.inner()),
                &app
            ).await?;
            Ok("stopping".into())
        }
        RecordingState::Transcribing => Ok("busy".into()),
    }
}

#[tauri::command]
pub async fn get_status(session: State<'_, RecordingSession>) -> Result<String, String> {
    use crate::state::RecordingState;
    let state = session.get_state().await;
    let state_str = match state {
        RecordingState::Idle => "idle",
        RecordingState::Recording => "recording",
        RecordingState::Transcribing => "transcribing",
    };
    Ok(state_str.into())
}

// ============================================================================
// SETTINGS COMMANDS
// ============================================================================

#[tauri::command]
pub async fn set_output_mode(
    settings: State<'_, Arc<Mutex<Settings>>>,
    mode: String,
) -> Result<(), String> {
    crate::conf::operations::set_output_mode(&settings, &mode).await
}

#[tauri::command]
pub async fn get_output_mode(
    settings: State<'_, Arc<Mutex<Settings>>>,
) -> Result<String, String> {
    let s = settings.lock().await;
    let mode_str = match s.output_mode {
        crate::conf::OutputMode::Print => "print",
        crate::conf::OutputMode::Copy => "copy",
        crate::conf::OutputMode::Insert => "insert",
    };
    Ok(mode_str.into())
}

#[tauri::command]
pub async fn set_window_decorations(
    settings: State<'_, Arc<Mutex<Settings>>>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    crate::conf::operations::set_window_decorations(&settings, &app, enabled).await
}

// ... etc for all 24 commands

// ============================================================================
// HISTORY/DATABASE COMMANDS
// ============================================================================

#[tauri::command]
pub async fn get_transcription_history(
    db: State<'_, Database>,
    limit: i64,
    offset: i64,
) -> Result<Vec<crate::history::TranscriptionHistory>, String> {
    crate::history::list_transcriptions(db.pool(), limit, offset)
        .await
        .map_err(|e| e.to_string())
}

// ... etc
```

### Phase D: Database Query Layer (REQUIRED - Create db/ folder)

**Create `src-tauri/src/db/mod.rs`:**
```rust
pub mod transcriptions;
```

**Create `src-tauri/src/db/transcriptions.rs`:**
```rust
// MOVE all query functions from history.rs
use sqlx::SqlitePool;
use crate::history::{TranscriptionHistory, NewTranscription};

pub async fn list(pool: &SqlitePool, limit: i64, offset: i64) 
    -> Result<Vec<TranscriptionHistory>, sqlx::Error> {
    // MOVE list_transcriptions() implementation here
}

pub async fn get(pool: &SqlitePool, id: i64) 
    -> Result<Option<TranscriptionHistory>, sqlx::Error> {
    // MOVE get_transcription() implementation here
}

pub async fn delete(pool: &SqlitePool, id: i64) 
    -> Result<bool, sqlx::Error> {
    // MOVE delete_transcription() implementation here
}

pub async fn search(pool: &SqlitePool, query: &str, limit: i64) 
    -> Result<Vec<TranscriptionHistory>, sqlx::Error> {
    // MOVE search_transcriptions() implementation here
}

pub async fn count(pool: &SqlitePool) 
    -> Result<i64, sqlx::Error> {
    // MOVE count_transcriptions() implementation here
}

pub async fn save(pool: &SqlitePool, transcription: NewTranscription) 
    -> Result<i64, sqlx::Error> {
    // MOVE save_transcription() implementation here
}
```

**Update `history.rs` - KEEP ONLY:**
- Type definitions: `TranscriptionHistory`, `NewTranscription`
- Builder methods: `with_duration()`, `with_model()`, etc.
- Remove all query functions (moved to db/)

**Commands use:**
```rust
#[tauri::command]
pub async fn get_transcription_history(
    db: State<'_, Database>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TranscriptionHistory>, String> {
    crate::db::transcriptions::list(db.pool(), limit, offset)
        .await
        .map_err(|e| e.to_string())
}
```

## Implementation Order

### Step 0: Fix State Structures (DO FIRST)
- Add `SettingsState` wrapper to `state.rs` with config_mtime tracking
- Add `elapsed_ms()` to `RecordingSession` 
- Update `lib.rs` to use `SettingsState` instead of bare Arc<Mutex<Settings>>

### Step 1: Create db/ folder structure
- Create `db/mod.rs`
- Create `db/transcriptions.rs` - MOVE all query functions from history.rs
- Update `history.rs` - KEEP ONLY types and builders
- Update `lib.rs` to include `mod db;`

### Step 2: Create audio/recording.rs
- Extract `start_recording()` helper
- Extract `stop_and_transcribe()` helper  
- Update `audio/mod.rs` to expose them

### Step 3: Create conf/operations.rs
- Extract `set_output_mode()`
- Extract `set_window_decorations()`
- Extract `set_osd_position()`
- Update `conf/mod.rs` to expose them

### Step 4: Create commands_new.rs
- Write all 24 commands as thin delegators
- Each command: 3-10 lines max
- Just parameter extraction + delegation
- Use `SettingsState` for settings commands
- Use `session.elapsed_ms()` for timestamps

### Step 5: Swap Commands
- Update `lib.rs` to use `commands_new` instead of `commands`
- Test compilation
- Delete old `commands.rs` when working

### Step 6: Error Types (Future Enhancement)
- Add `thiserror` dependency
- Create `errors.rs`
- Replace `String` errors with typed errors

## File Structure After Refactoring

```
src-tauri/src/
├── audio/
│   ├── mod.rs
│   ├── recorder.rs          # Existing
│   ├── detection.rs         # Existing
│   ├── spectrum.rs          # Existing
│   └── recording.rs         # NEW - recording orchestration
│
├── conf/
│   ├── mod.rs               # Updated - re-export operations
│   └── operations.rs        # NEW - settings operations
│
├── db/
│   ├── mod.rs               # NEW
│   └── transcriptions.rs    # NEW - DB queries moved from history.rs
│
├── state.rs                 # Updated - RecordingSession + SettingsState + TranscriptionState + Database
├── commands.rs              # OLD - to be deleted
├── commands_new.rs          # NEW - thin delegators (becomes commands.rs)
├── lib.rs                   # Updated - uses commands_new + mod db
└── history.rs               # Updated - ONLY types and builders (queries moved to db/)
```

## Benefits of This Approach

✅ **Logic lives near related code**
- Recording logic with `audio/`
- Settings logic with `conf/`
- DB queries with `history.rs`

✅ **Thin command layer**
- Easy to see what commands exist
- Minimal duplication
- Clear delegation pattern

✅ **Testable**
- Can test `audio::recording::start()` without Tauri
- Can test `conf::operations::set_output_mode()` without Tauri
- Commands are just thin wrappers

✅ **Maintainable**
- Changes to recording logic → edit `audio/recording.rs`
- Changes to settings → edit `conf/operations.rs`
- Commands rarely need changes

## Next Actions

1. **Fix state.rs** - Add SettingsState wrapper + elapsed_ms to RecordingSession
2. **Create db/ folder** - Move queries from history.rs
3. **Create** `audio/recording.rs` with extracted logic
4. **Create** `conf/operations.rs` with extracted logic
5. **Create** `commands_new.rs` with thin delegators
6. **Update** `lib.rs` to import from `commands_new` and use SettingsState
7. **Test** compilation
8. **Delete** old `commands.rs`
9. **Commit** with message about architectural refactoring

## Features Being PRESERVED (Not Removed!)

✅ **Config change detection** - via SettingsState wrapper with config_mtime
✅ **Elapsed time tracking** - via RecordingSession.elapsed_ms() for OSD timestamps
✅ **Database folder** - All queries moved to db/transcriptions.rs
✅ **All existing functionality** - Just better organized!
