# Consolidate ProjectDirs Usage Implementation Plan

**Goal:** Centralize ProjectDirs creation in a single `conf::get_project_dirs()` function, eliminating duplication across 4 files and standardizing on `("", "", "dictate")` parameters.

**Architecture:** Create `conf::get_project_dirs()` function that returns the ProjectDirs instance. All modules call this function and then use `.data_dir()`, `.cache_dir()`, or `.config_dir()` as needed.

**Tech Stack:** Rust, directories crate

---

## Current State

**ProjectDirs usage scattered across 4 files:**

1. `src-tauri/src/conf.rs`: Uses `ProjectDirs::from("", "", "dictate")` for config path
2. `src-tauri/src/db.rs`: Uses `ProjectDirs::from("com", "dictate", "dictate")` for database
3. `src-tauri/src/recording.rs`: Uses `ProjectDirs::from("com", "dictate", "dictate")` for recordings
4. `src-tauri/src/models.rs`: Uses `ProjectDirs::from("com", "dictate", "dictate")` for models

**Issues:**
- Duplication of ProjectDirs creation logic
- Inconsistent parameters (`""` vs `"com"`)
- Cannot change project directories in one place

---

## Task 1: Add get_project_dirs() to conf.rs

**Files:**
- Modify: `src-tauri/src/conf.rs`

**Step 1: Add get_project_dirs() function**

Add after the existing imports and before the OutputMode enum (after line 8):

```rust
/// Get the ProjectDirs instance for dictate
/// 
/// Uses ("", "", "dictate") for XDG-compliant directories:
/// - data_dir: ~/.local/share/dictate (on Linux)
/// - cache_dir: ~/.cache/dictate (on Linux)
/// - config_dir: ~/.config/dictate (on Linux)
pub fn get_project_dirs() -> anyhow::Result<ProjectDirs> {
    ProjectDirs::from("", "", "dictate")
        .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))
}
```

**Step 2: Update config_path() to use get_project_dirs()**

Replace the existing `config_path()` function:

```rust
/// Get the path to the config file: ~/.config/dictate/config.toml
pub fn config_path() -> Option<PathBuf> {
    get_project_dirs()
        .ok()
        .map(|dirs| dirs.config_dir().join("config.toml"))
}
```

**Step 3: Verify it compiles**

Run: `cd src-tauri && cargo check`

Expected: Compiles successfully

**Step 4: Commit**

```bash
git add src-tauri/src/conf.rs
git commit -m "feat(conf): add get_project_dirs for centralized directory management"
```

---

## Task 2: Update db.rs to use conf::get_project_dirs

**Files:**
- Modify: `src-tauri/src/db.rs`

**Step 1: Update imports**

Change:
```rust
use directories::ProjectDirs;
```

To:
```rust
use crate::conf;
```

**Step 2: Update get_db_path() function**

Replace the existing function (around line 13):

```rust
pub fn get_db_path() -> Result<PathBuf> {
    Ok(conf::get_project_dirs()?.data_dir().join("dictate.db"))
}
```

**Step 3: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 4: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "refactor(db): use conf::get_project_dirs for database path"
```

---

## Task 3: Update recording.rs to use conf::get_project_dirs

**Files:**
- Modify: `src-tauri/src/recording.rs`

**Step 1: Remove ProjectDirs import**

The import `use directories::ProjectDirs;` should be removed (it's around line 8).

**Step 2: Update stop_recording() function**

Find the recordings_dir block (around line 1550-1557) and replace:

```rust
let recordings_dir = {
    let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
        .ok_or_else(|| anyhow!("Failed to get project directories"))?;
    let dir = project_dirs.data_dir().join("recordings");
    tokio::fs::create_dir_all(&dir).await?;
    dir
};
```

With:

```rust
let recordings_dir = {
    let dir = crate::conf::get_project_dirs()?.data_dir().join("recordings");
    tokio::fs::create_dir_all(&dir).await?;
    dir
};
```

**Step 3: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 4: Commit**

```bash
git add src-tauri/src/recording.rs
git commit -m "refactor(recording): use conf::get_project_dirs for recordings directory"
```

---

## Task 4: Update models.rs to use conf::get_project_dirs

**Files:**
- Modify: `src-tauri/src/models.rs`

**Step 1: Update imports**

Change:
```rust
use directories::ProjectDirs;
```

To:
```rust
use crate::conf;
```

**Step 2: Update ModelManager::new() function**

Find the models_dir initialization (around lines 60-65) and replace:

```rust
let project_dirs = ProjectDirs::from("com", "dictate", "dictate")
    .ok_or_else(|| anyhow!("Failed to get project directories"))?;

let models_dir = project_dirs.data_dir().join("models");
```

With:

```rust
let models_dir = conf::get_project_dirs()?.data_dir().join("models");
```

**Step 3: Verify it compiles**

Run: `cd src-tauri && cargo check`

**Step 4: Commit**

```bash
git add src-tauri/src/models.rs
git commit -m "refactor(models): use conf::get_project_dirs for models directory"
```

---

## Task 5: Final verification

**Step 1: Clean build**

Run: `cd src-tauri && cargo clean && cargo build`

Expected: Clean build succeeds

**Step 2: Run all tests**

Run: `cd src-tauri && cargo test`

Expected: All 18 tests pass

**Step 3: Check for clippy warnings**

Run: `cd src-tauri && cargo clippy`

Expected: No new warnings (fix any that appear)

**Step 4: Verify grep shows consolidation**

Run: `cd src-tauri && rg 'ProjectDirs::from' src/`

Expected: Should only appear in `conf.rs` in the `get_project_dirs()` function

**Step 5: Final commit**

```bash
git add -A
git commit -m "refactor: consolidate ProjectDirs usage to conf::get_project_dirs"
```

---

## Summary

**Files after refactoring:**
- `src-tauri/src/conf.rs`: +9 lines (new `get_project_dirs()` function), modified `config_path()`
- `src-tauri/src/db.rs`: Simplified to use `conf::get_project_dirs()`
- `src-tauri/src/recording.rs`: Simplified to use `conf::get_project_dirs()`
- `src-tauri/src/models.rs`: Simplified to use `conf::get_project_dirs()`

**Benefits:**
- ✅ Single source of truth for ProjectDirs parameters
- ✅ Can change directory structure in one place
- ✅ Consistent use of `("", "", "dictate")` across codebase
- ✅ Eliminated 3× duplication of ProjectDirs creation
- ✅ Minimal code - just one helper function, not a whole submodule
- ✅ Flexible - callers choose which directory they need

**Verification checklist:**
- [ ] `cargo check` passes
- [ ] `cargo test` passes (18 tests)
- [ ] `cargo clippy` has no new warnings
- [ ] `rg 'ProjectDirs::from'` only shows one occurrence in conf.rs
