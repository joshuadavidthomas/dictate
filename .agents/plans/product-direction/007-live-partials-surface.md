# Plan 007: Live partials text surface above the overlay pill

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 0165f013 -- crates/dictate-ui/ crates/dictate/src/daemon.rs crates/dictate/src/main.rs crates/dictate/build.rs`
> This plan was rewritten against main `0165f013` (`oxuxllmt`, 2026-08-13).
> The in-scope UI and daemon files were unchanged from the earlier
> `70c4fae943fa` draft. Read the live code; on a mismatch beyond
> incidental edits, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: MED (second layer-shell surface is new ground; niri
  surface-behavior is the main unknown — PLAN.md flags layer-shell
  resize as an open question)
- **Depends on**: streaming partials (landed 2026-08-07, `9de89389`);
  overlay phase states (audited DONE 2026-08-13)
- **Planned at**: git `0165f013` / change `oxuxllmt`, 2026-08-13

## Why this matters

The daemon already decodes live hypotheses through `partials_model`, but
they only reach `eprintln!` — invisible to the user. Live text while
speaking is the signature "it's alive" moment of premium dictation apps
and was the promised follow-up when the plan 006 spike was superseded.
All the hard parts (dual recognizers, batch plumbing, realtime decode)
are done; this plan is the visible payoff.

**Maintainer design decision (2026-08-13)**: the partials text is a
surface **separate from** the spectrum overlay, positioned **above** the
pill. It is not a widening or reshaping of the existing pill — the pill's
geometry and visuals stay untouched.

## Current state

The facts below are from `0165f013`. Confirm them before editing.

- `crates/dictate/src/daemon.rs:701-712` — `feed_streaming_batches`
  drains mic batches into `StreamingSession::feed` on the worker's 20ms
  poll and `eprintln!("partial: …")`s changed hypotheses. This is the
  single point where partial text is born. The worker already holds an
  `Overlay` clone (`daemon.rs:493-512`, `604-620`).
- `crates/dictate-speech/src/transcription.rs:99-119` —
  `StreamingSession::feed` returns `Some(&str)` only when the trimmed
  hypothesis is non-empty and different from the last emitted string.
  Clone that `&str` before crossing the daemon/UI channel.
- `crates/dictate-ui/src/app.rs` — one bottom-anchored layer-shell
  window (80×48, `Anchor::BOTTOM`, 40px bottom margin, `Layer::Overlay`,
  no keyboard interactivity). `Overlay` sends revision-guarded
  `OverlayMessage::{Show, Hide}` through one ordered unbounded channel;
  the async loop in `run()` owns the `WindowHandle` and applies messages
  only when the revision is current. Spectrum bypasses the channel on a
  lock-free `SpectrumLevels` side path.
- `crates/dictate-ui/src/app.rs:68-88` — `show` and `hide` both
  `fetch_add` the shared `AtomicU64` revision, then stamp that new
  value on the message. `send_spectrum` does not touch the revision.
- `crates/dictate-ui/src/app.rs:90-100` — `OverlayMessage` is
  `Show { state, revision, hide_after }` or `Hide { revision }`. There
  is no partial variant.
- `crates/dictate-ui/src/app.rs:116-185` — `run()` owns one
  `Option<WindowHandle<OverlayView>>`. Current Show opens or updates
  that window; current Hide calls `remove_window()`. Stale Show/Hide
  fall through a no-op arm.
- `crates/dictate-ui/src/overlay.rs` — `OverlayView` renders
  per-state visuals inside `components::Panel`; animation runs on the
  wake/timer loop. Do not touch its pacing; see the in-code comment
  about GPUI frame callbacks (`overlay.rs:72-75`).
- `crates/dictate-ui/src/components/panel.rs` — the pill chrome
  (`Panel::new(id, width, label)`). It hardcodes `h(36.0)` and
  `rounded_full()`, fill `rgba(0x1e1e_1ef0)`, and a soft black shadow.
  A two-line text card cannot reuse this geometry as-is.
- `crates/dictate-ui/src/lib.rs` — public surface is `Overlay`,
  `UiIdentity`, `run`, `OverlayState`, `OverlayView`, and the pill
  window size constants. `OverlayMessage` and `Panel` stay private.
- `crates/dictate/src/main.rs:8-11` plus `crates/dictate/build.rs:18,28`
  — one Wayland namespace (`dictate-overlay` / `dictate-dev-overlay`).
  Derive the second surface's namespace in `dictate-ui` from
  `UiIdentity`; do not add a third build-script env var.
- Partial cadence: a handful of updates per second at most (hypothesis
  changes, not per-batch) — orders of magnitude below spectrum rate.
- `dictate-ui` has no protocol tests. The only UI tests are signal
  math in `overlay.rs:325-378`.

### Why revision-alone is not enough

A Partial that only checks "is my revision still current?" will reopen
the text card after stop or cancel.

`recording_id()` stays `Some` through `PendingStop`
(`crates/dictate-speech/src/dictation.rs:366-369`). The command thread
calls `overlay.show(OverlayState::Transcribing)` *before*
`begin_stopping()` (`daemon.rs:448-455`). The worker can still take
`MicSessionAction::Keep` and run `feed_streaming_batches` after that
Show has already bumped the revision (`daemon.rs:601-652`,
`1101-1109`). A Partial stamped with the Transcribing revision then
passes a revision-only guard.

Cancel, stream-error abort, and `FinishStopping::Empty` are worse:
they Hide and return the control state to Idle without clearing
`streaming.session` / `streaming.batches` (`daemon.rs:457-460`,
`564-569`, `655-658`, `665-680`). Later `(None, None)` polls are also
`Keep`, so a queued batch can emit a Partial stamped with the Hide
revision.

The apply rule below is the safety net. Do not expand daemon scope to
"fix" those leftover session fields.

## Commands you will need

| Purpose   | Command                                                      | Expected on success |
|-----------|--------------------------------------------------------------|---------------------|
| Check     | `just check`                                                 | exit 0              |
| Tests     | `just test`                                                  | all pass            |
| Lint      | `cargo clippy --locked --all-targets --all-features -- -D warnings` | exit 0        |
| Run live  | `just run daemon` (Wayland + mic)                            | partials visible    |

Do not use `just clippy` for verification: that recipe passes `--fix
--allow-dirty`. The cargo invocation above is check-only.

## Scope

**In scope**:
- `crates/dictate-ui/src/app.rs` (protocol + second surface lifecycle)
- `crates/dictate-ui/src/overlay.rs` only if a tiny shared fade helper
  is extracted; do not change pill rendering or animation pacing
- New view module under `crates/dictate-ui/src/` for the text surface
- `crates/dictate-ui/src/components/` (new text card; optional shared
  chrome tokens if that keeps the pill visually identical)
- `crates/dictate-ui/src/lib.rs` / `components.rs` module wiring
- `crates/dictate/src/daemon.rs` (only: send partials from
  `feed_streaming_batches`; keep the stderr line)

**Out of scope** (do NOT touch):
- The pill's geometry, visuals, or animation pacing.
- `crates/dictate-speech/` — `StreamingSession` already yields exactly
  what the UI needs.
- Formatting partials through `DictationFormatter` — display raw
  hypothesis text; formatted partials are a deferred layer (see
  maintenance notes).
- Overlay position/size settings.
- `crates/dictate/build.rs` / `crates/dictate/src/main.rs` — one
  `UiIdentity` is enough; derive the second namespace in `dictate-ui`.
- Clearing `StreamingState` on cancel/empty — UI apply rules cover
  those races; that cleanup is a later daemon hygiene change.
- Plan 008 (audio ducking) — it also edits `daemon.rs`. Stay on the
  overlay/partials path so the two can land in either order.

## Protocol (this must be done this way)

Partials ride the **existing** ordered `OverlayMessage` channel. Do not
add a second channel. Do not use the spectrum side path.

`send_partial` **must not** `fetch_add` the revision. It loads the
revision current at send time and stamps that value on
`OverlayMessage::Partial { text, revision }`. Show and Hide remain the
only messages that advance the counter.

The apply path needs more than a revision check. Keep a small session
record in the `run()` loop (or a private helper it calls):

```text
pill: Option<OverlayState>
partial: Option<String>   # None means the text surface is closed
```

Apply, after the existing "message revision == current revision" guard:

```text
Show(Recording):
    open or update the pill
    clear partial (close the text window if open)

