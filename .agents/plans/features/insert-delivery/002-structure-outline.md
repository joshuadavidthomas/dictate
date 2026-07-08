# Insert delivery structure outline

Status: implemented, reconciled after `bd1fd406 Remove insert debug simulator`

Source design: `001-design-discussion.md` (accepted)

## Implementation shape

Insert delivery was built in vertical slices. The original third slice, a side-effect-free debug simulator screen, was later removed because it did not exercise the real UI or insertion path.

Historical slice shape:

1. Delivery report and injectable effect policy.
2. Insert target wiring through CLI/settings/docs.
3. Side-effect-free debug simulator screen — **superseded/removed**.
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

## Slice 3 — Superseded: insert debug simulator screen

This slice was implemented, then removed in `bd1fd406 Remove insert debug simulator`.

Reason: the simulator was a visualized delivery-policy unit test, not a useful UI or integration test. It did not exercise Wayland, clipboard, focus, daemon flow, or real insertion. The policy cases it visualized now belong in `src/delivery.rs` tests.

Do not restore this slice unless the replacement exercises a real user-visible path.

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
- delivery and insertion tests stay green.

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
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo +nightly fmt --check`

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
- Live or simulated insertion from the debug harness by default.

## Drift checks

Before writing the final plan or executing:

- Confirm `src/delivery.rs` still contains policy tests for insert success, fallback-safe insert failures, clipboard fallback, stdout fallback, and uncertain insert failures.
- Confirm `src/delivery.rs` still owns fallback behavior and has not been replaced by a broader delivery service.
- Confirm `examples/insert_input_method.rs` still builds or has been superseded by production adapter code.
- Confirm no new product decision asks for terminal typing in the first insert target.

## STOP conditions

Stop and return to design if:

- Wayland input-method insertion only works by holding the input-method seat for daemon lifetime.
- Clipboard fallback cannot preserve the inserted/unavailable distinction.
- Implementing virtual keyboard becomes necessary for the first target.
- A proposed debug affordance would only simulate policy rather than exercise real UI, daemon, compositor, clipboard, or insertion behavior.
- `DeliveryTarget::Insert` starts requiring compositor-specific user configuration in the first slice.

## Review gate

Review the slice order, the report/effect seam, and the STOP conditions before writing the final executor plan.
