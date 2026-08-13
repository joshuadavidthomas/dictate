# Plan 005: Give the overlay distinct recording, transcribing, and error states

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from e65b4661cfcf -- src/app.rs src/overlay.rs src/daemon.rs src/components/`
> gpui-rewrite-hardening 006 (frame pacing) and 003/005 (overlay
> show/hide ordering, mic lifecycle) intentionally modify these files
> first — this plan REQUIRES 006's smooth-animation outcome and builds on
> 003/005's ordering. Read the live code; on a mismatch beyond those
> documented edits, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: LOW–MED (UI-only, but touches the overlay message protocol the
  daemon depends on)
- **Depends on**: gpui-rewrite-hardening 006 (no point polishing states on
  a choppy overlay) and 003/005 (they reorder/restructure the show/hide
  call sites this plan extends)
- **Planned at**: revision `pkzmprvzlnsn` (git `e65b4661cfcf`), 2026-06-11

## Why this matters

PLAN.md:206 commits to an overlay state contract — "idle, recording with
spectrum/timer, transcribing, error" — but today the overlay has exactly
one visual: the live waveform. The user cannot tell "still recording" from
"transcribing now" from "something failed"; after stop, the pill just sits
there animating from a mic that's no longer relevant until text appears.
The 2026 survey of premium dictation apps found phase feedback is table
stakes: Wispr Flow's pill swaps waveform → pulsing loader → error triangle
with deliberately smooth transitions; Superwhisper shows a color-coded
status dot (sources: docs.wisprflow.ai "Flow Bar" articles,
superwhisper.com/docs/get-started/interface-rec-window). Phase states are
the single highest-leverage *feel* improvement after the FPS fix — they
make the app legible.

## Current state

- `src/app.rs:27-51` — `Overlay` handle exposes `show()`, `hide()`,
  `send_spectrum()`; `enum OverlayMessage { Show, Hide }` is the entire
  protocol. The GPUI side consumes messages in an async loop
  (`src/app.rs:64-98`) and opens/removes the layer-shell window.
- `src/overlay.rs` — `OverlayView` owns `SpectrumLevels` and a 16ms
  timer + `cx.notify()` loop (post-hardening-006 this is the verified
  smooth pacing — do not change the pacing mechanism); rendering is the
  `Panel` + `Waveform` components (`src/components/panel.rs`,
  `src/components/waveform.rs`).
- Daemon call sites that define the phase timeline (post-hardening
  ordering): `overlay.show()` on `Started` (`src/daemon.rs:78-81` — after
  hardening 005, show fires when the stream is actually capturing);
  `overlay.hide()` around transcription completion (`src/daemon.rs:136-137`,
  reordered by hardening 003); `overlay.hide()` on `Cancelled`
  (`src/daemon.rs:88-91`); error paths mark the session unavailable
  (hardening 003 wires stream errors to overlay hide).
- `src/dictation.rs:61-70` — `DictationPhase { Idle, Recording,
  Transcribing, Unavailable }` with labels; the daemon already knows the
  phase at every transition.
- Component conventions: `#[derive(IntoElement)]` + `RenderOnce` for
  reusable pieces, `ParentElement` for child slots (AGENTS.md; exemplar
  `src/components/panel.rs`).

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon` (Wayland + mic)           | overlay phases visible |

## Scope

**In scope**:
- `src/app.rs` (overlay handle + message protocol)
- `src/overlay.rs` (view state + rendering per phase)
- `src/components/` (new/changed visual components)
- `src/daemon.rs` (only adding `set_phase`-style calls at the existing
  transition sites — no control-flow changes)

**Out of scope** (do NOT touch):
- The frame pacing mechanism (hardening 006's verified design — including
  its "do not use `request_animation_frame`" constraint and code comment).
- Elapsed-time display (PLAN.md:206 mentions a timer) — defer; visual
  phase identity first, content later.
- Overlay position/size settings — future settings work.
- `src/mic.rs`, `src/dictation.rs` — phase already exists; consume it.

## Steps

### Step 1: Extend the overlay protocol with a phase

What must be true:

- `Overlay` gains a phase-setting method (e.g.
  `set_phase(OverlayPhase)`) where `OverlayPhase` is an overlay-owned enum
  (suggest `Recording`, `Transcribing`, `Error`) — distinct from
  `DictationPhase` (the overlay never shows Idle/Unavailable; it's hidden
  then). Map at the daemon seam, types-first (AGENTS.md integration-
  boundaries rule).
- Messages remain ordered with Show/Hide through the same channel so a
  phase can never arrive after its window is gone (the `src/app.rs:64-98`
  loop already guarantees ordering — extend `OverlayMessage`, don't add a
  second channel; spectrum stays on its lock-free side path).
- `OverlayView` stores the current phase and re-renders on change.

**Verify**: `just check` → exit 0.

### Step 2: Render the phases

Design intent (executor has visual freedom within these constraints):

- **Recording**: today's live waveform — unchanged.
- **Transcribing**: the waveform stops tracking the mic and decays to a
  calm, clearly-distinct "working" visual (e.g. gentle idle pulse of the
  bars or a subtle shimmer) — the user must be able to glance and know
  "it heard me, it's thinking". No spinner clichés required; keep the
  pill's design language.
- **Error**: a brief, visually unmistakable state (e.g. bars flash to a
  warning tint) shown for ~1–2s before the daemon hides the overlay; it
  must never strand a permanent error pill on screen.
- Transitions must not flicker or jump the pill's geometry (Wispr Flow's
  bar is praised precisely for "no flickering or positioning artifacts");
  if a phase needs a different pill width, animate or pick one size.
- All phase visuals run inside the existing paced render loop — no new
  timers per phase.

**Verify**: `just check` → exit 0; live look in Step 4.

### Step 3: Drive phases from the daemon

At the existing transition sites only: `Started` → show + Recording;
`Stopped` (entering transcription) → Transcribing; transcription complete
/ cancelled → hide (as today); stream/transcription error (hardening 003's
wiring) → Error then hide. The daemon's stderr lines stay untouched.

**Verify**: `just test` → all pass (existing daemon tests unaffected).

### Step 4: Live verification

1. `just run daemon`; toggle recording → pill appears with live waveform.
2. Toggle stop while speaking → visual flips to Transcribing within one
   frame-ish; text delivers; pill hides.
3. Cancel mid-recording → pill hides immediately (no Transcribing flash).
4. Force an error if cheaply possible (e.g. unplug/suspend the input
   device mid-recording) → Error visual, then hidden, daemon still alive.
5. Watch transitions for flicker/geometry jumps.

**Verify**: observations (and ideally a short screen capture) noted in the
PR description.

## Test plan

Phase mapping is the testable core: unit-test the daemon-side
`DictationPhase`/update → `OverlayPhase` mapping function if you introduce
one (pattern: plain-function tests as in `src/dictation.rs:256+`).
Rendering is verified live; do not build a GPUI test harness for this.

**Verify**: `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

## Done criteria

- [ ] `just test` → all pass
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] Live run shows three visually distinct states with clean transitions
- [ ] No permanent overlay in any failure path (error always resolves to
      hidden)
- [ ] Only in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- Hardening 006 hasn't landed or the overlay still isn't smooth — polishing
  states on a choppy canvas wastes the work.
- The current `OverlayMessage` ordering can't carry phases without racing
  spectrum updates or show/hide — describe the race precisely.
- The Error state requires daemon control-flow changes beyond adding calls
  at existing sites (that belongs to the hardening track's owner).
- Phase visuals demand per-phase window sizes and layer-shell resize
  misbehaves on niri (PLAN.md:256 flags surface-resize as an open
  question) — record what the compositor did.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- The elapsed-time/timer display and an "amplitude too low" hint are the
  natural next layers on `OverlayPhase` — both deferred deliberately.
- When settings (plan 003) grow overlay options (position, size), the
  phase enum is the stable seam they style against.
- Reviewers: scrutinize the Error → hide timing — a sleep on the daemon
  thread is wrong; the hide should be scheduled without blocking command
  processing.
