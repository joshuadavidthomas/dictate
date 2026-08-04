# Plan 007: Spike — A/B the sherpa-onnx speech denoiser against raw ASR

> **Executor instructions**: This is a **measurement spike**, not a build
> plan. The deliverable is a short memo with WER numbers and a ship/don't
> recommendation. Prototype code is scratch and expected to be discarded.
> STOP into a handback rather than expanding scope.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate-speech/tests/integration.rs`
> This spike consumes plan 001's matrix; if the row ids or transforms
> changed after 001 landed, use the live ones and note it in the memo.

## Status

- **Effort**: S (given 001)
- **Risk**: LOW (no production changes)
- **Depends on**: 001
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

"Add noise suppression like Teams" is the obvious ask and the likely
mistake: meeting-grade denoisers optimize human intelligibility and can
*hurt* ASR by introducing artifacts the model never trained on — modern
models often handle raw noise better than denoiser output. Dictate should
decide this with numbers, and the numbers are nearly free: the `sherpa-onnx`
crate already ships an offline speech denoiser (GTCRN — crate source
`src/offline_speech_denoiser.rs`), and plan 001 provides noisy corpus rows
with measured baseline WER. One afternoon of A/B either buries the feature
request with evidence or promotes it with evidence.

## Current state

- Measurement substrate (after 001): `just test-integration` runs
  `noise_snr10` and `noise_snr0` rows with per-row WER and committed
  baseline thresholds; `gain_x0_02` covers the quiet-clean case.
- sherpa-onnx 1.13 exposes `offline_speech_denoiser` (GTCRN model family);
  the model asset is published in sherpa-onnx release assets — verify the
  exact URL and record URL + sha256 in the memo; do not commit the model.
- ASR entry point for prototyping: `transcribe(recognizer, utterance)`
  (`crates/dictate-speech/src/transcription.rs:134`) over a
  `CapturedUtterance` — a denoise pre-pass is a `Vec<f32> → Vec<f32>`
  transform inserted in scratch test code, same shape as 001's degradations.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Eval matrix baseline | `just test-integration` | all rows pass |
| Typecheck | `just check` | exit 0 |

## Scope

**In scope**: scratch prototype (feature-gated or `#[ignore]`d test),
`memo-denoiser-ab.md`.

**Out of scope**: any production denoiser wiring, settings, model-catalog
entries, or UI.

## Steps

### Step 1: Stand up the denoiser

Download the GTCRN model (record URL + hash). In scratch code, run the
denoiser over one noisy-row fixture and confirm it returns same-rate audio
of ~equal length.

### Step 2: A/B matrix

For each row — clean, `gain_x0_02`, `noise_snr10`, `noise_snr0` — compute
corpus WER with and without the denoise pre-pass (reuse 001's transforms and
the harness's WER functions). Also record per-fixture wall-clock cost of the
denoise pass (it's an extra ONNX inference; the daemon would pay it per
dictation).

The clean and quiet rows are the guardrail: a denoiser that buys WER on
`noise_snr0` by degrading clean/quiet speech fails the A/B.

### Step 3: Write the memo

`memo-denoiser-ab.md`: verdict first (ship / don't ship / ship-behind-config),
the A/B table, runtime cost, model asset details, and — if shipping — where
it would sit in the pipeline (before/after VAD per plan 006's outcome) as a
follow-up plan sketch.

**Verify**: memo exists; `just test-integration` unmodified and passing; no
production diffs (`jj diff` shows only plans/ and scratch, and scratch is
removed or `#[ignore]`d before finishing).

## Done criteria

- [ ] `memo-denoiser-ab.md` written with the four-row A/B table and runtime cost
- [ ] Effort README row → DONE, reconciliation-log entry pointing at the memo
- [ ] No production code changes; `just check` → exit 0

## STOP conditions

Stop and write a handback if:

- Plan 001 hasn't landed.
- The crate's denoiser API can't load the published GTCRN model (version
  mismatch) — record the incompatibility; that *is* a finding.
- Results are wildly inconsistent across fixtures (>2× WER variance within a
  row) — the corpus may be too small to decide; say so rather than
  averaging it away.

## Maintenance notes

- If the verdict is "don't ship", keep the memo discoverable — this question
  will be asked again every time someone dictates near a fan.
- A "ship-behind-config" verdict interacts with plan 008 (settings surface)
  and plan 006 (pipeline ordering); the follow-up plan should be written
  after both memos exist.
