# Insertion boundaries refactor

This effort refactors insertion around clear domain boundaries after the insert-delivery feature landed at planned revision `8b4d975e`. The goal is to keep `delivery.rs` responsible for target choice and fallback policy, keep `insertion/` responsible for the semantic insertion contract, and place concrete mechanisms under `insertion/backends/`. The first backend remains Wayland `zwp_input_method_v2`; this effort does not add another backend.

Execute in the order below unless dependencies say otherwise. Each executor: read the plan fully before starting, honor its STOP conditions, run the coding-standards review loop, and update your row when done.

## Target boundaries and names

```text
delivery.rs
  chooses stdout/clipboard/insert and owns fallback policy

insertion/
  owns the semantic insertion contract:
  InsertionBackend -> InsertionOutcome / InsertionFailure

insertion/backends/
  owns concrete insertion mechanisms

insertion/backends/wayland/
  owns the zwp_input_method_v2 adapter and its Wayland-only mechanics
```

Preferred names:

- `InsertionBackend`, not `TextInsertionBackend`.
- `InsertionOutcome::Submitted`, not `SentToInputMethod`.
- `WaylandInputMethodBackend` remains backend-specific and belongs under `backends::wayland`.
- Avoid a giant `input_method_session.rs`; split Wayland internals by concepts: IO/discovery, text chunks, commit lifecycle, protocol state.

## Execution order & status

| Plan | Title | Effort | Depends on | Status |
|---|---|---:|---|---|
| [001](001-clarify-insertion-contract.md) | Clarify the insertion contract names and outcomes | S | — | TODO |
| [002](002-create-backends-module-boundary.md) | Move Wayland insertion behind a backends module | M | 001 | TODO |
| [003](003-extract-wayland-io-and-discovery.md) | Extract Wayland discovery and IO mechanics | M | 002 | TODO |
| [004](004-extract-wayland-protocol-state.md) | Extract Wayland protocol state into small named modules | M | 003 | TODO |

Status values: TODO | IN PROGRESS | DONE | BLOCKED (one-line reason) | SUPERSEDED (one-line pointer to what replaced it)

## Dependency notes

- **001 → 002**: establish backend-neutral domain names before moving files, so module extraction does not preserve misleading names.
- **002 → 003**: create the real `backends/wayland` namespace before extracting Wayland-only IO modules.
- **003 → 004**: remove low-level IO mechanics before splitting protocol state, reducing the chance of circular module dependencies.

## Reconciliation log

Newest first.

- **2026-07-08**: Initial plan bundle created after insert-delivery landed. Next executable plan: 001.

## Considered and rejected

- **Add a second backend now**: rejected because it would force speculative abstraction. The seam should be real, but the only concrete adapter remains Wayland for now.
- **Create a generic cross-platform driver trait below `InsertionBackend`**: rejected because it would likely mirror Wayland lifecycle and become fake abstraction.
- **One `input_method_session.rs` file**: rejected because it recreates the large-file problem under a narrower name.
- **Backward-compatible type aliases for old names**: rejected because these are crate-private contracts and the project prefers clean breaks.

## Deferred

- **Normalize `InsertionFailure` into fully backend-neutral variants**: Plan 001 adds a maintenance note but does not redesign failure taxonomy. Do this before adding a non-Wayland backend if Wayland-specific variants become misleading.
- **Add another insertion backend**: not part of this refactor. After these plans, another backend should implement only `InsertionBackend` and return `InsertionOutcome`.
