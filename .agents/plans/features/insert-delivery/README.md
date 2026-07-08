# Insert delivery

## Status

- Current artifact: `001-design-discussion.md`
- Status: in review
- Next gate: accept or revise the delivery/outcome contract, then write the structure outline.

## Product decision

`DeliveryTarget::Insert` means: try semantic text insertion first, and if the focused app or compositor cannot accept it, copy the dictation to the clipboard and report that fallback explicitly. Virtual-keyboard/terminal typing is out of scope for the first implementation.

## Artifacts

| Artifact | Status | Purpose |
|---|---|---|
| `001-design-discussion.md` | in review | Decide the production seam, fallback semantics, debug-harness loop, and non-goals for the first insert-delivery implementation. |
