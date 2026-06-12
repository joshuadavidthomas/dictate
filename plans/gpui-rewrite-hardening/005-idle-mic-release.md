# Plan 005: Release the microphone while the daemon is idle

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from dd6db2c175a3 -- src/mic.rs src/daemon.rs`
> Plans 003 and 004 intentionally modify these files first — read the live
> code, not just the excerpts below. If the *structure* differs from what
> "Current state" describes beyond 003/004's changes (error-callback wiring,
> cap signal), treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: MED (restructures the audio capture lifecycle; has a real UX
  trade-off that must be measured, not assumed)
- **Depends on**: 003 and 004 (same files; execute last)
- **Planned at**: revision `mtnsrkmyruyz` (git `dd6db2c175a3`), 2026-06-11

## Why this matters

The daemon opens the default input device once at startup and holds the
stream open forever (`src/daemon.rs:114`), discarding samples while idle.
For a dictation tool meant to run permanently in the background, that means:
the compositor/PipeWire "microphone in use" indicator is on 24/7 (users
reasonably read that as "this app is listening"), the FFT spectrum analyzer
chews CPU on every audio batch around the clock, and the device is held even
when the user never dictates. The mic should be open exactly while a
dictation session is active — matching the overlay, which already appears
only during recording/transcription.

The trade-off: opening the device on `start` adds latency, and speech during
that window is lost. The plan's acceptance criterion is honest measurement —
if open latency turns out to clip speech noticeably, that's a design fork to
hand back, not a detail to paper over.

## Current state

- `src/daemon.rs:103-146` — `spawn_microphone_worker` runs one thread that:
  builds the recognizer, calls `crate::mic::capture(...)` **once** (line
  114), binds the result to `_mic` for the thread's lifetime, then loops
  forever polling `take_utterance()` every 20ms (`POLL_INTERVAL`,
  `src/daemon.rs:23`).
- `src/mic.rs:37-77` — `capture()` picks the default input device, builds a
  cpal stream that pushes downmixed f32 samples into an rtrb ring buffer,
  and spawns `audio_worker` (a second thread) that drains the ring,
  resamples to 16kHz, feeds `dictation.record_samples` and the spectrum
  analyzer → `overlay.send_spectrum` (`src/mic.rs:130-163`). The returned
  `Mic` struct holds `_stream` and `_worker`.
- **Threading constraint that shapes the design**: `cpal::Stream` is
  `!Send`. The stream must be created, held, and dropped on the same
  thread. Today that thread is the microphone worker thread — which is also
  the thread that already wakes every 20ms and can see `dictation.phase()`.
- `src/mic.rs:141-163` — `audio_worker` loops forever; it has no exit
  condition. If streams become per-session, this thread must terminate when
  its producer side is dropped (rtrb's consumer exposes `is_abandoned()`),
  or each session leaks a thread.
- `src/dictation.rs:188-190` — `phase()` gives any thread a cheap view of
  Idle/Recording/Transcribing/Unavailable.
- `src/app.rs:27-45` — `Overlay` is the in-process handle; `show`/`hide`
  are queued messages. The daemon command loop calls `overlay.show()` on
  `Started` (`src/daemon.rs:78-81`).
- Plan 003 added (if executed first, as ordered): stream error callback
  wiring to `mark_unavailable` + overlay hide. Plan 004 added: recording cap
  signal from `record_samples`. Preserve both behaviors through this
  restructure.

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon` (needs Wayland + mic)     | daemon ready line   |

## Scope

**In scope**:
- `src/mic.rs`
- `src/daemon.rs`

**Out of scope** (do NOT touch):
- `src/dictation.rs` state machine — `phase()` already exposes what you need.
- `src/app.rs` / overlay internals — message ordering is already correct.
- Replacing the 20ms poll with a condvar/channel — nice-to-have, only do it
  if it falls out naturally from the restructure; it is not a goal.
- Device selection / settings — deferred direction work.

## Steps

### Step 1: Make capture session-scoped

Restructure so the mic worker thread owns an `Option<Mic>` and drives it
from the phase:

- Phase `Recording` and no open stream → open one (today's `capture()`
  body), keep it for the session.
- Phase `Idle`/`Unavailable` and a stream is open → drop it (drop closes
  the cpal stream and abandons the ring producer).
- During `Transcribing`: the stream is no longer needed (samples are only
  recorded during `Recording`) — dropping at stop is correct; the overlay's
  waveform freezing/decaying during transcription is acceptable and arguably
  clearer than the current still-live waveform.
