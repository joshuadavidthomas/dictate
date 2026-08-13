# Plan 006: Spike — evaluate Silero VAD as Dictate's speech front-end

> **Executor instructions**: This is a **design spike**, not a build plan.
> The deliverable is a findings memo plus a go/no-go recommendation — not
> shipped code. Prototype code lives in a scratch branch/change and is
> expected to be discarded. Follow the steps, run the verifications, and
> STOP into a handback rather than expanding scope.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate-speech/src/transcription.rs crates/dictate-speech/src/models.rs`
> If the noise-filtering or model-catalog code has changed structurally
> since planning, note it in the memo; it rarely blocks a spike.

## Status

- **Effort**: M–L
- **Risk**: LOW (no production changes; risk is spending the time)
- **Depends on**: 001 (the degradation matrix is the measurement substrate)
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

Dictate has no voice-activity detection. The costs today: non-speech
recordings are filtered by a brittle English string blocklist
(`transcript_is_noise`, `crates/dictate-speech/src/transcription.rs:167-179`,
literal-matches "cough", "music", "buzz"…); silence is decoded at full cost
(matters at the 10-minute recording cap); and "no speech detected" can only
be inferred after a full ASR pass. The `sherpa-onnx` crate already exposes
Silero VAD (`VoiceActivityDetector`, `SpeechSegment` — crate source
`src/vad.rs`), so the marginal cost is a ~2 MB model asset plus integration
design. **The critical risk this spike must retire**: a VAD with default
thresholds could rebuild the exact bug this effort exists to prevent —
rejecting quiet-but-valid speech (the RMS-gate failure, but smarter-looking).

## Current state

- `crates/dictate-speech/src/transcription.rs:133-154` — `transcribe()`:
  duration gate (400 ms) → `recognizer.decode` → `transcript_is_noise`
  string check. The natural VAD insertion point is between the duration gate
  and decode (trim + speech-presence check on the finished utterance).
- `crates/dictate-speech/src/models.rs` — model catalog: download,
  verification, and local-dir layout for ASR models
  (`ensure_downloaded`, `local_models_dir`). A VAD model would ride this
  machinery; part of the spike is confirming the catalog's shape fits a
  non-ASR asset.
- sherpa-onnx 1.13 VAD API (crate source `src/vad.rs`):
  `VadModelConfig` / `SileroVadModelConfig` (threshold, min silence/speech
  durations, window size), `VoiceActivityDetector::create(config,
  buffer_size_in_seconds)`, `accept_waveform`, `detected`, `front() ->
  Option<SpeechSegment>`, `pop`, `flush`, `reset`. Also present:
  `TenVadModelConfig` (an alternative VAD family) — note it, evaluate only
  if Silero disappoints.
- Measurement substrate (after plan 001): `just test-integration` runs
  clean + degraded corpus rows (`gain_x0_02`, `gain_x0_005`, `noise_snr10`,
  `noise_snr0`) with per-row WER.
- Silero VAD model file: sherpa-onnx publishes `silero_vad.onnx` in its
  release assets (the sherpa-onnx docs/releases are the source of truth —
  verify the URL and record it in the memo).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Eval matrix | `just test-integration` | all rows pass (baseline) |
| Unit tests | `cargo test -p dictate-speech` | all pass |
| Typecheck | `just check` | exit 0 |

## Scope

**In scope**: scratch prototype code (a feature-gated test, an example, or a
temporary module — executor's choice), and the memo.

**Out of scope**:
- Shipping any VAD code path, settings, or model-catalog entry.
- UI changes ("no speech detected" overlay state) — design notes belong in
  the memo, implementation in a follow-up plan.

## Steps

### Step 1: Acquire the model and stand up the API

Download `silero_vad.onnx` (record exact URL + sha256 in the memo; do not
commit the model). Confirm `VoiceActivityDetector` runs against a fixture
WAV at 16 kHz and yields sensible `SpeechSegment`s (start index + samples).

**Verify**: a scratch `#[test]` (feature-gated or `#[ignore]`d) prints
segment boundaries for `cmu-arctic/arctic_a0001.wav` that bracket the actual
speech (compare by listening or by sample-index sanity: segments cover the
bulk of nonzero energy).

### Step 2: The quiet-speech kill criterion (run this before anything else)

For every fixture, at gains ×1.0, ×0.02, and ×0.005 (plan 001's transforms):
does Silero, at default thresholds, detect speech? Produce a table
(fixture × gain → detected yes/no, % of reference duration retained after
trim). If quiet rows lose speech at defaults, sweep `threshold` downward and
record where detection recovers and whether that setting starts admitting
the `noise_snr0` row's noise as speech.

**This is the go/no-go core**: a VAD configuration that cannot pass the
×0.02 row with ≥95% speech retention means "adopt VAD for trimming" is
rejected for now, whatever its other benefits.

### Step 3: WER impact of trimming

With the surviving configuration: run the plan-001 matrix with a
VAD-trimmed pre-pass (prototype wiring inside the scratch test is fine) and
compare per-row WER against baseline. Also measure decode-time delta on the
longest fixture (trimming should only help; confirm).

### Step 4: Assess the two adoption questions separately

1. **Trim + no-speech outcome**: replace the post-hoc `transcript_is_noise`
   blocklist with "VAD found no speech segments"? Evaluate against the
   blocklist's current catch cases (feed the recognizer junk: the
   `noise_snr0` row *without* underlying speech — pure generated noise — and
   see whether VAD-gating or the blocklist catches it better).
2. **Metrics honesty**: speech-frame RMS (RMS over VAD-retained samples
   only) vs. whole-recording RMS in the `CapturedSignalMetrics` log — how
   different are they on the quiet fixtures? (This is cheap once segments
   exist; it makes future incident logs meaningful.)

### Step 5: Write the memo

`memo-vad-findings.md` in this directory, using the house memo shape
(verdict first, evidence tables, rejected alternatives). Must contain: the
step-2 table, step-3 WER deltas, model asset URL+hash, threshold
recommendation, whether `models.rs` fits a VAD asset (and what changes if
not), a sketch of the follow-up build plan(s) if go, and explicit
open questions for the maintainer (e.g. does "no speech" deserve its own
overlay state).

**Verify**: memo exists; `just test-integration` still passes unmodified
(the spike left no production tracks); scratch code is either deleted or
clearly marked and `#[ignore]`d.

## Done criteria

- [ ] `memo-vad-findings.md` written, verdict-first, with the step-2 table
      and step-3 WER comparison
- [ ] Effort README updated: this plan's row → DONE, reconciliation-log
      entry pointing at the memo
- [ ] `just check` → exit 0 and `just test-integration` passes with no
      production code changed

## STOP conditions

Stop and write a handback if:

- Plan 001 is not yet landed (no measurement substrate — execute 001 first).
- The sherpa-onnx VAD API fails to load/run the published Silero model at
  all (version mismatch) — that changes the cost side of the decision.
- Any step tempts you to modify production code "just a little" to make
  measurement easier — describe the needed seam in the memo instead.

## Maintenance notes

- If the verdict is no-go, the memo's threshold data still matters: it
  documents *why*, so the next person doesn't re-run the spike blind.
- The TEN VAD config in the same crate is the designated second opinion if
  Silero's quiet-speech behavior is the blocker.
