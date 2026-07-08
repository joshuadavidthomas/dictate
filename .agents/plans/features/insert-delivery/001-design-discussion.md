# Insert delivery design discussion

Status: in review

## What better means

Dictate should put final text where the user is already working. When that cannot be done safely, Dictate should take the reliable fallback path and tell the truth: the text was copied to the clipboard, not inserted.

This design optimizes for a visible product improvement without hiding compositor uncertainty. It should also extend the debug harness so the insertion contract can be exercised by agents without typing into a real app.

## Current state

- `DeliveryTarget` is the runtime delivery selector with `Stdout` and `Clipboard` only (`src/delivery.rs:14`).
- `deliver(target, text)` is intentionally infallible from the daemon's perspective and owns stdout/clipboard fallback reporting (`src/delivery.rs:20`).
- Settings map TOML `delivery` into runtime `DeliveryTarget` (`src/settings.rs:91`, `src/settings.rs:206`).
- The daemon delivers non-empty formatted text from the microphone worker and does not currently receive a delivery outcome (`src/daemon.rs:215`).
- The debug harness has a screen registry and scenario contract suitable for adding an insertion simulator (`src/debug/registry.rs:30`, `src/debug/registry.rs:71`).
- The spike verdict chose `zwp_input_method_v2` first on niri and clipboard fallback when no active text input accepts insertion (`plans/product-direction/spike-insertion-findings.md:7`, `plans/product-direction/spike-insertion-findings.md:13`).
- The spike rejected virtual keyboard as the default path because it is key emission, lacks target acknowledgement, and mangled emoji (`plans/product-direction/spike-insertion-findings.md:11`).
- The spike already sketched the right conceptual seam: `TextInsertionBackend` returning `InsertionOutcome` (`plans/product-direction/spike-insertion-findings.md:152`, `plans/product-direction/spike-insertion-findings.md:157`).

## Resolved design questions

### What does `insert` mean?

`DeliveryTarget::Insert` means semantic insertion when available, with explicit clipboard fallback otherwise.

This resolves the largest prior open question from the spike. The target name is allowed to be aspirational because the outcome is not: callers and logs must distinguish `Inserted` from `CopiedToClipboard`.

### Is terminal insertion part of this feature?

No. Terminals and apps that do not activate Wayland text input take the clipboard fallback in the first implementation.

A future `insert-keyboard` or `type` target can explore virtual keyboard/uinput. It should not be smuggled into `Insert`, because key emission has different failure modes: keyboard layout, shortcuts, Unicode fidelity, and no semantic acknowledgement.

### Should the daemon hold an input-method object for its lifetime?

No. The first implementation should acquire around one delivery attempt and then release.

The spike observed that acquire-around-delivery worked on niri, and it avoids occupying the single input-method seat for the daemon lifetime. That keeps fcitx5/IBus coexistence risk smaller than a resident input-method owner.

## Proposed architecture

### 1. Delivery remains the use-case boundary

`src/delivery.rs` should continue to own the product-level delivery contract:

```rust
pub enum DeliveryTarget {
    Stdout,
    Clipboard,
    Insert,
}

pub enum DeliveryReport {
    WroteStdout,
    CopiedToClipboard,
    Inserted,
    CopiedToClipboardAfterInsertFallback { reason: InsertFallbackReason },
    WroteStdoutAfterClipboardFailure { attempted: DeliveryTarget, clipboard_failure: ClipboardFailure },
    WroteStdoutAfterInsertAndClipboardFallbackFailed {
        insert_reason: InsertFallbackReason,
        clipboard_failure: ClipboardFailure,
    },
}
```

The exact enum names can change in implementation, but the shape should preserve these facts:

- Was text inserted, copied, or written to stdout?
- If `Insert` fell back, why was semantic insertion unavailable?
- If clipboard fallback failed, what backup path was used?
- If insertion failed and clipboard fallback also failed, what insertion reason was preserved?

`deliver` should still not return `Result` to the daemon. Expected operational failures are outcomes, not daemon-fatal errors. This follows the recently hardened daemon contract.

### 2. Wayland protocol details stay in an adapter

Create an insertion module that translates protocol behavior into local outcomes:

```rust
pub trait TextInsertionBackend {
    fn insert(&self, text: &str) -> InsertionOutcome;
}

pub enum InsertionOutcome {
    Inserted,
    Unavailable(InsertUnavailable),
    Failed(InsertFailure),
}
```

The production Wayland adapter should own:

- connecting to Wayland;
- binding `zwp_input_method_manager_v2`;
- creating the input-method object around the current delivery;
- waiting briefly for activation/done;
- committing UTF-8 text;
- mapping timeout/no global/no display/protocol errors into local outcome tags.

Callers should not learn registry names, event queue sequencing, protocol object lifetime, or Wayland-specific errors.

### 3. Clipboard fallback is product policy, not adapter policy

The Wayland adapter should report `Unavailable` or `Failed`. `delivery.rs` decides whether to copy to clipboard and how to report it.

