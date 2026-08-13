# Plan 007: Live partials text surface above the overlay pill

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 70c4fae943fa -- crates/dictate-ui/ crates/dictate/src/daemon.rs`
> This plan was written against that revision (streaming partials +
> settings validation). Read the live code; on a mismatch beyond
> incidental edits, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: MED (second layer-shell surface is new ground; niri
  surface-behavior is the main unknown — PLAN.md flags layer-shell
  resize as an open question)
- **Depends on**: streaming partials (landed 2026-08-07, `9de89389`);
  overlay phase states (audited DONE 2026-08-13)
- **Planned at**: git `70c4fae943fa`, 2026-08-13

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

- `crates/dictate/src/daemon.rs:701-712` — `feed_streaming_batches`
  drains mic batches into `StreamingSession::feed` on the worker's 20ms
  poll and `eprintln!("partial: …")`s changed hypotheses. This is the
  single point where partial text is born.
- `crates/dictate-ui/src/app.rs` — one bottom-anchored layer-shell
  window (80×48, `Anchor::BOTTOM`, 40px bottom margin, `Layer::Overlay`,
  no keyboard interactivity). `Overlay` handle sends revision-guarded
  `OverlayMessage::{Show, Hide}` through one ordered unbounded channel;
  the async loop in `run()` owns the `WindowHandle` and applies messages
  only when the revision is current. Spectrum bypasses the channel on a
  lock-free `SpectrumLevels` side path.
- `crates/dictate-ui/src/overlay.rs` — `OverlayView` renders per-state
  visuals inside `components::Panel`; animation runs on the wake/timer
  loop (do not touch its pacing; see the in-code comment about GPUI
  frame callbacks).
- `crates/dictate-ui/src/components/panel.rs` — the pill chrome
  (`Panel::new(id, width, label)`), the design language the text surface
  should share.
- Partial cadence: a handful of updates per second at most (hypothesis
  changes, not per-batch) — orders of magnitude below spectrum rate.

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon` (Wayland + mic)           | partials visible    |

## Scope

**In scope**:
- `crates/dictate-ui/src/app.rs` (protocol + second surface lifecycle)
- `crates/dictate-ui/src/overlay.rs` / new view module for the text
  surface
- `crates/dictate-ui/src/components/` (text card component)
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

## Steps

### Step 1: Extend the overlay protocol with partial text

What must be true:

- `Overlay` gains `send_partial(text: &str)` (or equivalent). Partials
  travel through the **existing ordered message channel** as a new
  `OverlayMessage::Partial { text, revision }` variant carrying the
  revision current at send time — the revision guard then drops any
  partial that arrives after its recording's Hide, exactly as Show/Hide
  are guarded today. Do not add a second channel and do not use the
  spectrum side path (text is low-rate and must stay ordered relative
  to Show/Hide).
- A new recording's Show resets/clears any prior partial text.

**Verify**: `just check` → exit 0.

### Step 2: The text surface

- A second layer-shell window, opened and closed by the same async loop
  in `run()` that owns the pill window (one loop, two `WindowHandle`s —
  ordering stays trivial). Same `Layer::Overlay`,
  `KeyboardInteractivity::None`, transparent background; its own
  namespace derived from `UiIdentity`.
- Positioned above the pill: `Anchor::BOTTOM` with a bottom margin
  larger than the pill's 40px + 48px height (i.e. ≥ ~96px, leaving a
  small gap). Fixed size chosen by the executor (suggest ~420px wide,
  tall enough for 2 lines) — layer-shell resize is the flagged
  unknown, so pick one size and keep it.
- Design intent (executor has visual freedom within these constraints):
  shares the pill's design language (a `Panel`-like card); shows the
  **tail** of the hypothesis (last 1–2 lines, clipped from the front);
  hidden until the first partial of a recording arrives, then fades in;
  no flicker or geometry jumps as text grows.
- Empty text never shows an empty card (the regression the
  `32a158f` fix removed — do not reintroduce an empty placeholder).

**Verify**: `just check` → exit 0; live look in Step 4.

### Step 3: Drive it from the daemon

In `feed_streaming_batches` only: alongside the existing
`eprintln!("partial: …")`, forward the changed hypothesis through the
`Overlay` handle (the worker already holds a clone). Lifecycle
decisions:

- Recording stops → the partials surface hides when the pill switches
  to Transcribing (simplest correct behavior; persisting dimmed tail
  text into Transcribing is a deferred nicety — see maintenance notes).
- Cancel / error / empty-transcript paths → surface hides with the pill
  via the same revision-guarded Hide; no path may strand it on screen.

**Verify**: `just test` → all pass.

### Step 4: Live verification

1. `just run daemon`; start recording and speak → pill shows waveform;
   text card fades in above it within ~1s and updates as you speak.
2. Keep talking past two lines → card shows the tail, no growth jumps.
3. Stop → card and pill transition cleanly; final text delivers.
4. Cancel mid-recording → both surfaces hide immediately.
5. Silent recording (no speech) → no text card ever appears.
6. Watch niri for stacking/positioning artifacts between the two
   layer-shell surfaces.

**Verify**: observations (and ideally a short screen capture) noted in
the PR description.

## Test plan

The testable core is protocol ordering and tail-clipping: unit-test the
tail-of-text windowing function, and the revision guard behavior for
`Partial` (a partial sent before Hide is applied, one sent after is
dropped) if the message-application logic is extractable as a plain
function. Rendering is verified live; no GPUI test harness.

**Verify**: `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

## Done criteria

- [ ] `just test` → all pass
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
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
  Show/Hide — describe the race precisely.
- The text card demands per-content resizing to look acceptable —
  layer-shell resize is the flagged open question; do not improvise.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- Deferred niceties, in rough order of value: persist the dimmed tail
  into Transcribing until the final text delivers; run partials through
  `DictationFormatter` so the preview matches the delivered text;
  elapsed-time display on the pill (carried over from plan 005's
  deferral).
- If settings later grow overlay options, the partials surface should
  style against the same seam as the pill, not fork its own.
