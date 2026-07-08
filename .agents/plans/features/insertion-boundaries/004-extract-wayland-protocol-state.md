# Plan 004: Extract Wayland protocol state into small named modules

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback — do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from 8b4d975e --to @ -- src/insertion.rs`
> If Plans 002 and 003 have already run, translate this drift check to the live Wayland backend paths under `src/insertion/backends/wayland/` and compare the current-state symbols below against live code.

## Status

- **Effort**: M
- **Risk**: MED
- **Depends on**: `003-extract-wayland-io-and-discovery.md`
- **Planned at**: revision `8b4d975e`, 2026-07-08

## Why this matters

The remaining Wayland backend state model should be understandable without reading IO mechanics. Avoid a giant `input_method_session.rs` junk drawer: split the state by concepts that change for different reasons. Commit request lifecycle, text chunking/accounting, and input-method protocol state each deserve their own small module.

## Current state

In the pre-refactor file:

- `src/insertion.rs:800` defines `InputMethodSession`.
- `src/insertion.rs:839` maps `InputMethodFailure` into insertion failure.
- `src/insertion.rs:957` defines `ChunkQueue` and sent/maybe-sent accounting.
- `src/insertion.rs:1007` defines `State`, which mixes discovery fields, chunk queue, session, roundtrip, and progress.
- `src/insertion.rs:1184-1269` implements Wayland `Dispatch` for registry, manager, callback, and input-method events.
- `src/insertion.rs:1274` defines `commit_string_chunks`.

After prior plans, these symbols should live under `src/insertion/backends/wayland/`.

Conventions to match:

- The pure state model should not know about `Connection`, `EventQueue`, rustix polling, or clipboard/stdout fallback.
- Wayland protocol object types belong at the Wayland adapter edge, not in the top-level insertion contract.
- Keep one-way dependencies where possible: orchestration imports state/commits/chunks/io; lower-level modules should not import orchestration.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Focused insertion tests | `cargo test --lib insertion::tests` | all pass |
| Full check | `just check` | exit 0 |
| Full tests | `just test` | all pass |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo +nightly fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/insertion/backends/wayland/mod.rs`
- `src/insertion/backends/wayland/commits.rs`
- `src/insertion/backends/wayland/text_chunks.rs`
- `src/insertion/backends/wayland/protocol_state.rs`

**Out of scope**:
- A file named `input_method_session.rs` containing all protocol state.
- Changing Wayland IO helpers extracted in Plan 003 except imports/visibility.
- Changing delivery fallback policy.
- Adding a non-Wayland backend.

## Steps

### Step 1: Extract text chunking and byte accounting

Create `src/insertion/backends/wayland/text_chunks.rs` for:

- `MAX_COMMIT_STRING_BYTES`
- `commit_string_chunks`
- chunk-boundary tests
- sent/maybe-sent accounting if it remains independent of commit request state

Name the module after what it owns: splitting text into protocol-sized UTF-8 chunks and tracking byte accounting. Do not name it generically `utils.rs`.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 2: Extract commit request lifecycle into `commits.rs`

Create `src/insertion/backends/wayland/commits.rs` for commit-specific data:

- `CommitChunk`
- `CommitBatch`
- `CommitRequest`
- `BufferedCommit`
- commit queue/flush token invariants

The important invariant: `BufferedCommit` represents that a non-idempotent commit request entered the Wayland client buffer, and consuming it is the only way to mark bytes as flushed.

Keep protocol-state transitions that are not commit-specific out of this file.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 3: Extract input-method protocol state into `protocol_state.rs`

Create `src/insertion/backends/wayland/protocol_state.rs` for:

- `InputMethodSession`
- `InputMethodEvent`
- `InputMethodFailure`
- state transitions for activate/deactivate/done/unavailable
- `State` if it still primarily acts as the Wayland protocol state carried by `EventQueue<State>`

If `State` still contains registry-discovery fields (`input_method_manager`, `seat`, `roundtrip_done`), keep those fields in `State` for now because Wayland dispatch needs one state object. Do not split into multiple dispatch states unless the interface stays simpler than the current one.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 4: Keep Wayland dispatch mapping readable

Move or leave `Dispatch` impls wherever they are easiest to read, but keep this boundary:

- `Dispatch<wl_registry::WlRegistry, ()>` maps registry events to discovery fields.
- `Dispatch<wl_callback::WlCallback, ()>` marks roundtrip completion.
- `Dispatch<ZwpInputMethodV2, ()>` converts Wayland events into `InputMethodEvent` and calls protocol-state transitions.

Do not let dispatch implementations own commit lifecycle or IO loops.

**Verify**: `just check` → exit 0.

### Step 5: Redistribute tests by responsibility

Move tests next to the module that owns the behavior when doing so improves locality:

- chunk-size and UTF-8 boundary tests → `text_chunks.rs`
- commit token/queue transition tests → `commits.rs` or `protocol_state.rs`, whichever owns the transition
- event-driven insertion outcome tests may remain in the Wayland module integration-style test block

Do not expose private internals just to keep old test placement.

**Verify**: `cargo test --lib insertion::tests` → all pass.

### Step 6: Run review and final validation

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
- `cargo clippy --all-targets -- -D warnings` → exit 0
- `cargo +nightly fmt --check` → exit 0

## Test plan

Preserve all existing insertion tests. Prefer moving tests beside their owned concepts, but do not weaken coverage. The final focused test command must still report all insertion tests passing.

## Done criteria

- [ ] There is no giant `input_method_session.rs` module.
- [ ] Text chunking/accounting, commit request lifecycle, and input-method protocol state live in separately named modules.
- [ ] The Wayland backend entrypoint reads as orchestration rather than state-machine implementation.
- [ ] No top-level insertion contract type imports Wayland protocol types.
- [ ] Coding-standards review is clean.
- [ ] Final validation commands all pass.

## STOP conditions

Stop if:

- Splitting state creates circular dependencies that require broad `pub(crate)` exposure.
- `State` cannot be split cleanly because `EventQueue<State>` requires one concrete dispatch state; keep `State` whole and write a handback instead of forcing a bad split.
- A module becomes a miscellaneous bucket. If a file cannot be named after one concept, stop and propose a better split.
- Tests require exposing internals that were private before.

On stopping, write a handback with the concept that resisted separation and the smallest alternative split you see.

## Maintenance notes

The goal is conceptual locality, not maximum file count. If a proposed module would be shallow or just re-export another module, do not create it. Prefer three meaningful Wayland state modules over six tiny pass-through files.
