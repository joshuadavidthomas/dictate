# Plan 005: Show "Recording" only when the microphone is actually live

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate/src/daemon.rs crates/dictate-ui/src/overlay.rs`
> Plan 004 adds an env-gated save block to `daemon.rs`'s worker loop — read
> the live code. If the mic open/overlay flow differs structurally from the
> excerpts below, STOP.

## Status

- **Effort**: M
- **Risk**: MED (touches overlay UI states; visual result needs maintainer sign-off)
- **Depends on**: none (recommended after 004 to avoid `daemon.rs` churn)
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

The overlay shows **Recording** the instant a start command is accepted, but
the microphone stream opens later: the mic worker notices on its next 20 ms
poll, then pays PipeWire device-open latency (commonly ~100 ms+). Speech in
that window is silently lost — the classic clipped first word — while the UI
actively invites the user to speak into a dead mic. This plan makes the UI
honest (a distinct "opening" state until the stream is live) and finally
takes the measurement that `plans/gpui-rewrite-hardening/005-idle-mic-release.md`
promised: log the actual open latency, so the standing-pre-roll question can
be reopened with data instead of vibes.

**Decision context**: keeping the mic *closed* while idle is a recorded,
deliberate decision (privacy indicator, idle CPU — see that plan's "Why this
matters"). A standing pre-roll stream would reverse it and is explicitly out
of scope; it was considered and rejected for this effort (see the effort
README).

## Current state

- `crates/dictate/src/daemon.rs:421-429` — on `DictationUpdate::Started`:

  ```rust
  self.overlay.show(OverlayState::Recording);   // shown before mic exists
  if self.dictation.begin_recording() { ... }
  ```

- `crates/dictate/src/daemon.rs:570-609` — `run_microphone_worker` loop:
  sleeps `POLL_INTERVAL` (20 ms, `daemon.rs:55`), sees the new recording id,
  calls `capture(...)` (the blocking device open), and on success hits
  `mic = Some(...)` + the `"dictation started"` log inside the
  `if dictation.recording_id() == Some(recording_id)` guard
  (`daemon.rs:600-603`). The worker already holds `overlay: &Overlay`.
- On open failure (`daemon.rs:590-598`): `abort_recording` + `overlay.hide()`.
- `crates/dictate-ui/src/overlay.rs:35-43` — `OverlayState` enum:
  `Recording, Transcribing, PendingTranscript, InsertionUncertain,
  DeliveryFailed, NoTranscript, NothingToPaste`. Each variant carries an
  `accessible_label` (overlay.rs:45+) and a rendered presentation; matches
  over this enum are exhaustive, so adding a variant is compiler-guided
  (expect match arms in `dictate-ui` and possibly `dictate-dev`'s debug
  overlay screen — the latter is allowed to change for exhaustiveness).
- Phase labels: `DictationPhase::label()`
  (`crates/dictate-speech/src/dictation.rs:79-90`) — daemon-side strings,
  unaffected unless you choose to reuse their wording.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Workspace tests | `just test` | all pass (runs cargo + npm) |
| Typecheck | `just check` | exit 0 |
| Lint | `just lint` | exit 0 |
| Debug overlay smoke | `just debug-eval` | jq pipeline exits 0 |

## Scope

**In scope**:
- `crates/dictate/src/daemon.rs`
- `crates/dictate-ui/src/overlay.rs` (new state variant + presentation)
- `crates/dictate-dev/src/**` — only match-arm exhaustiveness fixes

**Out of scope**:
- `crates/dictate-speech/**` — no capture-lifecycle changes; the mic still
  opens per-session (recorded decision, see above).
- Any pre-roll/standing-stream machinery.

## Steps

### Step 1: Add the opening state to the overlay

New `OverlayState` variant (suggested name: `OpeningMicrophone`) with an
accessible label like "Opening microphone…". Presentation: visually distinct
from `Recording` enough that "safe to talk" is unambiguous — the minimal
honest shape is the Recording layout with the waveform inactive and the
label swapped; do not invent new visual language beyond that (maintainer
reviews the result). Fix all exhaustive matches the compiler surfaces.

**Verify**: `just check` → exit 0; `just debug-eval` → exit 0.

### Step 2: Sequence the daemon's overlay transitions

- `daemon.rs:422`: `show(OverlayState::Recording)` →
  `show(OverlayState::OpeningMicrophone)`.
- In `run_microphone_worker`, inside the success guard at
  `daemon.rs:600-603` (same place as the `"dictation started"` log):
  `overlay.show(OverlayState::Recording)`. It must be inside the
  `recording_id` match guard so a superseded recording never flips the
  overlay back to Recording.
- Failure path (`daemon.rs:590-598`) already hides the overlay — confirm it
  still runs after the change (the user sees Opening… then nothing, plus
  the existing stderr explanation).

**Verify**: `just check` → exit 0.

### Step 3: Measure and log open latency

Wrap the `capture(...)` call (`daemon.rs:580-588`) with `Instant::now()` and
log: `"microphone opened in {}ms"` on success. This is the measurement the
idle-mic-release plan deferred; it accumulates in daemon logs for the
pre-roll debate.

**Verify**: `just check` → exit 0. With a mic available:
`cargo run -p dictate` + one dictation → log shows Opening→open latency line
→ Recording; without a mic, state so in the completion summary.

## Test plan

The dictation state machine is unchanged (this plan re-sequences *overlay*
calls, which are fire-and-forget UI messages), so existing daemon tests
should pass untouched — treat any daemon test failure as a STOP, not a test
to edit. New coverage:

- If `dictate-ui` has unit tests over `OverlayState` (labels/presentation),
  extend them for the new variant following the file's existing pattern.
- **Verify**: `just test` → all pass.

## Done criteria

- [ ] `just test` → all pass; `just check` → exit 0; `just lint` → exit 0
- [ ] `just debug-eval` → exit 0
- [ ] Overlay never shows `Recording` before `capture()` has returned `Ok`
      for the current recording id (by code inspection of the two call
      sites; cite both lines in the completion summary)
- [ ] Open-latency log line present on the success path
- [ ] No files outside the in-scope list modified

## STOP conditions

Stop and write a handback if:

- The overlay `show` calls turn out not to be safely callable from the mic
  worker thread for a *new* state (they're already called there for
  `Transcribing`/`NoTranscript`, so this would indicate drift).
- Making the opening state visually distinct requires real overlay design
  work (new layout/animation) — that's maintainer-taste territory, hand it
  back with a description of the options.
- Any existing daemon test fails — the state machine was not supposed to
  change.
- Measured open latency in your smoke run exceeds ~500 ms consistently —
  worth handing back as data, since it strengthens the pre-roll case the
  maintainer deferred.

## Maintenance notes

- The `OpeningMicrophone` → `Recording` transition is the user's "safe to
  talk" signal; any future change to mic-open flow must preserve it.
- The latency log is the evidence stream for ever reopening the pre-roll
  decision — don't remove it as log cleanup.
- Deferred deliberately: pre-roll/standing stream (reverses the
  idle-mic-release decision; reopen only with the latency data this plan
  starts collecting).
