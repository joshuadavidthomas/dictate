# Plan 004: Bound recording length and document the Whisper 30-second limit

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from dd6db2c175a3 -- src/dictation.rs src/mic.rs src/daemon.rs README.md`
> If in-scope files have changed since this plan was written (plan 003
> intentionally touches `src/daemon.rs`/`src/mic.rs` first — re-read those
> regions), compare the "Current state" excerpts against the live code
> before proceeding; on a semantic mismatch, treat it as a STOP condition.

## Status

- **Effort**: S–M
- **Risk**: LOW (additive guard on the capture path, plus docs)
- **Depends on**: 003 (same files; execute 003 first)
- **Planned at**: revision `mtnsrkmyruyz` (git `dd6db2c175a3`), 2026-06-11

## Why this matters

Recording is started and stopped manually (`dictate record toggle`). If the
user forgets to stop — the overlay is a small pill that's easy to miss —
the sample buffer grows without bound at 16,000 f32/s (~230 MB/hour), and
the eventual transcription of an hour of audio is useless. Worse, the
default model is Whisper via sherpa-onnx's **offline** API, and Whisper ONNX
models process a fixed 30-second window: everything after ~30s is silently
discarded at decode time. Today a 5-minute dictation appears to work and
quietly returns only the first sentence or two. A bounded recording with an
explicit auto-stop converts silent loss into predictable, visible behavior.

## Current state

- `src/dictation.rs:192-197` — samples accumulate unboundedly:

  ```rust
  pub(crate) fn record_samples(&self, new_samples: &[f32]) {
      let mut state = self.state.lock().unwrap();
      if let DictationControlState::Recording { samples, .. } = &mut *state {
          samples.extend_from_slice(new_samples);
      }
  }
  ```

- `src/mic.rs:130-163` — `audio_worker` calls `dictation.record_samples(&samples)`
  per ~256-sample batch; it has no view of recording duration.
- `src/dictation.rs:115-127` — `DictationControl::apply` is the only
  entry point for state transitions; `stop_recording`
  (`src/dictation.rs:147-172`) moves `Recording → Transcribing` and queues
  the `CapturedUtterance`.
- `src/daemon.rs:103-146` — the transcription worker polls
  `take_utterance()` every 20ms and hides the overlay after transcribing;
  the command loop (`src/daemon.rs:63-101`) prints a status line per
  transition. An auto-stop that goes through the same
  `Recording → Transcribing` transition gets all of this behavior for free.
- `src/transcription.rs:7-8` — precedent for capture-quality constants:
  `MIN_DICTATION_DURATION`, `MIN_DICTATION_RMS`.
- `README.md:7-21` — "Current state" feature list; the place a user-facing
  limitation note belongs.
- Tests: `src/dictation.rs:256-321` is the exemplar — direct
  `DictationControl` manipulation (`apply`, `record_samples`,
  `take_utterance`).

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Tests     | `just test dictation::`                     | all pass            |
| All tests | `just test`                                 | all pass            |
| Check     | `just check`                                | exit 0              |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |

## Scope

**In scope**:
- `src/dictation.rs` (the cap and its tests)
- `src/mic.rs` and/or `src/daemon.rs` (only the minimal wiring that reacts
  to the cap — see Step 2)
- `README.md` (limitation note)

**Out of scope** (do NOT touch):
- `src/models.rs`, `src/text.rs`, `src/transcription.rs`.
- Chunked/segmented transcription of long audio (the real fix for >30s
  dictation) — deferred; see effort README.
- Per-model duration limits or any settings/config surface — there is no
  settings system yet (deferred direction work).

## Steps

### Step 1: Verify the Whisper 30-second claim (investigation gate)

Confirm what sherpa-onnx's offline Whisper recognizer does with input longer
than 30 seconds. Acceptable evidence, in preference order:

1. The sherpa-onnx documentation/source: the k2-fsa sherpa docs state the
   Whisper ONNX export supports only ≤30s of audio per decode
   (https://k2-fsa.github.io/sherpa/onnx/pretrained_models/whisper/index.html);
   confirm against the version in `Cargo.lock` (`sherpa-onnx` 1.13).
2. The crate source vendored locally (`cargo doc -p sherpa-onnx` /
   `~/.cargo/registry/src/.../sherpa-onnx-*`), looking at the offline
   Whisper model wrapper.

Record the finding (one paragraph, with the source you used) in the PR
description and in the README note (Step 4). If the claim is **false** for
this version — long audio is chunked internally and transcribes fully —
skip Step 4's limitation wording, still land the cap (memory/UX grounds),
and say so in the PR.

**Verify**: the PR description contains the evidence paragraph. (No code
change in this step.)

### Step 2: Cap recording duration with an auto-stop

What must be true:

- A constant `MAX_DICTATION_DURATION` (suggest 120s — generous for the
  memory concern without pretending >30s transcribes well; adjust if Step 1
  changes the picture) lives next to the recording state, mirroring the
  `MIN_DICTATION_*` precedent in `src/transcription.rs:7-8`.
- When appended samples reach the cap, the recording **auto-stops through
  the normal stop transition** (`Recording → Transcribing`, utterance
  queued) — not by silently dropping samples, and not by discarding the
  recording. The user gets whatever they said up to the cap, transcribed
  and delivered as usual; the overlay hides when transcription completes,
  exactly as a manual stop does.
- Whoever detects the cap (suggestion: `record_samples` returns a
  signal/enum the `audio_worker` in `src/mic.rs` reacts to by invoking the
  stop transition, keeping the decision in `DictationControl` and the
  side effects on the worker thread) prints one `eprintln!` status line in
  the daemon's existing voice, e.g.
  "dictation reached the N s limit; transcribing captured audio".
- Exactly at the boundary, the captured utterance length is ≤ the cap
  (truncate the final batch; don't overshoot by one batch).

**Verify**: `just check` → exit 0; new tests from Step 3 pass.

### Step 3: Tests

In `src/dictation.rs`'s tests module (pattern:
`recording_stops_to_captured_utterance`, `src/dictation.rs:276-294`):

- Feeding exactly cap-many samples transitions to Transcribing and yields an
  utterance of exactly the cap length.
- Feeding a batch that crosses the cap yields an utterance truncated to the
  cap, not the overshoot length.
- After the auto-stop, further `record_samples` calls are ignored (state is
  Transcribing) and a subsequent `take_utterance` + `finish_transcription`
  returns to Idle.
- A short recording (under the cap) is unaffected — existing tests stay
  green.

If the cap constant makes tests slow/awkward at 16kHz·120s, parameterize the
cap for tests (e.g. compute from a `SampleRate` + `Duration` so tests can
use a tiny rate) rather than allocating 2M-sample vectors — `SampleRate::new(4)`
precedent at `src/dictation.rs:314-320`.

**Verify**: `just test` → all pass.

### Step 4: README limitation note

In `README.md`'s "Current state" section, add one or two sentences: manual
recordings auto-stop at the cap, and (if Step 1 confirmed) Whisper models
transcribe only the first ~30 seconds of a capture — longer dictation needs
the future chunking work.

**Verify**: `rg -n "30" README.md` → the note exists.

## Test plan

Covered in Step 3. **Verify**: `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

## Done criteria

- [ ] `just test` → all pass, including ≥3 new cap tests
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] `rg -n "MAX_DICTATION" src/` → constant exists and is used
- [ ] README contains the limitation note (or the PR explains why not, per Step 1)
- [ ] Only in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- The code at the "Current state" locations doesn't match the live tree in a
  way plan 003's known edits don't explain.
- Step 1 produces contradictory evidence about the 30s behavior — handback
  with both sources rather than guessing.
- The auto-stop can't reuse the normal `Recording → Transcribing` transition
  without restructuring `DictationControl`'s locking — describe the
  conflict.

On stopping, write a **handback**: current state, desired outcome, lingering
questions. Descriptive, not prescriptive.

## Maintenance notes

- The honest fix for long dictation is VAD-segmented or windowed decoding on
  the continuous path (PLAN.md's "Dictation versus continuous transcription"
  section); the cap is a guardrail, not the feature.
- When a settings system lands (deferred direction work), the cap becomes a
  setting and may become model-aware (Parakeet/Moonshine handle longer audio
  than Whisper).
- Reviewers: check the boundary-truncation math at the final batch and that
  the auto-stop's status line can't print more than once per session.
