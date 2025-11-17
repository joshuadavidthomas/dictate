# Refactoring Plan: Split Monolithic State & Improve Organization

## Current Problems

1. **Monolithic AppState** - All 24 commands get access to 11 fields, even when they only need 1-2
2. **Logic in commands.rs** - 706 lines mixing command handlers with business logic
3. **String-based errors** - `Result<T, String>` loses context and is hard to pattern match
4. **SQL mixed with types** - Database queries in `history.rs` alongside type definitions

## Goal

Move logic **closer to the code it uses** - not a centralized "services" layer, but organized by domain:
- Recording logic → with `audio/` module
- Database queries → in `db/` module  
- Settings logic → with `conf/` module
- Commands → thin handlers that delegate

## Phase 1: Split Monolithic State

### Current Structure
```rust
// state.rs - Everything in one struct
pub struct AppState {
    pub recording_state: Mutex<RecordingState>,
    pub recorder: Arc<Mutex<Option<AudioRecorder>>>,
    pub engine: Arc<Mutex<Option<TranscriptionEngine>>>,
    pub model_manager: Arc<Mutex<Option<ModelManager>>>,
    pub current_recording: Mutex<Option<ActiveRecording>>,
    pub broadcast: BroadcastServer,
    pub start_time: Instant,
    pub output_mode: Mutex<OutputMode>,
    pub settings: Arc<Mutex<Settings>>,
    pub last_modified_at: Arc<Mutex<Option<SystemTime>>>,
    pub db_pool: Arc<Mutex<Option<SqlitePool>>>,
}

// commands.rs - Everything gets AppState
#[tauri::command]
pub async fn get_output_mode(
    state: State<'_, AppState>  // Gets all 11 fields!
) -> Result<String, String> {
    // Only uses state.output_mode
}
```

### New Structure

Create separate state modules:

```
src-tauri/src/
├── state/
│   ├── mod.rs              # Re-exports
│   ├── database.rs         # Database state
│   ├── recorder.rs         # Recording state  
│   └── settings.rs         # Settings state
```

#### state/database.rs
```rust
use sqlx::SqlitePool;

/// Manages database connection pool
#[derive(Clone)]
pub struct Database(SqlitePool);

impl Database {
    pub fn new(pool: SqlitePool) -> Self {
        Self(pool)
    }
    
    pub fn pool(&self) -> &SqlitePool {
        &self.0
    }
}
```

#### state/recorder.rs
```rust
use tokio::sync::Mutex;
use std::sync::Arc;
use crate::audio::AudioRecorder;
use crate::transcription::TranscriptionEngine;
use crate::broadcast::BroadcastServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Transcribing,
}

pub struct ActiveRecording {
    pub audio_buffer: Arc<std::sync::Mutex<Vec<i16>>>,
    pub stop_signal: Arc<std::sync::atomic::AtomicBool>,
    pub stream: Option<cpal::Stream>,
    pub start_time: std::time::Instant,
}

/// Manages recording-related state
#[derive(Clone)]
pub struct Recorder {
    state: Arc<Mutex<RecordingState>>,
    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    engine: Arc<Mutex<Option<TranscriptionEngine>>>,
    current_recording: Arc<Mutex<Option<ActiveRecording>>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            recorder: Arc::new(Mutex::new(None)),
            engine: Arc::new(Mutex::new(None)),
            current_recording: Arc::new(Mutex::new(None)),
        }
    }
    
    pub async fn get_state(&self) -> RecordingState {
        *self.state.lock().await
    }
    
    pub async fn set_state(&self, new_state: RecordingState) {
        *self.state.lock().await = new_state;
    }
    
    // Accessors for internal fields
    pub fn recorder(&self) -> &Arc<Mutex<Option<AudioRecorder>>> {
        &self.recorder
    }
    
    pub fn engine(&self) -> &Arc<Mutex<Option<TranscriptionEngine>>> {
        &self.engine
    }
    
    pub fn current_recording(&self) -> &Arc<Mutex<Option<ActiveRecording>>> {
        &self.current_recording
    }
}
```

