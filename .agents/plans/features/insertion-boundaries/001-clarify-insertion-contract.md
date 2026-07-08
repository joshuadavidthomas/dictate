# Plan 001: Clarify the insertion contract names and outcomes

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback — do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from 8b4d975e --to @ -- src/insertion.rs src/delivery.rs src/debug/screens/insert.rs`
> If these files have changed since this plan was written, compare the "Current state" excerpts against the live code before proceeding; on a mismatch, treat it as a STOP condition.

## Status

- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Planned at**: revision `8b4d975e`, 2026-07-08

## Why this matters

The insertion seam exists, but its names still expose the first backend. `InsertOutcome::SentToInputMethod` makes the product contract sound Wayland-specific even though delivery only needs to know whether a semantic insertion request was submitted, definitely not submitted, or became uncertain. This plan renames the top-level contract so future backends implement insertion semantics without inheriting Wayland vocabulary.

## Current state

- `src/insertion.rs:27` defines `TextInsertionBackend` with `fn insert(&mut self, text: &str) -> InsertOutcome`.
- `src/insertion.rs:32` defines `InsertOutcome`, including `SentToInputMethod { sent_bytes }`.
- `src/insertion.rs:44` defines `InsertFailure` with Wayland-oriented variants.
- `src/delivery.rs:13-16` imports `InsertFailure`, `InsertOutcome`, `TextInsertionBackend`, and `WaylandInputMethodBackend`.
- `src/delivery.rs:160` maps `InsertOutcome::SentToInputMethod` to `DeliveryReport::InsertRequestSent`.
- `src/debug/screens/insert.rs:26-28` imports the same insertion contract for side-effect-free insert preview scenarios.

Conventions to match:

- Delivery policy lives in `src/delivery.rs`; insertion backends return structured outcomes and do not perform clipboard/stdout fallback.
- User-facing CLI/config spelling remains `insert`; this plan only renames Rust domain types and outcome variants.
- `deliver()` remains infallible from the caller perspective: delivery problems are reported in `DeliveryReport`, not propagated.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Focused insertion tests | `cargo test --lib insertion::tests` | all pass |
| Delivery tests | `cargo test --lib delivery::tests` | all pass |
| Debug insert tests | `cargo test --lib debug::screens::insert::tests` | all pass |
| Full check | `just check` | exit 0 |
| Full tests | `just test` | all pass |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo +nightly fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/insertion.rs`
- `src/delivery.rs`
- `src/debug/screens/insert.rs`

**Out of scope**:
- Moving files into `src/insertion/` — Plan 002 owns module layout.
- Rewriting Wayland internals — later plans own backend decomposition.
- Adding a second insertion backend.

## Steps

### Step 1: Rename the product-level trait and types

Rename top-level insertion contract names to domain nouns:

- `TextInsertionBackend` → `InsertionBackend`
- `InsertOutcome` → `InsertionOutcome`
- `InsertFailure` → `InsertionFailure`

Update imports and fake implementations in `src/delivery.rs` and `src/debug/screens/insert.rs`.

Do not rename `WaylandInputMethodBackend`; that is intentionally backend-specific.

**Verify**: `cargo test --lib delivery::tests` → all pass.

### Step 2: Rename backend-specific outcome wording

Rename `InsertionOutcome::SentToInputMethod { sent_bytes }` to `InsertionOutcome::Submitted { sent_bytes }`.

Document the important invariant near the enum: `Submitted` means the backend submitted a semantic insertion request; it does not prove the focused application inserted text.

Update `src/delivery.rs` so `Submitted` still maps to `DeliveryReport::InsertRequestSent`.

**Verify**: `cargo test --lib insertion::tests` and `cargo test --lib delivery::tests` → all pass.

### Step 3: Keep failure taxonomy behavior stable

Keep the existing failure variants for now, only renamed under `InsertionFailure`. Do not redesign failure classification in this plan. The goal is to reduce name coupling without mixing in behavior changes.

Add a maintenance note near `InsertionFailure`: new backend-specific failures should not add backend-specific top-level enum variants without revisiting the failure taxonomy.

**Verify**: `cargo test --lib debug::screens::insert::tests` → all pass.

### Step 4: Run review and full validation

Run the standard review loop after tests pass:

```rust
agent({
  name: "coding-standards-review",
  skills: ["coding-standards"],
  task: "Review the current jj diff for code quality, with focus on the coding-standards. Return findings with file paths and concrete fixes."
})
```

Address findings until the review is clean.

**Verify**:
- `just check` → exit 0
- `just test` → all pass
- `cargo clippy --all-targets -- -D warnings` → exit 0
- `cargo +nightly fmt --check` → exit 0

## Test plan

No new tests are required unless the rename exposes a missing assertion. Existing insertion, delivery, and debug insert tests should cover the behavior. If a test needs to change, only update expected names/variants, not behavior.

## Done criteria

- [ ] `TextInsertionBackend`, `InsertOutcome`, and `InsertFailure` no longer appear in `src/`.
- [ ] `InsertionOutcome::Submitted` replaces `SentToInputMethod`.
- [ ] `DeliveryReport::InsertRequestSent` behavior remains unchanged.
- [ ] Coding-standards review is clean.
- [ ] Final validation commands all pass.
- [ ] No files outside the scope list are modified.

## STOP conditions

Stop if:

- Renaming requires changing user-facing CLI/config spelling from `insert`.
- A second backend abstraction seems necessary to complete the rename.
- Failure taxonomy changes become necessary to make tests pass.
- The drift check shows substantive edits in the scoped files.

On stopping, write a handback with the current diff, the desired outcome, and the naming fork encountered.

## Maintenance notes

This plan intentionally leaves `InsertionFailure` variants backend-shaped. That is acceptable as an intermediate state because there is still only one backend. Future backend work should normalize failure taxonomy before adding backend-specific variants.
