# Insert delivery structure outline

Status: in review

Source design: `001-design-discussion.md` (accepted)

## Implementation shape

Build insert delivery in five vertical slices:

1. Delivery report and injectable effect policy.
2. Insert target wiring through CLI/settings/docs.
3. Side-effect-free debug simulator screen.
4. Wayland input-method adapter.
5. Production integration and manual compositor checks.

Each slice should leave the repo green and preserve the current daemon failure contract: delivery problems are reported as delivery outcomes, not daemon-fatal errors.

## Slice 1 — Delivery report and policy seam

### Goal

Make delivery outcomes explicit before adding Wayland. This lets tests and debug scenarios prove the fallback contract without a compositor.

### Likely files

- `src/delivery.rs`
- tests in `src/delivery.rs`

### Shape

Add a report type and internal effect seams:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryReport {
    WroteStdout,
    CopiedToClipboard,
    Inserted,
    CopiedToClipboardAfterInsertFallback { reason: InsertFallbackReason },
    WroteStdoutAfterClipboardFailure {
        attempted: DeliveryTarget,
        clipboard_failure: ClipboardFailure,
    },
    WroteStdoutAfterInsertAndClipboardFallbackFailed {
        insert_reason: InsertFallbackReason,
        clipboard_failure: ClipboardFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InsertFallbackReason {
    NoWaylandDisplay,
    InputMethodUnavailable,
    NoFocusedTextInput,
    ActivationTimedOut,
    ProtocolFailed,
}

pub trait TextInsertionBackend {
    fn insert(&mut self, text: &str) -> InsertionOutcome;
}

pub enum InsertionOutcome {
    Inserted,
    Unavailable(InsertFallbackReason),
    Failed(InsertFallbackReason),
}
```

The final names can change, but do not collapse the report facts.

Keep public `deliver(target, text)` simple for production. Add a private or crate-local `deliver_with_effects(...) -> DeliveryReport` policy function for tests/debug.

### Important details

- Existing clipboard/stdout behavior must remain unchanged for current targets except that a `DeliveryReport` is available internally.
- `deliver` may still print the same stderr messages and ignore the report where not needed.
- Do not return `Result` from the delivery path. Delivery expected failures are handled outcomes.
- Do not create an object hierarchy. Use the smallest Rust shape that allows fake insertion, fake clipboard, and fake stdout in tests.

### Verification

- Unit tests for stdout success and stdout writer failure stay.
- New unit tests:
  - clipboard success reports `CopiedToClipboard`;
  - clipboard failure reports stdout fallback;
  - insertion success reports `Inserted`;
  - insertion unavailable reports clipboard fallback with the insert reason;
  - insertion unavailable + clipboard failure reports stdout fallback with both the insert reason and clipboard failure;
  - insertion protocol failure follows the same fallback policy.

## Slice 2 — Wire `DeliveryTarget::Insert` through config and CLI

### Goal

Expose the new target everywhere `Stdout` and `Clipboard` already exist, without changing production behavior yet beyond falling back honestly if no backend is available.

### Likely files

- `src/delivery.rs`
- `src/settings.rs`
- `src/cli.rs` if help text needs adjustment
- `README.md`
- `plans/product-direction/README.md` if it has stale delivery wording

### Shape

Add `Insert` to:

- `DeliveryTarget` with clap `ValueEnum` value `insert`;
- `SettingsDeliveryTarget` with serde kebab-case parse;
- valid settings examples or docs.

Before the Wayland adapter lands, production `Insert` can route through a temporary no-backend insertion implementation that reports `InputMethodUnavailable` and falls back to clipboard. Do not add a backwards-compatible alias or second config shape.

### Verification

- `DeliveryTarget` clap round-trip includes `Insert`.
- Settings parse `delivery = "insert"`.
- Missing delivery still defaults to stdout.
- Invalid delivery still fails parse.
- `just check` passes.

## Slice 3 — Add an insert debug simulator screen

### Goal

Use the shipped debug harness to make insertion outcomes visible and headless before touching real Wayland.

### Likely files

- `src/debug/registry.rs`
- `src/debug/screens/insert.rs` (new)
- `src/debug/chrome.rs` if outcome blocks need a small reusable helper
- `src/debug/mod.rs` only if the screen needs generic shell support

### Scenarios

- `inserted`
- `fallback-no-text-input`
- `fallback-no-wayland`
- `fallback-clipboard-failed`
- `backend-failed`

### Shape

Implement `DebugComponent` using fake insertion/clipboard/stdout effects and the real delivery policy function from Slice 1. Render:

- requested target (`insert`);
- final outcome (`inserted`, `copied`, `stdout fallback`);
- insert reason when present;
- clipboard failure when present;
- human-facing message text.

This screen should not produce live-loop stats unless there is a real changing measurement. It should still work with `--duration`/`--frames`/`--exit` because the debug shell owns exit bounds.

### Verification

- `dictate debug --list` includes screen `insert` and all scenarios.
- Registry validation passes.
- For every scenario:
  - `cargo run -- debug --screen insert --scenario <scenario> --duration 1s --exit` exits 0.
- Unit tests cover scenario IDs and scenario-to-report mapping through the same fake effects used by the screen.

## Slice 4 — Implement the Wayland input-method adapter

### Goal

Translate `zwp_input_method_v2` behavior into `InsertionOutcome` without leaking protocol mechanics into delivery policy.

### Likely files

- `src/insertion.rs` or `src/insertion/wayland.rs` (new; choose the smallest cohesive module split)
- `src/lib.rs` or `src/main.rs` module declarations as needed
- `Cargo.toml` only if current Wayland deps are not already sufficient for production code
- possibly move/reuse code from `examples/insert_input_method.rs`

### Shape

Implement a production backend roughly equivalent to:

```rust
pub struct WaylandInputMethodBackend {
    timeout: Duration,
}

impl TextInsertionBackend for WaylandInputMethodBackend {
    fn insert(&mut self, text: &str) -> InsertionOutcome {
        // connect, bind, wait for activation, commit UTF-8, release
    }
}
```

Adapter responsibilities:

- connect to the current Wayland display;
- bind `zwp_input_method_manager_v2`;
- create an input-method object for the relevant seat;
- wait briefly for activation/done;
- commit UTF-8 text;
- release all Wayland objects by dropping after the attempt;
- map failures into `InsertFallbackReason` tags.

Mapping guidance:

| Condition | Outcome |
|---|---|
| `WAYLAND_DISPLAY`/connection missing | `Unavailable(NoWaylandDisplay)` |
| no `zwp_input_method_manager_v2` global | `Unavailable(InputMethodUnavailable)` |
| no focused text input activates before timeout | `Unavailable(ActivationTimedOut)` or `Unavailable(NoFocusedTextInput)` |
| protocol dispatch/commit error | `Failed(ProtocolFailed)` |
| committed string accepted | `Inserted` |

Do not implement virtual keyboard, xkb, uinput, libei, or terminal mode in this slice.

### Verification

Automated:

- adapter-free unit tests for failure mapping helpers if extraction is useful;
- `cargo check --all-targets` catches examples and production Wayland code;
- existing debug simulator tests stay green.

Manual:

- On niri with a Chromium/GTK text field focused:
  - run a command or small temporary harness that uses production `DeliveryTarget::Insert` with Unicode text;
  - verify text appears and report says inserted.
- With a terminal or non-text-input app focused:
  - verify fallback message and clipboard contents.

STOP if the adapter needs daemon-lifetime input-method ownership to work. That reopens the fcitx5/IBus coexistence question and must return to design.

## Slice 5 — Production delivery integration and docs

### Goal

Make normal dictation use insert delivery when configured or passed on the CLI, with honest reporting from the daemon.

### Likely files

- `src/delivery.rs`
- `src/daemon.rs`
- `src/settings.rs`
- `README.md`
- `PLAN.md` only if it still claims insertion is future-only

### Shape

- `deliver(DeliveryTarget::Insert, text)` constructs the production Wayland backend plus real clipboard/stdout effects.
- Daemon can keep calling `delivery::deliver(...)`, or it can receive the `DeliveryReport` if a cleaner message/reporting split emerges during implementation.
- Stderr should say what happened:
  - inserted;
  - copied after insert unavailable;
  - stdout after fallback failure.

Keep message text specific but short. Do not dump raw Wayland errors.

### Verification

Automated:

- `just check`
- `just test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo +nightly fmt --check`
- `cargo run -- debug --screen insert --scenario inserted --duration 1s --exit`
- `cargo run -- debug --screen insert --scenario fallback-no-text-input --duration 1s --exit`
- `cargo run -- debug --screen insert --scenario fallback-no-wayland --duration 1s --exit`
- `cargo run -- debug --screen insert --scenario fallback-clipboard-failed --duration 1s --exit`
- `cargo run -- debug --screen insert --scenario backend-failed --duration 1s --exit`

Manual:

- `dictate daemon --delivery insert`, then dictate into Chromium/GTK on niri.
- `dictate daemon --delivery insert`, then dictate with a terminal focused; verify clipboard fallback.
- If available, repeat with fcitx5/IBus running and confirm normal input recovers after Dictate exits.

## Out of scope for this outline

- Socket ack protocol for clients.
- Daemon audio injection.
- Virtual-keyboard/terminal typing.
- GNOME/KDE/macOS/Windows insertion backends.
- User-configurable backend policy.
- Live insertion from the debug harness by default.

## Drift checks

Before writing the final plan or executing:

- Confirm `main` still contains debug harness commit `90977da7` or equivalent registry/screen support.
- Confirm `src/delivery.rs` still owns fallback behavior and has not been replaced by a broader delivery service.
- Confirm `examples/insert_input_method.rs` still builds or has been superseded by production adapter code.
- Confirm no new product decision asks for terminal typing in the first insert target.

## STOP conditions

Stop and return to design if:

- Wayland input-method insertion only works by holding the input-method seat for daemon lifetime.
- Clipboard fallback cannot preserve the inserted/unavailable distinction.
- Implementing virtual keyboard becomes necessary for the first target.
- The debug screen would need to type into the user's focused app to prove the policy.
- `DeliveryTarget::Insert` starts requiring compositor-specific user configuration in the first slice.

## Review gate

Review the slice order, the report/effect seam, and the STOP conditions before writing the final executor plan.