#### state/settings.rs
```rust
use tokio::sync::Mutex;
use std::sync::Arc;
use std::time::SystemTime;
use crate::conf::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Print,
    Copy,
    Insert,
}

/// Manages application settings
#[derive(Clone)]
pub struct SettingsState {
    settings: Arc<Mutex<Settings>>,
    last_modified_at: Arc<Mutex<Option<SystemTime>>>,
    output_mode: Arc<Mutex<OutputMode>>,
}

impl SettingsState {
    pub fn new(settings: Settings) -> Self {
        let output_mode = settings.output_mode;
        let last_modified_at = crate::conf::config_last_modified_at().ok();
        
        Self {
            settings: Arc::new(Mutex::new(settings)),
            last_modified_at: Arc::new(Mutex::new(last_modified_at)),
            output_mode: Arc::new(Mutex::new(output_mode)),
        }
    }
    
    pub async fn get_output_mode(&self) -> OutputMode {
        *self.output_mode.lock().await
    }
    
    pub async fn set_output_mode(&self, mode: OutputMode) {
        *self.output_mode.lock().await = mode;
    }
    
    pub fn settings(&self) -> &Arc<Mutex<Settings>> {
        &self.settings
    }
    
    pub fn last_modified_at(&self) -> &Arc<Mutex<Option<SystemTime>>> {
        &self.last_modified_at
    }
}
```

#### state/mod.rs
```rust
mod database;
mod recorder;
mod settings;

pub use database::Database;
pub use recorder::{Recorder, RecordingState, ActiveRecording};
pub use settings::{SettingsState, OutputMode};

// BroadcastServer stays simple - just clone
pub use crate::broadcast::BroadcastServer;
```

### Updated lib.rs Setup
```rust
use state::{Database, Recorder, SettingsState, BroadcastServer};

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Initialize separate states
            let settings = conf::Settings::load();
            let broadcast = BroadcastServer::new();
            
            app.manage(SettingsState::new(settings));
            app.manage(Recorder::new());
            app.manage(broadcast.clone());
            
            // Initialize database asynchronously
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match db::init_db().await {
                    Ok(pool) => {
                        app_handle.manage(Database::new(pool));
                        eprintln!("Database initialized");
                    }
                    Err(e) => eprintln!("Failed to init DB: {}", e),
                }
            });
            
            // ... rest of setup
            Ok(())
        })
        .invoke_handler(/* ... */)
}
```

### Updated Commands - Now Type-Safe
```rust
// Before: Gets everything
#[tauri::command]
pub async fn get_output_mode(
    state: State<'_, AppState>
) -> Result<String, String> {
    let mode = state.output_mode.lock().await;
    Ok(format!("{:?}", mode).to_lowercase())
}

// After: Gets only what it needs
#[tauri::command]
pub async fn get_output_mode(
    settings: State<'_, SettingsState>
) -> Result<String, String> {
    let mode = settings.get_output_mode().await;
    Ok(format!("{:?}", mode).to_lowercase())
}
```

```rust
// Before: Gets everything
#[tauri::command]
pub async fn get_transcription_history(
    state: State<'_, AppState>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TranscriptionHistory>, String> {
    let pool = state.db_pool.lock().await;
    let pool = pool.as_ref().ok_or("Database not initialized")?;
    // ...
}

// After: Gets only database
#[tauri::command]
pub async fn get_transcription_history(
    db: State<'_, Database>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TranscriptionHistory>, String> {
    crate::db::transcription_queries::list(db.pool(), limit, offset)
        .await
        .map_err(|e| e.to_string())
}
```

## Phase 2: Move Logic Closer to Where It's Used

Instead of a centralized "services" folder, organize logic by feature/domain:

### audio/ - Recording Logic

Create `audio/recording.rs` for recording orchestration:

