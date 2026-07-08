# Plan 003: Extract Wayland discovery and IO mechanics

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback — do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from 8b4d975e --to @ -- src/insertion.rs`
> If Plan 002 has already run, translate this drift check to the live Wayland backend paths under `src/insertion/backends/wayland/` and compare the current-state symbols below against live code.

## Status

- **Effort**: M
- **Risk**: MED
- **Depends on**: `002-create-backends-module-boundary.md`
- **Planned at**: revision `8b4d975e`, 2026-07-08
- **Revised at**: revision `bd1fd406`, after removing the insert debug simulator

## Why this matters

The Wayland backend currently mixes protocol orchestration with low-level event queue mechanics. Polling, dispatch, flushing, read/write races, and deadline classification are operational mechanics, not insertion-domain state. Extracting them makes the backend entrypoint easier to read and makes future IO bugs local to one module.

## Current state

In the pre-refactor file:

- `src/insertion.rs:194` defines `bounded_roundtrip` for registry discovery.
- `src/insertion.rs:216` defines `ProgressDeadline`.
- `src/insertion.rs:273` defines `dispatch_until_event_or_timeout`.
- `src/insertion.rs:344` starts Wayland read classification.
- `src/insertion.rs:405` starts `flush_event_queue` and related flush helpers.
- `src/insertion.rs:721` defines `poll_wayland_fd`.

After Plan 002, these symbols should live in `src/insertion/backends/wayland/mod.rs` before this plan extracts them.

Conventions to match:

- IO helpers return `Result<_, InsertionFailure>` and do not mutate insertion session state except through explicit `&mut State` parameters already required by `EventQueue<State>` dispatch.
- Deadline code must keep the current semantics: idle timeout resets after protocol progress or write progress; attempt timeout bounds the whole attempt.
- `WouldBlock` and `Interrupted` must keep their current distinct behavior.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Focused insertion tests | `cargo test --lib insertion::tests` | all pass |
| Full check | `just check` | exit 0 |
| Full tests | `just test` | all pass |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| Format | `cargo +nightly fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/insertion/backends/wayland/mod.rs`
- `src/insertion/backends/wayland/discovery.rs`
- `src/insertion/backends/wayland/io.rs`

**Out of scope**:
- Moving commit/session/chunk state — Plan 004 owns that.
- Changing `InsertionOutcome` or delivery fallback behavior.
- Adding generic IO traits for non-Wayland backends.

## Steps

### Step 1: Extract registry discovery into `discovery.rs`

Move bounded registry discovery mechanics into `src/insertion/backends/wayland/discovery.rs`.

The module should own the discovery roundtrip helper currently represented by `bounded_roundtrip`. Keep the interface narrow: it should serve the Wayland attempt, not expose generic discovery abstractions.

A good shape is one function that says what the caller wants, for example:

```rust
pub(super) fn run_registry_roundtrip(...) -> Result<(), InsertionFailure>
```

Use concrete Wayland types at the Wayland module edge. Do not create a backend-neutral discovery trait.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 2: Extract event loop, polling, and flush mechanics into `io.rs`

Move these concepts into `src/insertion/backends/wayland/io.rs`:

- `ProgressDeadline`
- `dispatch_until_event_or_timeout`
- `dispatch_ready_events`
- Wayland read classification and flush-wait policy
- event queue flush helpers
- commit flush helpers
- `FlushRetry`
- `WaylandIoOperation`
- `PollDeadline`
- `FdReady`
- `poll_wayland_fd`

Keep names focused on Wayland IO, not insertion policy. If a helper name contains insertion-domain language but only manages Wayland fd/event queue mechanics, rename it during the move.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 3: Keep `mod.rs` as orchestration, not mechanics

After extraction, `src/insertion/backends/wayland/mod.rs` should primarily contain:

- `WaylandInputMethodBackend`
- trait implementation for `InsertionBackend`
- high-level `insert_with_input_method` attempt orchestration
- Wayland `Dispatch` implementations until Plan 004 moves protocol state
- module declarations and imports

It should not contain raw poll/read/flush loops.

**Verify**: `just check` → exit 0.

### Step 4: Preserve the readiness race tests

Keep or move the tests that prove:

- readable + writable with read `WouldBlock` preserves writable progress
- `Interrupted` retries the read path instead of treating it as writable progress
- flush errors after a buffered/non-idempotent request remain delivery-uncertain

If tests move into `io.rs`, keep them private to that module unless they need full backend state.

**Verify**: `cargo test --lib insertion::tests` → all pass.

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

No behavior change is intended. Preserve existing tests that exercise IO behavior. If extracting private helpers makes direct unit tests awkward, prefer testing a small policy function in `io.rs` over exposing internals through public module interfaces.

## Done criteria

- [x] Wayland polling, read classification, event dispatch, and flushing live in `io.rs`.
- [x] Registry roundtrip/discovery helper lives in `discovery.rs`.
- [x] The Wayland backend entrypoint no longer contains low-level IO loops.
- [x] Readiness-race tests still exist and pass.
- [x] Coding-standards review is clean.
- [x] Final validation commands all pass.

## STOP conditions

Stop if:

- Extracting IO requires making `State` or protocol internals broadly public.
- A helper starts to look backend-neutral even though it depends on Wayland types; keep it Wayland-local and hand back the naming fork.
- Tests require behavior changes to pass.
- The extraction creates circular module dependencies between IO and session/commit state.

On stopping, write a handback with the dependency cycle or visibility problem.

## Maintenance notes

This plan should improve locality for Wayland IO bugs without creating a fake cross-backend IO abstraction. A future non-Wayland backend should not import this module.
