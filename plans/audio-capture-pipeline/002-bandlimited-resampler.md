# Plan 002: Replace the fallback resampler with sherpa-onnx's bandlimited resampler

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate-speech/src/mic.rs`
> If `mic.rs` changed since planning, compare the "Current state" excerpts
> against the live code before proceeding; on a mismatch, STOP.

## Status

- **Effort**: S–M
- **Risk**: LOW (fallback path only; the preferred path opens the device at 16 kHz and never resamples in-app)
- **Depends on**: none
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

When the input device offers a config containing 16 kHz, Dictate opens it at
16 kHz and the in-app resampler is a pass-through. When it doesn't, Dictate
opens the device's default config (typically 48 kHz) and downsamples with a
hand-rolled **linear-interpolation** resampler — no anti-alias low-pass, so
all energy above 8 kHz folds down into the speech band. That degrades
recognition on exactly the marginal devices that hit the fallback. The fix is
nearly free: the `sherpa-onnx` crate (already a dependency) ships a
bandlimited streaming resampler (Kaldi's `LinearResample` — windowed-sinc,
despite the name). This swap deletes ~75 lines of DSP we shouldn't own.

## Current state

- `crates/dictate-speech/src/mic.rs:440-515` — `struct LinearResampler` with
  `new(input_rate, output_rate)` and `process_into(&[f32], &mut Vec<f32>)`;
  pure linear interpolation with fractional-position carry across chunks.
  Same-rate short-circuit at `mic.rs:464-467` copies input through untouched.
- `crates/dictate-speech/src/mic.rs:372-416` — `run_audio_worker` owns the
  resampler: constructed at line 382 on the worker thread, `process_into`
  called per ~256-sample batch at line 403, output fed to the sink (ASR +
  spectrum). The worker exits when `consumer.is_abandoned()` (line 395).
- `crates/dictate-speech/src/mic.rs:656-695` — unit tests asserting exact
  linear-interpolation outputs (`resampler_downsamples_linearly`,
  `resampler_keeps_fractional_position_across_buffers`,
  `same_rate_resampler_returns_input`, `same_rate_resampler_supports_rates_above_u16`).
  The exact-value expectations are an artifact of linear interpolation and
  **will not survive** the swap.
- sherpa-onnx 1.13 API (crate source `src/resampler.rs`):
  `LinearResampler::create(samp_rate_in_hz: i32, samp_rate_out_hz: i32) -> Option<Self>`,
  `resample(&self, samples: &[f32], flush: bool) -> Vec<f32>`, `reset(&self)`.
  It is an FFI wrapper; construct and use it on one thread (the worker
  thread already does exactly this).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unit tests | `cargo test -p dictate-speech` | all pass |
| Typecheck | `just check` | exit 0 |
| Lint | `just lint` | exit 0 |
| Headless smoke | `cargo run -p dictate -- transcribe crates/dictate-speech/tests/fixtures/cmu-arctic/arctic_a0001.wav` | non-empty transcript |

## Scope

**In scope** (the only files you should modify):
- `crates/dictate-speech/src/mic.rs`

**Out of scope**:
- `crates/dictate-speech/src/transcription.rs`, `dictation.rs` — untouched by
  a resampler swap.
- `preferred_input_config` / device-format selection (`mic.rs:181-209`) —
  recently fixed; leave alone.
- Adding a WER matrix row for resampling — deliberately deferred (see plan
  001's maintenance notes).

## Steps

### Step 1: Swap the implementation behind the existing seam

Replace the body of the resampling seam so `run_audio_worker` still consumes
a "resampler" with a `process_into(&[f32], &mut Vec<f32>)`-shaped interface
(keep the wrapper struct; delete the interpolation internals):

- Same-rate case: keep the pass-through short-circuit — don't construct the
  FFI resampler at all when `input == output`.
- Different-rate case: hold a `sherpa_onnx::LinearResampler` created on
  first use; `process_into` delegates to `resample(input, false)` and
  extends the output Vec.
- Construction can fail (`create` returns `Option`): surface that as an
  error at capture setup or fall back with a logged warning — pick the
  simplest shape that keeps `run_audio_worker`'s signature workable; a
  `Result` from the wrapper's constructor propagated out of
  `capture_with_config` matches the existing `anyhow` error style
  (`mic.rs:77-115`).
- On worker exit (the `consumer.is_abandoned()` break), flush: one final
  `resample(&[], true)` pushed through the sink, so the filter's tail
  samples aren't dropped at stop. It's ~a few ms of audio; the code is two
  lines, so do it.

**Verify**: `just check` → exit 0.

### Step 2: Rewrite the resampler unit tests as property tests

Delete the exact-value linear-interpolation assertions; keep/replace with:

- Same-rate pass-through returns input exactly (both existing tests survive).
- Output length ratio: feeding N seconds at 48 kHz yields ~N seconds at
  16 kHz (length within a small tolerance of `input_len / 3`, allowing
  filter delay).
- Chunked equivalence: feeding one buffer vs. the same samples in two chunks
  produces identical concatenated output (streaming consistency).
- **Aliasing regression** (the point of the plan): synthesize a pure 12 kHz
  sine at 48 kHz (above the 8 kHz output Nyquist), resample to 16 kHz, and
  assert the output RMS is a small fraction of the input RMS (start at
  < 0.05; calibrate against the actual filter's stopband and comment the
  measured value). The old interpolator folds that tone to 4 kHz at high
  amplitude, so this test fails against the old implementation — state that
  in a test comment.

**Verify**: `cargo test -p dictate-speech` → all pass.

### Step 3: End-to-end smoke

**Verify**: the headless smoke command above → non-empty transcript, and
`cargo test -p dictate-speech` still green. If a microphone is available in
the execution environment, a live `dictate` run is nice-to-have, not a gate.

## Test plan

Covered in step 2 — the property tests replace the interpolation-specific
tests at `mic.rs:656-695`. Structural pattern: the existing `mod tests` in
`mic.rs` (plain `#[test]` fns, no fixtures).

- **Verify**: `cargo test -p dictate-speech` → all pass, including the
  aliasing regression test.

## Done criteria

- [ ] `cargo test -p dictate-speech` → all pass
- [ ] `just check` → exit 0; `just lint` → exit 0
- [ ] The hand-rolled interpolation math (fractional position, `interpolation_fraction`) is gone from `mic.rs`
- [ ] No files outside `crates/dictate-speech/src/mic.rs` modified

## STOP conditions

Stop and write a handback if:

- `sherpa_onnx::LinearResampler` turns out not to be constructible/usable on
  the worker thread (e.g., a `Send`/lifetime constraint the crate imposes) —
  the fallback design would need rethinking, not patching.
- The aliasing test shows the sherpa resampler does **not** attenuate the
  12 kHz tone (measured RMS ratio > 0.2) — the "bandlimited" assumption this
  plan rests on would be false.
- The flush-on-exit interacts badly with the drain path (duplicated or
  reordered samples reaching `record_samples`).
- Current-state excerpts don't match the live code.

## Maintenance notes

- The preferred path (device opened at 16 kHz) still depends on PipeWire's
  resampler quality; that's fine and deliberate — this plan only fixes the
  path Dictate owns.
- Reviewer attention: the flush ordering at worker exit relative to the last
  `sink` call — trailing samples must reach `record_samples` before the
  worker returns (the daemon's drain relies on it).
- If a future plan adds a WER matrix row exercising capture-side resampling,
  it needs a public seam; that was deliberately not added here.