```rust
// audio/recording.rs
use crate::state::{Recorder, RecordingState, SettingsState, Database};
use crate::broadcast::BroadcastServer;
use tauri::{AppHandle, Emitter};

/// Start recording - orchestrates the recording flow
pub async fn start(
    recorder: &Recorder,
    settings: &SettingsState,
    broadcast: &BroadcastServer,
    app: &AppHandle,
) -> Result<(), String> {
    // Check current state
    let current = recorder.get_state().await;
    if current != RecordingState::Idle {
        return Err("Already recording".into());
    }
    
    // Set state
    recorder.set_state(RecordingState::Recording).await;
    
    // Emit event
    app.emit("recording-started", StatusUpdate {
        state: "recording".into()
    }).ok();
    
    // Create recorder
    let settings_lock = settings.settings().lock().await;
    let sample_rate = settings_lock.sample_rate;
    let device_name = settings_lock.audio_device.clone();
    drop(settings_lock);
    
    let audio_recorder = AudioRecorder::new_with_device(
        device_name.as_deref(),
        sample_rate
    )?;
    
    *recorder.recorder().lock().await = Some(audio_recorder);
    
    // Start recording loop
    start_recording_loop(recorder, broadcast).await?;
    
    Ok(())
}

/// Stop recording and transcribe
pub async fn stop(
    recorder: &Recorder,
    settings: &SettingsState,
    db: &Database,
    app: &AppHandle,
) -> Result<(), String> {
    // ... stop logic
}

// Helper functions
async fn start_recording_loop(
    recorder: &Recorder,
    broadcast: &BroadcastServer,
) -> Result<(), String> {
    // Recording loop implementation
}
```

### db/ - Database Queries

Split `history.rs` into types and queries:

```
src-tauri/src/
├── history.rs          # KEEP: Types + builders
└── db/
    ├── mod.rs
    └── transcription_queries.rs  # NEW: SQL queries
```

```rust
// history.rs - Just types and builders
pub struct TranscriptionHistory { /* ... */ }
pub struct NewTranscription { /* ... */ }

impl NewTranscription {
    pub fn new(text: String) -> Self { /* ... */ }
    pub fn with_duration(mut self, duration_ms: i64) -> Self { /* ... */ }
    // ... other builders
}
```

```rust
// db/transcription_queries.rs - SQL queries
use sqlx::SqlitePool;
use crate::history::{TranscriptionHistory, NewTranscription};

pub async fn save(
    pool: &SqlitePool,
    transcription: NewTranscription,
) -> Result<i64, sqlx::Error> {
    // SQL here
}

pub async fn list(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TranscriptionHistory>, sqlx::Error> {
    // SQL here
}

pub async fn search(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<TranscriptionHistory>, sqlx::Error> {
    // SQL here
}

pub async fn delete(
    pool: &SqlitePool,
    id: i64,
) -> Result<bool, sqlx::Error> {
    // SQL here
}

pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    // SQL here
}
```

### conf/ - Settings Logic

Add `conf/operations.rs` for settings operations:

```rust
// conf/operations.rs
use crate::state::SettingsState;
use crate::conf::Settings;
use std::time::SystemTime;

/// Check if config file changed externally
pub async fn check_changed(settings_state: &SettingsState) -> Result<bool, String> {
    let last_seen_modified_at = settings_state.last_modified_at().lock().await;
    let file_last_modified_at = crate::conf::config_last_modified_at()
        .map_err(|e| format!("Failed to get config last modified time: {}", e))?;
    
    Ok(match *last_seen_modified_at {
        Some(last_seen) => file_last_modified_at > last_seen,
        None => false,
    })
}

/// Update stored config last_modified_at
pub async fn update_last_modified(settings_state: &SettingsState) -> Result<(), String> {
    let last_modified_at = crate::conf::config_last_modified_at()
        .map_err(|e| format!("Failed to get config last modified time: {}", e))?;
    *settings_state.last_modified_at().lock().await = Some(last_modified_at);
    Ok(())
}
```

### Updated commands.rs - Thin Handlers

