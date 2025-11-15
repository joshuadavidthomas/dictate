# Tauri Migration Status

## ✅ Completed

1. **Project Setup**
   - Created Tauri + Svelte project
   - Migrated all Rust code from `src-bak/` to `src-tauri/src/`
   - Updated Cargo.toml with all dependencies
   - Set Rust edition to 2024 (for let-chains)

2. **State Management**
   - Created simple `AppState` (no Arc<AtomicBool> mess!)
   - Enum-based state: Idle, Recording, Transcribing
   - Clean state transitions

3. **Tauri Commands**
   - `toggle_recording` - Start/stop recording
   - `get_status` - Get current state
   - Events: `recording-started`, `recording-stopped`, `transcription-complete`

4. **Svelte UI**
   - Clean modern interface
   - Real-time status updates
   - Toggle button with visual feedback
   - Dark mode styled

## 🚧 Next Steps

### Phase 1: Core Recording (High Priority)
- [ ] Wire up actual AudioRecorder in toggle_recording
- [ ] Implement recording to buffer/file
- [ ] Add transcription after recording stops
- [ ] Test basic record → transcribe flow

### Phase 2: iced OSD Integration
- [ ] Spawn iced layer-shell overlay when recording starts
- [ ] Pass state updates to iced process
- [ ] Show spectrum visualization during recording

### Phase 3: System Integration
- [ ] Add system tray icon
- [ ] Hide window to tray (don't quit)
- [ ] Small CLI tool for hotkey invocation
- [ ] Test Wayland hotkey workflow

### Phase 4: Settings & Models
- [ ] Settings page (model selection, audio device, etc.)
- [ ] Model download UI
- [ ] Transcription history viewer

## Architecture

```
dictate (Tauri App)
├── Rust Backend (src-tauri/src/)
│   ├── state.rs - Simple state management
│   ├── commands.rs - Tauri IPC commands
│   ├── audio/ - Recording (keep existing)
│   ├── transcription/ - Engine (keep existing)
│   └── ui/ - iced OSD (keep existing)
│
├── Svelte Frontend (src/)
│   └── routes/+page.svelte - Main UI
│
└── Future: CLI tool for hotkeys
    └── Sends commands to Tauri backend
```

## Key Benefits

- ✅ No socket races (single process)
- ✅ Simple state (Mutex instead of Arc<AtomicBool>)
- ✅ Svelte UI (fast development)
- ✅ Keep iced OSD (Wayland-native)
- ✅ System tray support
- ✅ Cross-platform ready

## Commands

```bash
# Development
npm run tauri dev

# Build
npm run tauri build

# Just Rust
cd src-tauri && cargo build
```