Show(any other state):
    open or update the pill
    clear partial (close the text window if open)

Hide:
    close the pill
    clear partial

Partial(text):
    if pill != Some(Recording): ignore
    if text is empty: ignore
    set partial and open or update the text window
```

A new recording's Show(Recording) therefore wipes any leftover hypothesis
before the first live Partial of that recording arrives. Empty text never
opens a card.

Sketch the helper as a plain function so the cases above are unit-tested
without GPUI. Names can change; the transition table cannot.

```rust
struct OverlaySession {
    pill: Option<OverlayState>,
    partial: Option<String>,
}

enum OverlayCommand {
    Show(OverlayState),
    Hide,
    Partial(String),
}

fn apply_overlay_command(session: &mut OverlaySession, command: OverlayCommand) { /* … */ }
```

The async loop remains the owner of both `WindowHandle`s. The helper
decides *whether* a surface should exist; the loop opens, updates, or
removes windows to match.

## Steps

### Step 1: Extend the overlay protocol with partial text

What must be true:

- `Overlay` gains `send_partial(&self, text: &str)`. It clones the
  text, loads (does not increment) `revision`, and sends `Partial`.
- `OverlayMessage` gains `Partial { text: String, revision: u64 }`.
- The stale-message no-op arm in `run()` also matches `Partial`.
- `apply_overlay_command` (or equivalent) implements the transition
  table above. Wire `run()` through it so Show/Hide/Partial cannot
  drift from the tests.
- A Show(Recording) resets any prior partial text.

**Verify**: `just check` → exit 0. The new unit tests in Step 3's test
plan can land in this step; if they do, `just test` should pass too.

### Step 2: The text surface

- A second layer-shell window, opened and closed by the same async loop
  in `run()` that owns the pill window (one loop, two `WindowHandle`s —
  ordering stays trivial). Same `Layer::Overlay`,
  `KeyboardInteractivity::None`, transparent background.
- Namespace: `{identity.wayland_namespace}-partials`
  (`dictate-overlay-partials` / `dictate-dev-overlay-partials`).
  `UiIdentity` fields are crate-private; `app.rs` can read them.
  Same `app_id` as the pill is fine.
- Positioned above the pill: `Anchor::BOTTOM` with a bottom margin
  larger than the pill's 40px + 48px height. Use a fixed 100px bottom
  margin (40 + 48 + 12px gap) unless a one-line comment in the code
  records a tighter measured gap that still clears the pill.
- Fixed size: 420px wide, 72px tall (two lines plus padding).
  Layer-shell resize is the flagged unknown — pick these numbers and
  keep them. Do not grow the window as the hypothesis lengthens.
- New view + new card component. Do not feed two-line text through
  `Panel`: that type is a 36px capsule. Copy the pill's fill
  (`rgba(0x1e1e_1ef0)`), shadow, and padding tokens onto a rounded
  rectangle tall enough for two lines. If you extract shared tokens,
  `Panel`'s rendered pill must stay visually identical.
- Show the **tail** of the hypothesis (last two wrapped lines, clipped
  from the front). Put wrapping/clipping in a pure function, e.g.
  `clip_partial_tail(text, max_lines = 2, max_chars_per_line)`, and
  prefix an ellipsis when the front was cut. Choose a character budget
  that fits the 420px card; the tests pin the clipping, not the exact
  budget.
- Hidden until the first accepted Partial of a recording arrives, then
  fades in (~180ms, same ease as `overlay.rs`'s `MORPH_DURATION` /
  `ease_out_quart`). Later text updates must not restart the fade or
  change window size.
- Empty text never shows an empty card (the regression the
  `32a158f` fix removed — do not reintroduce an empty placeholder).

**Verify**: `just check` → exit 0; live look in Step 4.

### Step 3: Drive it from the daemon

In `feed_streaming_batches` only: alongside the existing
`eprintln!("partial: …")`, forward the changed hypothesis through
`overlay.send_partial`. Give the function the `Overlay` reference the
worker already has. Do not send empty strings; `feed` already withholds
them.

Lifecycle is owned by the UI apply table, not by new daemon branches:

- Stop → command thread Shows Transcribing → apply clears the text
  surface even if a late Keep still feeds a Partial.
- Cancel / error / empty-transcript Hide → apply ignores any Partial
  stamped with that Hide revision because `pill` is `None`.
- Next recording's Show(Recording) clears leftover text before the
  first new Partial.

**Verify**: `just test` → all pass.

### Step 4: Live verification

1. `just run daemon`; start recording and speak → pill shows waveform;
   text card fades in above it within ~1s and updates as you speak.
2. Keep talking past two lines → card shows the tail, no growth jumps.
3. Stop → card hides when the pill switches to Transcribing; final
   text still delivers.
4. Cancel mid-recording → both surfaces hide immediately and stay gone.
5. Silent recording (no speech) → no text card ever appears.
6. Watch niri for stacking/positioning artifacts between the two
   layer-shell surfaces.

**Verify**: observations (and ideally a short screen capture) noted in
the PR description.

## Test plan

Put tests next to the plain functions they cover. Follow the style of
`crates/dictate-ui/src/overlay.rs:325-378` (direct assertions on
pure helpers; no GPUI harness). Suggested home: a `#[cfg(test)]` module
in `app.rs` for protocol, and next to `clip_partial_tail` for wrapping.