- The existing 20ms poll loop is the natural place for this check; device
  open then costs at most one poll tick + actual device-open time.
- `audio_worker` must exit when its session ends: terminate when the ring is
  empty and the producer is abandoned (`Consumer::is_abandoned`). Verify no
  thread leak across repeated sessions.
- Keep the recognizer setup (model download etc.) at thread start as today —
  only the mic becomes session-scoped. Keep plan 003's error-callback wiring
  (it now marks the *session* unavailable) and plan 004's cap signal intact.
- Startup messaging: today "microphone ready" prints at startup
  (`src/daemon.rs:115`); move/reword honestly (e.g. print device info at
  first open per session, or drop the line) — don't claim a mic is open
  when it isn't.

**Verify**: `just check` → exit 0; `just test` → all pass.

### Step 2: Don't clip the first words — overlay as the "speak now" signal

What must be true: **when the overlay pill is visible, samples are being
captured.** Today `overlay.show()` fires immediately on `Started`
(`src/daemon.rs:78-81`) while the mic may still be opening. Pick the
cheapest arrangement that restores the invariant — e.g. the worker triggers
`overlay.show()` once the stream is playing, instead of the command loop
doing it. Cancel/stop paths must still hide the overlay exactly as before.

**Verify**: `just check` → exit 0; live run (Step 3) confirms ordering.

### Step 3: Measure open latency (live verification)

On a real Wayland session with a mic:

1. `just run daemon`, then time `dictate record toggle` → instrument with a
   temporary `eprintln!` timestamp pair (command received → stream playing /
   first samples flowing). Run it ~5 times; note cold vs warm numbers.
2. Confirm the desktop's mic-in-use indicator turns **off** when idle and
   **on** only during recording.
3. Dictate a phrase starting immediately when the pill appears; confirm the
   transcript's first word isn't clipped.
4. Remove the temporary instrumentation; record the measured numbers in the
   PR description.

Acceptance: indicator off while idle; open latency ≤ ~200ms warm, no
first-word clipping when speech starts at pill-appearance. If latency
exceeds that or words clip → STOP condition (design fork: pre-roll buffer,
paused-stream approach, or accepting always-on with a settings toggle — the
maintainer chooses).

**Verify**: PR description contains the latency numbers and the indicator
observation.

### Step 4: Tests

`LinearResampler` tests must still pass untouched. Add what's testable
without hardware:

- If the restructure introduces a session-decision function (e.g. "given
  phase + stream-open state, open/close/keep"), unit-test that mapping
  directly.
- `audio_worker` termination: construct a ring buffer, drop the producer,
  assert the worker function returns (pattern: direct function call with
  rtrb handles, as `LinearResampler` tests do with plain values —
  `src/mic.rs:222-257`).

**Verify**: `just test` → all pass; `cargo clippy --all-targets -- -D warnings`
→ exit 0.

## Done criteria

- [ ] `just test` → all pass, including new worker-termination test
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] Live check (Step 3) done with numbers in the PR description
- [ ] Idle daemon: no input stream open (mic indicator off)
- [ ] Only `src/mic.rs` and `src/daemon.rs` modified (`jj st`)

## STOP conditions

Stop if:

- Measured open latency > ~200ms warm or first words clip (Step 3) — this is
  the anticipated design fork; hand back with the numbers.
- `cpal::Stream`'s `!Send` constraint can't be satisfied inside the worker
  loop without a dedicated stream thread per session that complicates
  shutdown — describe the structure you ended up needing.
- Preserving plan 003's error-callback behavior or plan 004's cap conflicts
  with the session-scoped structure.
- Repeated sessions leak threads or the second session fails to open the
  device (some backends are slow to release) — hand back with the backend
  (PipeWire/ALSA) and the failure mode.

On stopping, write a **handback**: current state, desired outcome, lingering
questions. Descriptive, not prescriptive.

## Maintenance notes

- This makes "mic open" a per-session resource; future device-selection
  settings plug in at the open point.
- The continuous/meeting-transcription mode envisioned in PLAN.md will want
  a long-lived stream again — keep `capture()` reusable for that caller
  rather than fusing it into the dictation loop.
- Reviewers: race between cancel (command thread) and stream-open (worker
  thread) — a session cancelled mid-open must still drop the stream and
  hide the overlay.
