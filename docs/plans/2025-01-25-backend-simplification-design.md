# Backend Simplification Design

## Problem

The backend has ~670 lines of real logic spread across ~2200 lines in 22+ files. Key issues:

1. **Duplication**: The stop→transcribe→output flow is duplicated 3 times (`commands/recording.rs`, `cli.rs` x2)
2. **Over-modularization**: `commands/` (5 files of thin wrappers), `audio/` (4 files), `platform/` (8 files, 4 levels deep)
3. **False abstractions**: `platform/` pretends to be cross-platform for a Linux-only app
4. **Scattered state**: `state.rs` centralizes state types that each belong with their domain

## Solution

Consolidate to 11 top-level files with clear responsibilities based on actual runtime behavior.

### Final Structure

```
src-tauri/src/
├── lib.rs            # Tauri setup, state initialization
├── main.rs           # Entry point
├── recording.rs      # NEW: capture + shortcuts + state machine
├── transcription.rs  # EXPANDED: + output delivery
├── models.rs         # Unchanged: model types, download/management
├── conf.rs           # Unchanged (minus OutputMode::deliver())
├── broadcast.rs      # Simplified: one event type
├── commands.rs       # FLATTENED: all Tauri commands
├── cli.rs            # Thinner: just calls recording::toggle_recording()
├── db.rs             # Unchanged
├── tray.rs           # Unchanged
├── osd.rs            # Unchanged (facade)
└── osd/              # Unchanged (Iced overlay)
```

Pattern: `{name}.rs` + `{name}/` for submodules (no `mod.rs`)

### What Gets Consolidated

| Current | Becomes | Notes |
|---------|---------|-------|
| `audio.rs` + `audio/` (4 files) | `recording.rs` | Capture pipeline |
| `platform/` (8 files) | Dissolved | Shortcuts → `recording.rs`, text insertion → `transcription.rs` |
| `commands/` (5 files) | `commands.rs` | Flat file |
| `state.rs` | Dissolved | States move to their domains |

### Core Flow

```
recording.rs: hotkey → capture → audio file
                ↓
transcription.rs: audio file → inference → text → output
```

`transcription.rs` is the shared service - recording calls it now, file upload will call it later.

### Key Changes

1. **`recording.rs`** (~450 lines)
   - Shortcut backend trait + X11/Wayland/Fallback implementations
   - CPAL audio capture + spectrum analysis
   - `RecordingState` + `RecordingPhase` state machine
   - Public entry: `toggle_recording(app: &AppHandle)`

2. **`transcription.rs`** (~400 lines)
   - Engine loading (Whisper/Parakeet)
   - Inference
   - Entity + persistence
   - Output delivery (print/copy/insert + display detection + xdotool/wtype)
   - `TranscriptionState`
   - Public entry: `transcribe_and_deliver(audio_path: &Path, app: &AppHandle)`

3. **`commands.rs`** (~350 lines)
   - All 30 Tauri commands in one flat file
   - Thin wrappers that call domain functions

4. **`broadcast.rs`** (~150 lines)
   - Single `Message` enum (remove `TauriEvent` duplication)
   - Emit directly to Tauri

5. **`conf.rs`**
   - Remove `OutputMode::deliver()` (moves to `transcription.rs`)
   - Pure configuration data + load/save

6. **`cli.rs`**
   - Remove duplicated orchestration
   - Just calls `recording::toggle_recording()`

### Deleted Entirely

- `audio.rs` (facade)
- `audio/` directory
- `platform.rs` (facade)  
- `platform/` directory
- `commands/` directory
- `state.rs`

### Unchanged

- `models.rs` - already cohesive
- `db.rs` - small infrastructure
- `tray.rs` - small, focused
- `osd.rs` + `osd/` - genuinely separate UI framework
- `lib.rs` - setup (cleaner state init)
- `main.rs` - entry point

## Benefits

- **22+ files → 11 files** (not counting `osd/` internals)
- **3x duplication → 1 function**
- **8-file platform/ hierarchy → 0 files** (code inlined where used)
- **Clear ownership**: recording owns capture, transcription owns inference + output
- **Future-ready**: file upload just calls `transcription::transcribe_and_deliver()`

## Non-Goals

- Changing the OSD (Iced) architecture
- Refactoring the frontend (Svelte)
- Changing the database schema
- Adding new features