Protocol cases (all through `apply_overlay_command`):

- Partial during Recording stores the text.
- Empty Partial during Recording leaves `partial` as `None`.
- Partial with no pill (`None`) is ignored.
- Partial after Hide is ignored.
- Partial after Show(Transcribing) is ignored, even though a real
  worker can still emit one at that revision.
- Show(Recording) clears a previous partial.
- Show(Transcribing) on a session that already has partial text
  clears it.
- Hide clears both pill and partial.

Tail-clip cases:

- Short string passes through unchanged.
- Long string returns only the last two wrapped lines.
- A clipped result starts with an ellipsis (or equivalent visible
  "this is a tail" mark).
- Empty input stays empty.

Rendering is verified live; no GPUI test harness.

**Verify**: `just test` → all pass;
`cargo clippy --locked --all-targets --all-features -- -D warnings` → exit 0.

## Done criteria

- [ ] `just test` → all pass
- [ ] `cargo clippy --locked --all-targets --all-features -- -D warnings` → exit 0
- [ ] Live partials visible above the pill while speaking; pill
      untouched
- [ ] No stranded text surface in any path (stop, cancel, error, empty)
- [ ] Only in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- niri misbehaves with two layer-shell surfaces from one app (stacking,
  anchor, or margin artifacts) — record exactly what the compositor
  did; the fallback (one taller window containing both cards) changes
  the pill's window and needs maintainer sign-off.
