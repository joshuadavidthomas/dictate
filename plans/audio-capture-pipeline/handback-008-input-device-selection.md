# Plan 008 handback

## Why implementation stopped

The plan says `crates/dictate/src/daemon.rs` contains the only `capture(...)` call. Live code has a second call in `crates/dictate-dev/src/screens/overlay.rs`.

Plan 008 requires a clean break that adds `requested_device: Option<&str>` to `capture`. That change cannot compile until every call site is updated. The debug overlay call must pass `None`, since the debug harness has no settings object.

The same plan limits changes to five named files and says completion requires that no other file change. `crates/dictate-dev/src/screens/overlay.rs` is outside that list. Updating it would violate the done criteria. Leaving it unchanged would fail `just check`.

This is a plan mismatch, so implementation stopped rather than adding a compatibility overload or quietly widening scope. All attempted production edits were restored.

## Required decision

Amend Plan 008 to include `crates/dictate-dev/src/screens/overlay.rs` in scope. Its call should become:

```rust
capture(DICTATION_SAMPLE_RATE.as_hz(), None, handler)
```

Then run the plan unchanged. No broader design change is needed.
