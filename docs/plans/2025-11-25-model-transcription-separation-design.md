# Model & Transcription Separation Design

_Date: 2025-11-25_

## Context & Problem
- `models.rs` currently mixes three concerns: static model metadata, filesystem/download orchestration, and APIs used by the transcription runtime.
- `transcription.rs` depends on `ModelManager` to discover model paths, so the inference layer knows about storage details and download status.
- `recording.rs` indirectly feels these leaks because `transcribe_and_deliver` juggles settings, downloads, cache state, and output in one place.
- Result: responsibilities are complected, terminology is confusing (`TranscriptionEngine` vs actual engines), and the codebase is larger than necessary for a brand-new app.

## Goals
1. **Single responsibility per domain**
   - `models` owns catalog + storage.
   - `transcription` owns runtime inference cache.
   - `recording` just captures audio and delivers output.
2. **Obvious placement**: behavior clearly belongs to one module.
3. **Less code / less ceremony**: delete wrapper structs/traits that exist only to shuttle data around.
4. **No feature regressions**: UI model management, transcription flow, and settings continue to behave the same.

## Architecture Overview
```
recording -> transcription::transcribe_and_deliver()
                 |
                 v
          ensure_loaded_engine(settings)
                 |
                 v
        models::catalog + models::storage
```
- `models::catalog`: pure data (`ModelDescriptor` table, lookup helpers).
- `models::storage`: filesystem & network helpers (paths, download/remove, stats).
- `transcription`: mutex-protected `(ModelId, LoadedEngine)` cache + inference runner.
- `recording`: unchanged, never touches model storage.

## Data Structures & APIs
### Catalog
```rust
pub struct ModelDescriptor {
    pub id: ModelId,
    pub storage_name: &'static str,
    pub is_directory: bool,
    pub download_url: &'static str,
}

pub fn all_models() -> &'static [ModelDescriptor];
pub fn find(id: ModelId) -> Option<&'static ModelDescriptor>;
pub fn preferred_or_default(pref: Option<ModelId>) -> &'static ModelDescriptor; // preferred -> Parakeet V3 -> Whisper Base
```
No runtime state; used by both UI commands and transcription fallback logic.

### Storage
```rust
pub fn models_dir() -> Result<PathBuf>;
pub fn local_path(id: ModelId) -> Result<PathBuf>;        // builds path regardless of download state
pub fn is_downloaded(id: ModelId) -> Result<bool>;
pub async fn download(id: ModelId, progress: ProgressFn) -> Result<()>;
pub async fn remove(id: ModelId) -> Result<()>;
pub fn storage_info() -> Result<StorageInfo>;             // total bytes, counts
```
`download/remove` reuse existing logic (tar extraction, Apple cleanup) but live here, not in a manager struct.

### Transcription Cache
```rust
type EngineCache = Mutex<Option<(ModelId, LoadedEngine)>>;

enum LoadedEngine {
    Whisper { engine: WhisperEngine },
    Parakeet { engine: ParakeetEngine },
}

impl LoadedEngine {
    fn transcribe(&mut self, audio_path: &Path) -> Result<String>;
}

async fn ensure_loaded_engine(
    cache: &EngineCache,
    settings: &SettingsState,
) -> Result<MutexGuard<'_, Option<(ModelId, LoadedEngine)>>> {
    let descriptor = catalog::preferred_or_default(settings.preferred_model);
    let path = storage::local_path(descriptor.id)?;
    // verify download or error out
    // load engine iff cache missing or ID mismatch
}
```
`transcribe_and_deliver` locks once, uses `LoadedEngine::transcribe`, records the `ModelId` on the resulting `Transcription` (wrapped in `Some`).

### Commands / UI
- `list_models` uses `catalog::all_models()` and `storage::is_downloaded` to build UI rows.
- `download_model/remove_model` call into `storage` helpers directly.
- `get_model_storage_info` proxies `storage_info()`.
- Preferred model validation simply checks `catalog::find(id).is_some()`.

## Control Flow
1. **Recording** collects PCM, calls `transcription::transcribe_and_deliver`.
2. **Transcription**:
   - Locks `EngineCache`.
   - Calls `ensure_loaded_engine` which:
     - Picks `(ModelId, ModelDescriptor)` via catalog fallback.
     - Resolves local path and confirms it exists (if missing, return “download model first” error).
     - Loads Whisper/Parakeet if cache empty or ID changed, storing `(ModelId, LoadedEngine)`.
   - Runs `LoadedEngine::transcribe(audio_path)`.
   - Persists transcription (with `model_id: Some(id)`), delivers output as before.
3. **Model download/remove** flows stay exactly the same but routed through `models::storage`.

## Error Handling
- `ensure_loaded_engine` emits three clear errors:
  1. `ModelNotDownloaded(id)` when `local_path` missing.
  2. `ModelLoadFailed { id, source }` when Whisper/Parakeet constructors fail.
  3. `NoModelAvailable` when neither preferred nor fallback exists.
- Download/remove commands propagate `anyhow` context (URL, dest path, disk space message).
- Cache stays `None` on load failure so next attempt retries after user resolves files.

## Testing Strategy
1. **Unit tests**
   - `catalog`: fallback order, descriptor metadata.
   - `storage`: `local_path` naming, `is_downloaded` using temp dirs, `storage_info` on fake tree.
   - `ensure_loaded_engine`: use a stub `LoadedEngine::from_path` behind a trait to validate cache behavior without real models.
2. **Integration/manual**
   - `cargo test -p dictate` (Rust) + `pnpm test` (Svelte) remain as-is; manually hit tauri commands for download/remove/list.
   - Run transcription flow with/without downloaded models to confirm error surfacing.

## Migration Steps
1. Extract catalog metadata into `models/catalog.rs`; delete `ModelSpec`, `ModelInfo`, `ModelManager`.
2. Create `models/storage.rs` holding path/download/remove/storage-info helpers; move existing logic here.
3. Update `commands.rs` to use the new helpers (no struct instantiation).
4. Replace the transcription cache with `(ModelId, LoadedEngine)`, wiring it to `storage::local_path`.
5. Delete `TranscriptionEngine` wrapper + redundant options; rename types for clarity.
6. Run `cargo fmt` & `cargo check` to verify.

## Open Questions / Future Work
- Auto-trigger model download from transcription when missing? (Out of scope now.)
- Persist multiple models simultaneously? (Currently only one cached; good enough for MVP.)
- Surface better UI messaging for missing models (consider toasts vs console logs).

Once implemented, each domain has one responsibility, behavior placement is obvious, and we delete significant scaffolding—matching the entropy goals.