- Partials cannot ride the existing message channel without racing
  Show/Hide — describe the race precisely. The apply table above is
  the planned answer to the known Keep-after-Transcribing / Keep-after-
  Hide races; hitting those races in live use is not a STOP if the
  table drops the Partial. A *new* race (for example GPUI refusing a
  second `open_window` from the same async loop) is a STOP.
- The text card demands per-content resizing to look acceptable —
  layer-shell resize is the flagged open question; do not improvise.
- `send_partial` appears to need its own revision increment to "work"
  — that reopens the Hide/Transcribing race; write a handback instead
  of adding a second counter.
- Applying the protocol seems to require edits in
  `crates/dictate-speech/` or new daemon transition sites beyond
  `feed_streaming_batches`.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- Deferred niceties, in rough order of value: persist the dimmed tail
  into Transcribing until the final text delivers; run partials through
  `DictationFormatter` so the preview matches the delivered text;
  elapsed-time display on the pill (carried over from plan 005's
  deferral); clear `StreamingState` on cancel/empty so the worker
  stops decoding after the overlay is gone.
- If settings later grow overlay options, the partials surface should
  style against the same seam as the pill, not fork its own.
- Plan 008 also edits `daemon.rs`. Keep this change on
  `feed_streaming_batches` / `Overlay` so the diffs compose.