This keeps the adapter honest and lets a debug screen simulate insertion outcomes without invoking real clipboard or Wayland effects unless explicitly requested later.

### 4. The delivery policy needs injectable effects

Do not make `deliver(target, text)` instantiate Wayland, clipboard, and stdout effects directly in the same function that owns fallback policy. That would make policy tests and debug simulations depend on the user's compositor and clipboard.

Use one narrow production seam, for example:

```rust
struct DeliveryEffects<I, C, O> {
    insertion: I,
    clipboard: C,
    stdout: O,
}

fn deliver_with_effects(
    target: DeliveryTarget,
    text: &str,
    effects: &mut DeliveryEffects<impl TextInsertionBackend, impl ClipboardSink, impl TextSink>,
) -> DeliveryReport;
```

The exact Rust shape can be simpler than this sketch. The requirements are:

- production `deliver(target, text)` constructs the acquire-around-delivery Wayland insertion backend plus real clipboard/stdout sinks;
- tests pass fake insertion/clipboard/stdout sinks through the same policy function;
- the insertion debug screen uses fake sinks and never touches real Wayland, clipboard, or stdout by default;
- the final `DeliveryReport` preserves both the original insertion reason and any later clipboard failure.

### 5. Debug harness gets an insertion simulator screen

Add a `dictate debug` screen for delivery outcomes before wiring live insertion into normal dictation.

Suggested scenarios:

- `inserted` — semantic insertion succeeded.
- `fallback-no-text-input` — focused app never activated text input; clipboard fallback used.
- `fallback-no-wayland` — no Wayland input-method backend; clipboard fallback used.
- `fallback-clipboard-failed` — insertion unavailable and clipboard failed, so stdout fallback used.
- `backend-failed` — protocol/runtime failure classified and reported, then clipboard fallback used.

The screen should render stable stat blocks or outcome blocks, plus the exact human-facing consequence. It should be available through the same headless loop as existing screens: `--screen`, `--scenario`, `--duration`/`--frames`, and `--exit`.

Important: debug simulation must not type into the user's focused app. Live compositor insertion, if added later, should be an explicit manual command or scenario with clear danger copy.

### 6. Production integration stays narrow

Add `insert` to:

- CLI delivery values;
- settings TOML parsing;
- docs/config examples;
- daemon reporting through the returned `DeliveryReport`.

Do not add settings for timeouts, backend choice, terminal mode, or compositor-specific knobs in the first implementation. The first contract is a clean end-state for one target, not a compatibility matrix.

## Error and reporting contract

Expected insertion outcomes should be structured internally and message-ready at the edge.

Recommended local reason tags:

- `NoWaylandDisplay`
- `InputMethodUnavailable`
- `NoFocusedTextInput`
- `ActivationTimedOut`
- `ProtocolFailed`

User-facing messages should preserve the consequence:

- `dictation inserted into focused app (... chars)`
- `focused app did not accept Wayland text insertion; copied dictation to clipboard`
- `Wayland text insertion is unavailable; copied dictation to clipboard`
- `clipboard fallback failed after insertion was unavailable; wrote dictation to stdout`

Do not expose raw Wayland errors in normal CLI copy. Keep detailed causes in debug logs/stderr only when safe.

## Verification strategy

Automated checks should prove the local contract without requiring a compositor:

- delivery enum CLI round-trip includes `insert`;
- settings parse `delivery = "insert"`;
- simulated insertion outcomes map to the right `DeliveryReport`;
- insert fallback calls clipboard policy and produces the expected report;
- clipboard failure after insert fallback writes stdout and reports both the original insert reason and the clipboard failure;
- debug registry validates the new insertion scenarios;
- `dictate debug --screen insert --scenario <id> --duration 1s --exit` exits successfully for every scenario.

Manual checks should be limited and explicit:

- On niri, run the production input-method adapter against a known Chromium/GTK field and verify text appears.
- On a focused terminal or non-text-input app, verify the fallback message and clipboard contents.
- If fcitx5/IBus is running on a tester's machine, verify acquire-around-delivery does not permanently break normal input. This is a risk check, not a first-slice blocker.

## Non-goals

- Virtual-keyboard, uinput, xdotool/wtype, or terminal typing.
- GNOME/KDE-specific insertion backends.
- macOS or Windows insertion.
- Daemon audio injection or socket ack protocol.
- User-configurable insertion backend policy.
- Long-form transcription/VAD work.

## Standing policy recommendation

Any future delivery target must return an explicit report that says what happened to the text. A target that silently falls back is not acceptable; a target that falls back and reports the consequence is.

## Design review checklist

Before outlining implementation, confirm:

1. `DeliveryReport` is the right level of outcome for daemon/debug callers.
2. Clipboard fallback belongs in `delivery.rs`, not in the Wayland adapter.
3. The debug screen should simulate outcomes first and avoid live typing by default.
4. Virtual-keyboard/terminal mode remains out of scope for `DeliveryTarget::Insert`.
