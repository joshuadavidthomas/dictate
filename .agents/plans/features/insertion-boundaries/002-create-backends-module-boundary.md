# Plan 002: Move Wayland insertion behind a backends module

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback — do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from bd1fd406 --to @ -- src/insertion.rs src/lib.rs src/delivery.rs`
> If these files have changed since this plan was revised, compare the "Current state" excerpts against the live code before proceeding; on a mismatch, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: MED
- **Depends on**: `001-clarify-insertion-contract.md`
- **Planned at**: revision `8b4d975e`, 2026-07-08
- **Revised at**: revision `bd1fd406`, after removing the insert debug simulator

## Why this matters

`src/insertion.rs` is 1853 lines and mixes the product insertion contract with one concrete Wayland backend. Future backends should live under an explicit `backends/` seam instead of sharing a file with the domain contract. This plan changes module layout without changing behavior.

## Current state

- `src/lib.rs` declares `mod insertion;`; Rust currently resolves that to `src/insertion.rs`.
- `src/insertion.rs:27-63` contains the crate-level insertion contract.
- `src/insertion.rs:66` starts `WaylandInputMethodBackend`, after which almost all remaining code is Wayland-specific implementation and tests.
- `src/delivery.rs:16` imports `WaylandInputMethodBackend` from `crate::insertion`.
- There is no `src/insertion/` directory yet.

Conventions to match:

- Keep the public crate interface stable for internal callers: `crate::insertion::{InsertionBackend, InsertionOutcome, InsertionFailure, WaylandInputMethodBackend}` should still work after Plan 001 names are applied.
- Use `jj file move` for tracked file moves.
- Do not create compatibility aliases for old names.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Status/diff | `jj st && jj diff --stat` | only scoped files changed |
| Focused insertion tests | `cargo test --lib insertion::tests` | all pass |
| Delivery tests | `cargo test --lib delivery::tests` | all pass |
| Full check | `just check` | exit 0 |
| Full tests | `just test` | all pass |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| Format | `cargo +nightly fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/insertion.rs` moved to `src/insertion/mod.rs`
- `src/insertion/mod.rs`
- `src/insertion/outcome.rs`
- `src/insertion/backends/mod.rs`
- `src/insertion/backends/wayland/mod.rs`
- `src/delivery.rs`

**Out of scope**:
- Splitting Wayland internals into many files — Plans 003 and 004 own that.
- Changing insertion behavior or fallback policy.
- Changing `src/lib.rs` unless Rust module resolution requires it.

## Steps

### Step 1: Convert `src/insertion.rs` into a module directory

Use `jj file move src/insertion.rs src/insertion/mod.rs`.

At this point, `src/lib.rs` should still compile with `mod insertion;` because Rust resolves `src/insertion/mod.rs`.

**Verify**: `just check` → exit 0.

### Step 2: Extract the product contract into `outcome.rs`

Move the top-level insertion contract from `src/insertion/mod.rs` into `src/insertion/outcome.rs`:

- `InsertionBackend`
- `InsertionOutcome`
- `InsertionFailure`
- `InsertionFailure::protocol` helper if it remains shared only by the Wayland backend through `pub(super)`/`pub(crate)` visibility

In `src/insertion/mod.rs`, expose the contract with explicit re-exports:

```rust
mod outcome;
pub(crate) use outcome::{InsertionBackend, InsertionFailure, InsertionOutcome};
```

Keep the interface small. Do not expose Wayland internal helper types.

**Verify**: `cargo test --lib delivery::tests` → all pass.

### Step 3: Move the Wayland implementation under `backends/wayland`

Create:

```text
src/insertion/backends/mod.rs
src/insertion/backends/wayland/mod.rs
```

Move `WaylandInputMethodBackend`, `insert_with_input_method`, Wayland imports, dispatch implementations, private helpers, and insertion tests into `src/insertion/backends/wayland/mod.rs`.

In `src/insertion/backends/mod.rs`:

```rust
pub(crate) mod wayland;
```

In `src/insertion/mod.rs`:

```rust
pub(crate) mod backends;
pub(crate) use backends::wayland::WaylandInputMethodBackend;
```

The rest of the crate should continue importing `WaylandInputMethodBackend` from `crate::insertion` unless there is a strong reason to point directly at `crate::insertion::backends::wayland`.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 4: Keep tests close to the backend during the move

Move the existing `insertion::tests` module with the Wayland implementation for now. Do not try to redistribute tests across future files in this plan.

If module paths in test names change, that is acceptable. Behavior and assertions should not change except for Plan 001 naming updates.

**Verify**: `just test` → all pass.

### Step 5: Run review and final validation

Run the coding-standards review loop:

```rust
agent({
  name: "coding-standards-review",
  skills: ["coding-standards"],
  task: "Review the current jj diff for code quality, with focus on the coding-standards. Return findings with file paths and concrete fixes."
})
```

Address findings until clean.

**Verify**:
- `just check` → exit 0
- `just test` → all pass
- `cargo clippy --all-targets --all-features -- -D warnings` → exit 0
- `cargo +nightly fmt --check` → exit 0

## Test plan

No new behavior tests are expected. The plan is a module move. Existing tests must remain green:

- `cargo test --lib insertion::tests`
- `cargo test --lib delivery::tests`
- `just test`

## Done criteria

- [x] `src/insertion.rs` is now the lint-compatible insertion module root with only module declarations, re-exports, and minimal contract-facing glue.
- [x] `src/insertion/outcome.rs` owns the product insertion contract.
- [x] `src/insertion/backends/wayland.rs` owns the Wayland backend implementation; this uses the repo's clippy-required non-`mod.rs` layout.
- [x] Existing crate imports still work without compatibility aliases.
- [x] Coding-standards review is clean.
- [x] Final validation commands all pass.

## STOP conditions

Stop if:

- Rust module resolution forces broad import churn outside the scoped files.
- The move requires behavior changes to make tests pass.
- The Wayland implementation needs to expose private helper types through `src/insertion/mod.rs`.
- The existing tests become too coupled to move without redesigning them.

On stopping, write a handback describing the module-resolution problem or test-coupling fork.

## Maintenance notes

This is deliberately a mechanical boundary change. It creates a real `backends/` namespace but leaves the Wayland backend internally large. Plans 003 and 004 reduce that backend after this safer move lands.
