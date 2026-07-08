# Plan 001: Clarify the insertion contract names and outcomes

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback — do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from bd1fd406 --to @ -- src/insertion.rs src/delivery.rs`
> If these files have changed since this plan was revised, compare the "Current state" excerpts against the live code before proceeding; on a mismatch, treat it as a STOP condition.

## Status

- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Planned at**: revision `8b4d975e`, 2026-07-08
- **Revised at**: revision `bd1fd406`, after removing the insert debug simulator

## Why this matters

The insertion seam exists, but its names still expose the first backend. `InsertOutcome::SentToInputMethod` makes the product contract sound Wayland-specific even though delivery only needs to know whether a semantic insertion request was submitted, definitely not submitted, or became uncertain. This plan renames the top-level contract so future backends implement insertion semantics without inheriting Wayland vocabulary.

## Current state

Historical pre-plan state:

- `src/insertion.rs` defined `TextInsertionBackend` with `fn insert(&mut self, text: &str) -> InsertOutcome`.
- `src/insertion.rs` defined `InsertOutcome`, including `SentToInputMethod { sent_bytes }`.
- `src/insertion.rs` defined `InsertFailure` with Wayland-oriented variants.
- `src/delivery.rs` imported `InsertFailure`, `InsertOutcome`, `TextInsertionBackend`, and `WaylandInputMethodBackend`.
- `src/delivery.rs` mapped `InsertOutcome::SentToInputMethod` to `DeliveryReport::InsertRequestSent`.

Implementation note: coding-standards review rejected a backend-neutral `InsertionFailure` name with Wayland-shaped variants, so the implemented failure taxonomy uses backend-neutral variants instead of preserving the original variant names.

Conventions to match:

- Delivery policy lives in `src/delivery.rs`; insertion backends return structured outcomes and do not perform clipboard/stdout fallback.
- User-facing CLI/config spelling remains `insert`; this plan only renames Rust domain types and outcome variants.
- `deliver()` remains infallible from the caller perspective: delivery problems are reported in `DeliveryReport`, not propagated.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Focused insertion tests | `cargo test --lib insertion::tests` | all pass |
| Delivery tests | `cargo test --lib delivery::tests` | all pass |
| Full check | `just check` | exit 0 |
| Full tests | `just test` | all pass |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| Format | `cargo +nightly fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/insertion.rs`
- `src/delivery.rs`

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

Update imports and fake implementations in `src/delivery.rs`.

Do not rename `WaylandInputMethodBackend`; that is intentionally backend-specific.

**Verify**: `cargo test --lib delivery::tests` → all pass.

### Step 2: Rename backend-specific outcome wording

Rename `InsertionOutcome::SentToInputMethod { sent_bytes }` to `InsertionOutcome::Submitted { sent_bytes }`.

Document the important invariant near the enum: `Submitted` means the backend submitted a semantic insertion request; it does not prove the focused application inserted text.

Update `src/delivery.rs` so `Submitted` still maps to `DeliveryReport::InsertRequestSent`.

**Verify**: `cargo test --lib insertion::tests` and `cargo test --lib delivery::tests` → all pass.

### Step 3: Keep failure behavior stable while removing backend vocabulary

Keep fallback behavior stable, but do not keep Wayland-shaped variant names in the backend-neutral `InsertionFailure` contract. The implemented taxonomy maps the existing Wayland causes into backend-neutral failure facts such as environment unavailable, backend unavailable, target unavailable, authority unavailable/deactivated, timeout, or backend failure.

**Verify**: `cargo test --lib delivery::tests` and `cargo test --lib insertion::tests` → all pass.

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
- `cargo clippy --all-targets --all-features -- -D warnings` → exit 0
- `cargo +nightly fmt --check` → exit 0

## Test plan

No new tests are required unless the rename exposes a missing assertion. Existing insertion and delivery tests should cover the behavior. If a test needs to change, only update expected names/variants, not behavior.

## Done criteria

- [x] `TextInsertionBackend`, `InsertOutcome`, and `InsertFailure` no longer appear in `src/`.
- [x] `InsertionOutcome::Submitted` replaces the old outcome variant.
- [x] `DeliveryReport::InsertRequestSent` behavior remains unchanged.
- [x] Coding-standards review is clean.
- [x] Final validation commands all pass after review feedback.
- [x] No production files outside the scope list are modified.

## STOP conditions

Stop if:

- Renaming requires changing user-facing CLI/config spelling from `insert`.
- A second backend abstraction seems necessary to complete the rename.
- Failure taxonomy changes become necessary to make tests pass, rather than being an explicit review-driven boundary improvement.
- The drift check shows substantive edits in the scoped files.

On stopping, write a handback with the current diff, the desired outcome, and the naming fork encountered.

## Maintenance notes

This plan originally allowed backend-shaped `InsertionFailure` variants as an intermediate state. Review feedback found that too misleading after the type became backend-neutral, so the implemented taxonomy now uses backend-neutral failure variants.
