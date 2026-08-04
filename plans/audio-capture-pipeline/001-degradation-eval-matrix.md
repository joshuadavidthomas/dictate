# Plan 001: Extend the WER integration harness with a degradation matrix

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from cc3223f80bfb -- crates/dictate-speech/tests/integration.rs crates/dictate-speech/tests/fixtures/README.md`
> If these files changed since planning, compare the "Current state"
> excerpts against the live code before proceeding; on a mismatch, STOP.

## Status

- **Effort**: M
- **Risk**: LOW (adds tests; touches no production code)
- **Depends on**: none
- **Planned at**: revision `nootnkmorwsk` (git `cc3223f80bfb`), 2026-08-04

## Why this matters

A real headset produced valid speech at RMS 0.002141 (−53 dBFS) and Dictate
discarded it (fixed RMS gate, since removed; U8 capture format, since fixed).
Nothing prevents a regression: the corpus WER harness only runs *clean*
fixtures. This plan adds programmatically degraded variants — quiet gain,
added noise — so "quiet speech still transcribes" becomes a number CI can
check, and so later pipeline decisions (VAD thresholds in plan 006, denoiser
A/B in plan 007) have a scoreboard instead of opinions.

## Current state

- `crates/dictate-speech/tests/integration.rs` — model-backed corpus test,
  compiled only with `--features integration` (see `crates/dictate-speech/Cargo.toml:31-33`).
  - `committed_corpus_meets_transcription_thresholds` (~line 92) walks every
    `.wav` under `tests/fixtures/` with a sibling `.txt` reference
    (`discover_transcription_fixtures`), transcribes with the preinstalled
    default model, snapshots raw hypotheses via `insta`, and aggregates
    WER/CER against `MAX_WORD_ERROR_RATE = 0.08` / `MAX_CHARACTER_ERROR_RATE
    = 0.03` (lines 17-18).
  - `locate_preinstalled_default_model` honors `DICTATE_MODEL_DIR` and gives
    actionable errors when the model is missing.
- `crates/dictate-speech/tests/fixtures/README.md` — fixture rules: commit
  only 16 kHz mono WAV + sibling transcript + license, everything recorded in
  `manifest.toml`/`manifest.lock`. **Degraded variants must therefore be
  generated at test time in memory, not committed.**
- `crates/dictate-speech/src/dictation.rs:93-126` — `CapturedUtterance::new(sample_rate, samples)`
  wraps a `Vec<f32>`; `load_wav_utterance` (`src/audio.rs:11`) produces one
  from a fixture path. Degradations are pure `Vec<f32>` → `Vec<f32>`
  transforms applied between load and `transcribe`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Integration tests | `just test-integration` | all pass (needs default model preinstalled or `DICTATE_MODEL_DIR` set — the harness's error message explains how) |
| Unit tests | `cargo test -p dictate-speech` | all pass |
| Typecheck | `just check` | exit 0 |
| Lint | `just lint` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `crates/dictate-speech/tests/integration.rs`

**Out of scope**:
- `crates/dictate-speech/tests/fixtures/**` — no new committed audio; the
  fixture rules forbid derived clips.
- `crates/dictate-speech/src/**` — no production changes in this plan.
- `MAX_WORD_ERROR_RATE` / `MAX_CHARACTER_ERROR_RATE` for the clean corpus —
  leave the existing thresholds and test untouched.

## Steps

### Step 1: Deterministic degradation transforms

In `integration.rs`, add pure helpers, each `fn(&[f32]) -> Vec<f32>`:

- `scale_gain(samples, factor)` — multiply every sample.
- `add_noise(samples, snr_db, seed)` — additive white noise at a target SNR
  relative to the *signal's* measured RMS. Determinism is required and
  `Date`-free: use a tiny inline LCG or xorshift seeded by the `seed`
  argument — do **not** add a `rand` dependency for this.

Sketch:

```rust
fn add_noise(samples: &[f32], snr_db: f64, mut state: u64) -> Vec<f32> {
    let signal_rms = /* sqrt(mean(x^2)) */;
    let noise_rms = signal_rms / 10f64.powf(snr_db / 20.0);
    samples.iter().map(|s| {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let uniform = ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0; // [-1, 1)
        s + (uniform * noise_rms * SQRT_3) as f32 // uniform noise, matched RMS
    }).collect()
}
```

**Verify**: `cargo test -p dictate-speech --features integration --test integration <a unit test for the helpers>` → the transforms are deterministic (same seed → identical output) and hit the target RMS within 5%. Write that unit test (no model needed — it can run without fixtures beyond one loaded WAV).

### Step 2: The degradation matrix test

Add `degraded_corpus_meets_transcription_thresholds` alongside the existing
corpus test, reusing `discover_transcription_fixtures`,
`locate_preinstalled_default_model`, `word_error_rate`, and
`character_error_rate`. Rows:

| Row id | Transform | WER threshold |
|--------|-----------|---------------|
| `gain_x0_02` | gain ×0.02 (the headset case: LJ/arctic speech lands near RMS 0.002) | same as clean: 0.08 |
| `gain_x0_005` | gain ×0.005 | same as clean: 0.08 |
| `noise_snr10` | noise at 10 dB SNR, fixed seed | calibrate in step 3 |
| `noise_snr0` | noise at 0 dB SNR, fixed seed | calibrate in step 3 |

The quiet rows deliberately share the clean threshold — level-invariance is
the exact property this plan locks in. Per-row aggregate WER/CER, and a
failure message that names the failing row(s) with their rates (follow the
existing `corpus_report` formatting). **No insta snapshots for degraded
rows** — thresholds only; snapshots stay a clean-corpus concern.

**Verify**: `just test-integration` → the two gain rows pass against 0.08
before any calibration.

### Step 3: Calibrate the noise-row thresholds

Run the matrix once, note the measured WER for `noise_snr10` and
`noise_snr0`, and set each row's threshold to measured × 1.5 (rounded up to
the nearest percent), with a comment recording the measured baseline and
model id. These rows exist to catch *regressions* and to give plans 006/007
a before/after number — not to assert the model is noise-proof.

**Verify**: `just test-integration` → all rows pass with the committed
thresholds.

## Test plan

This plan is tests. New coverage, all in `integration.rs`:

- Transform unit tests (determinism, RMS targets) — runnable without a model.
- `degraded_corpus_meets_transcription_thresholds` — model-backed, feature-gated
  like its sibling.
- Pattern to match: `committed_corpus_meets_transcription_thresholds` and its
  report helpers.
- **Verify**: `just test-integration` → all pass; `cargo test -p dictate-speech` → all pass.

## Done criteria

- [ ] `just test-integration` → passes, including the new matrix test
- [ ] `just check` → exit 0; `just lint` → exit 0
- [ ] Quiet rows (`gain_x0_02`, `gain_x0_005`) assert the clean thresholds, not looser ones
- [ ] No files outside `crates/dictate-speech/tests/integration.rs` modified

## STOP conditions

Stop and write a handback if:

- The `gain_x0_02` row **fails** its threshold — that means the level-invariance
  assumption behind removing the RMS gate is wrong for the default model, which
  changes the direction of plans 006/007. Report per-fixture rates.
- A noise row measures above 50% WER — the transform is probably wrong
  (check SNR math) or the model degrades far faster than assumed; either way,
  calibrating a threshold around it would be noise, not signal.
- The current-state excerpts don't match the live harness.

On stopping, write a handback: current state, desired outcome, open
questions. Descriptive, not prescriptive.

## Maintenance notes

- Plans 006 (VAD) and 007 (denoiser) consume this matrix as their measurement
  substrate; keep row ids stable so their memos can cite them.
- If the default model changes (see memory: parakeet default decision), the
  noise-row thresholds need recalibration — the baseline comment makes that a
  five-minute job.
- Deliberately deferred: a resampling/aliasing row (plan 002 carries its own
  unit-level aliasing test instead, because the capture resampler lives behind
  a private seam in `mic.rs`).