```rust
// commands.rs - Just delegates to modules
use tauri::{State, AppHandle};
use crate::state::{Database, Recorder, SettingsState, BroadcastServer};

#[tauri::command]
pub async fn toggle_recording(
    recorder: State<'_, Recorder>,
    settings: State<'_, SettingsState>,
    db: State<'_, Database>,
    broadcast: State<'_, BroadcastServer>,
    app: AppHandle,
) -> Result<String, String> {
    let state = recorder.get_state().await;
    
    match state {
        RecordingState::Idle => {
            crate::audio::recording::start(&recorder, &settings, &broadcast, &app).await?;
            Ok("started".into())
        }
        RecordingState::Recording => {
            crate::audio::recording::stop(&recorder, &settings, &db, &app).await?;
            Ok("stopped".into())
        }
        RecordingState::Transcribing => {
            Err("Currently transcribing".into())
        }
    }
}

#[tauri::command]
pub async fn get_status(recorder: State<'_, Recorder>) -> Result<String, String> {
    let state = recorder.get_state().await;
    Ok(format!("{:?}", state).to_lowercase())
}

#[tauri::command]
pub async fn set_output_mode(
    settings: State<'_, SettingsState>,
    mode: String,
) -> Result<(), String> {
    crate::conf::operations::set_output_mode(&settings, &mode).await
}

#[tauri::command]
pub async fn get_transcription_history(
    db: State<'_, Database>,
    limit: i64,
    offset: i64,
) -> Result<Vec<crate::history::TranscriptionHistory>, String> {
    crate::db::transcription_queries::list(db.pool(), limit, offset)
        .await
        .map_err(|e| e.to_string())
}
```

## Phase 3: Error Types

Create `errors.rs`:

```rust
// errors.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("recording error: {0}")]
    Recording(String),
    
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    
    #[error("audio error: {0}")]
    Audio(String),
    
    #[error("config error: {0}")]
    Config(String),
    
    #[error("transcription error: {0}")]
    Transcription(String),
    
    #[error("{0}")]
    Other(String),
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}
```

Update Cargo.toml:
```toml
[dependencies]
thiserror = "2.0"
```

Then gradually replace `Result<T, String>` with `Result<T, AppError>`:

```rust
// Before
pub async fn start(...) -> Result<(), String> {
    Err("Failed".into())
}

// After
pub async fn start(...) -> Result<(), AppError> {
    Err(AppError::Recording("Failed".into()))
}

// Commands still return String for Tauri
#[tauri::command]
pub async fn toggle_recording(...) -> Result<String, String> {
    crate::audio::recording::start(...)
        .await
        .map_err(|e| e.to_string())?;  // Convert to string at boundary
    Ok("started".into())
}
```

## Final Structure

```
src-tauri/src/
├── audio/
│   ├── mod.rs
│   ├── recorder.rs        # AudioRecorder implementation
│   ├── detection.rs
│   └── recording.rs       # NEW: Recording orchestration logic
│
├── db/
│   ├── mod.rs
│   └── transcription_queries.rs  # NEW: SQL queries
│
├── state/
│   ├── mod.rs
│   ├── database.rs        # NEW: Database state
│   ├── recorder.rs        # NEW: Recorder state
│   └── settings.rs        # NEW: Settings state
│
├── conf/
│   ├── mod.rs
│   └── operations.rs      # NEW: Settings operations
│
├── history.rs             # KEEP: Types + builders
├── commands.rs            # REFACTOR: Thin handlers
├── errors.rs              # NEW: Error types
└── ... (other modules)
```

## Migration Strategy

### Week 1: State Split
1. Create `state/` module structure
2. Create `Database`, `Recorder`, `SettingsState` types
3. Update `lib.rs` to manage separate states
4. Update 2-3 simple commands (e.g., `get_status`, `get_output_mode`)
5. Test that it works

### Week 2: Move DB Logic
1. Create `db/transcription_queries.rs`
2. Move SQL from `history.rs` to queries module
3. Update all history-related commands
4. Keep types in `history.rs`

### Week 3: Extract Recording Logic
1. Create `audio/recording.rs`
2. Move `toggle_recording` logic out of commands.rs
3. Create `start()` and `stop()` functions
4. Update command to delegate

### Week 4: Error Types
1. Add `thiserror` dependency
2. Create `errors.rs`
3. Replace `String` errors in new modules
4. Keep string conversion at command boundary

## Benefits You'll Get

✅ **Explicit Dependencies** - Commands only get what they need
✅ **Easier Testing** - Mock just Database, not all 11 fields  
✅ **No Option Hell** - If it's managed, it exists
✅ **Better Organization** - Logic lives near related code
✅ **Clearer Errors** - Rich error types with context
✅ **Maintainability** - Changes are localized

## What NOT to Do

❌ Don't create a `services/` folder - organize by feature instead
❌ Don't create a `domain/` folder - types are fine in `history.rs`
❌ Don't add trait abstractions unless you need them
❌ Don't rush - do it incrementally, one phase at a time
