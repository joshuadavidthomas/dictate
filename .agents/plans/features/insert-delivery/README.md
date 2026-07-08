# Insert delivery

## Status

- Current artifact: `002-structure-outline.md`
- Status: outlining
- Next gate: review the vertical slices, report/effect seam, and STOP conditions before writing the final executor plan.

## Product decision

`DeliveryTarget::Insert` means: try semantic text insertion first, and if the focused app or compositor cannot accept it, copy the dictation to the clipboard and report that fallback explicitly. Virtual-keyboard/terminal typing is out of scope for the first implementation.

## Accepted decisions

- Semantic Wayland input-method insertion is the first backend.
- Clipboard is the universal fallback.
- Fallbacks must produce explicit delivery reports; silent fallback is not acceptable.
- Delivery policy owns fallback; the Wayland adapter only reports insertion outcomes.
- The debug harness gets side-effect-free simulated insertion outcomes before live production wiring.

## Artifacts

| Artifact | Status | Purpose |
|---|---|---|
| `001-design-discussion.md` | accepted | Decide the production seam, fallback semantics, debug-harness loop, and non-goals for the first insert-delivery implementation. |
| `002-structure-outline.md` | in review | Slice implementation into reviewable vertical phases with validation and STOP conditions. |
