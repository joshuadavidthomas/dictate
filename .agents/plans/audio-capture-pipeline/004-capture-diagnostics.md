# Plan 004: Capture diagnostics — name the device, persist the audio

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate-speech/src/mic.rs crates/dictate-speech/src/audio.rs crates/dictate-speech/src/lib.rs crates/dictate/src/daemon.rs`
> Plan 002 intentionally modifies `mic.rs` first and plan 005 modifies
> `daemon.rs` — read the live code, not just the excerpts. If the structure
> differs beyond those plans' changes, STOP.

## Status

- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (recommended after 002 to avoid `mic.rs` churn)
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

Diagnosing the 2026-08 "quiet headset discarded" incident required a live
debugging session because Dictate can answer neither of the two questions
that matter after a bad dictation: *which device did you record from* (the
capture log omits the device name — and "dock vs. headset" was the actual
question) and *what did the audio contain* (captured utterances are never
persisted anywhere). After this plan: the log names the device, and setting
one environment variable makes the daemon write each captured utterance as a
WAV that `dictate transcribe` can replay — turning any future capture bug
into a five-minute reproduction.

## Current state

- `crates/dictate-speech/src/mic.rs:131-137` — the capture log line prints
  rate/channels/format/buffer but not the device:

  ```rust
  eprintln!(
      "capturing microphone audio at {}Hz, {} channel(s), {}, {:?} buffer", ...
  ```

  `capture_with_config` receives `device: &Device`; cpal's
  `DeviceTrait::name()` returns `Result<String>`.
- `crates/dictate-speech/src/audio.rs:11-83` — `load_wav_utterance(path) ->
  Result<CapturedUtterance>` via `hound`; 16 kHz mono is the enforced format
  (`DICTATION_SAMPLE_RATE`). There is **no inverse** (no WAV writer outside
  test helpers — see `write_i16_wav` in `audio.rs` tests, line 112, for the
  hound-writer shape).
- `crates/dictate/src/daemon.rs:623-634` — `run_microphone_worker` obtains
  the finished `ready_dictation` and (working-copy change) logs
  `CapturedSignalMetrics` before calling `transcribe`. This is the one point
  that sees every utterance — including ones that will fail transcription,
  which are exactly the ones worth saving.
- Crate responsibilities (`AGENTS.md`): WAV I/O belongs in `dictate-speech`;
  the `dictate` binary orchestrates. `hound` is a workspace dep used only by
  `dictate-speech`.
- CLI replay already exists: `dictate transcribe <wav> [--raw]`
  (`crates/dictate/src/cli.rs`, `Command::Transcribe`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unit tests | `cargo test -p dictate-speech` | all pass |
| Typecheck | `just check` | exit 0 |
| Lint | `just lint` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `crates/dictate-speech/src/mic.rs` (log line only)
- `crates/dictate-speech/src/audio.rs` (WAV writer)
- `crates/dictate-speech/src/lib.rs` (export)
- `crates/dictate/src/daemon.rs` (env-gated save call)

**Out of scope**:
- `crates/dictate/src/settings.rs` — this is a debug affordance, not user
  configuration; an env var is deliberate (see Maintenance notes).
- `crates/dictate/src/cli.rs` — replay already exists.

## Steps

### Step 1: Device name in the capture log

Extend the `eprintln!` at `mic.rs:131-137` to lead with the device name:
`device.name().unwrap_or_else(|_| "<unknown device>".into())`. Thread the
`&Device` in if it isn't already in scope at the log site (it is —
`capture_with_config` takes it).

**Verify**: `just check` → exit 0.

### Step 2: `save_wav_utterance` in `audio.rs`

Add the inverse of `load_wav_utterance`:

```rust
pub fn save_wav_utterance(path: &Path, utterance: &CapturedUtterance) -> Result<()>
```

16-bit int mono at the utterance's sample rate, via `hound::WavWriter`
(shape: the `write_i16_wav` test helper at `audio.rs:112-124`). Clamp
samples to [-1.0, 1.0] before i16 conversion. Export from
`crates/dictate-speech/src/lib.rs` next to `load_wav_utterance`.

**Verify**: `cargo test -p dictate-speech` → all pass, including a new
round-trip test (see Test plan).

### Step 3: Env-gated persistence in the daemon

In `run_microphone_worker`, immediately after the metrics log
(`daemon.rs:628-634`) and **before** `transcribe` (so failed transcriptions
are still saved): if `DICTATE_CAPTURE_DIR` is set, write
`<dir>/capture-<NNN>.wav` (recording id or a monotonic counter — pick what's
reachable; `ready_dictation` carries no id, a worker-local counter is fine),
creating the directory if needed. Log the written path, or a warning on
failure — a diagnostics write must never abort the dictation (no `?` into
the transcription flow).

**Verify**: `just check` → exit 0, then a manual smoke if a mic is
available: `DICTATE_CAPTURE_DIR=/tmp/dictate-captures cargo run -p dictate`
+ one dictation → file exists and
`cargo run -p dictate -- transcribe /tmp/dictate-captures/capture-1.wav`
prints a transcript. If no mic is available in the execution environment,
state that in the completion summary; the unit round-trip is the gate.

## Test plan

- In `audio.rs` tests: round-trip — build a `CapturedUtterance` (pattern:
  `transcription.rs` tests' `test_utterance`), `save_wav_utterance` to a
  temp path (pattern: existing `temp_wav_path` helper, `audio.rs:102`),
  `load_wav_utterance` back, assert sample count and rate match and samples
  match within i16 quantization (±1/32768).
- Clamp case: samples outside [-1, 1] save without panicking and load back
  clamped.
- **Verify**: `cargo test -p dictate-speech` → all pass.

## Done criteria

- [ ] `cargo test -p dictate-speech` → all pass, including round-trip tests
- [ ] `just check` → exit 0; `just lint` → exit 0
- [ ] Capture log line includes the device name
- [ ] `DICTATE_CAPTURE_DIR` unset → daemon behavior byte-identical to before
      (no writes, no new log lines)
- [ ] No files outside the in-scope list modified

## STOP conditions

Stop and write a handback if:

- `daemon.rs` no longer has a single point that sees every utterance
  pre-transcription (plan 005 or other drift restructured the worker loop).
- Saving requires information (recording id) that would mean widening a
  `dictate-speech` public interface beyond `save_wav_utterance` — that's a
  crate-responsibility question, not an implementation detail.
- Current-state excerpts don't match the live code beyond plans 002/005's
  documented changes.

## Maintenance notes

- Env var over settings key is deliberate: settings are user contract
  (`deny_unknown_fields`, documented); this is a debug tap that should stay
  invisible in normal operation. If it ever graduates to a feature ("review
  my dictations"), that's a new plan with retention/privacy decisions.
- The capture dir grows without bound while the var is set — acceptable for
  a debugging session, worth a warning line in the log when enabled.
- Saved WAVs are post-downmix/post-resample (what the recognizer sees), not
  the raw device stream — that's the right tap for "what did ASR hear", but
  a reviewer should know it can't diagnose device-side format bugs.
