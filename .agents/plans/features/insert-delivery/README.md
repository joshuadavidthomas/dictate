# Insert delivery

## Status

- Status: implemented
- Last reconciliation: 2026-07-08, after `bd1fd406 Remove insert debug simulator`
- This bundle is now historical. Use `.agents/plans/features/insertion-boundaries/` for the next executable insertion work.

## Product decision

`DeliveryTarget::Insert` means: try semantic text insertion first, and if the focused app or compositor cannot accept it, copy the dictation to the clipboard and report that fallback explicitly. Virtual-keyboard/terminal typing is out of scope for the first implementation.

## Accepted decisions

- Semantic Wayland input-method insertion is the first backend.
- Clipboard is the universal fallback.
- Fallbacks must produce explicit delivery reports; silent fallback is not acceptable.
- Delivery policy owns fallback; the Wayland adapter only reports insertion outcomes.
- Side-effect-free insertion policy coverage belongs in delivery tests, not the interactive debug harness.

## Reconciliation

- **2026-07-08**: Insert delivery landed. The insert debug simulator was later removed in `bd1fd406` because it did not exercise UI, Wayland, clipboard, focus, daemon flow, or real insertion. The policy cases it visualized are covered in `src/delivery.rs` tests.

## Artifacts

| Artifact | Status | Purpose |
|---|---|---|
| `001-design-discussion.md` | accepted, reconciled | Decided the production seam, fallback semantics, and non-goals for the first insert-delivery implementation. Its insert-debug-screen proposal was superseded after implementation. |
| `002-structure-outline.md` | implemented, reconciled | Historical slice outline. Slice 3's insert debug simulator was removed; delivery policy coverage now lives in tests. |
