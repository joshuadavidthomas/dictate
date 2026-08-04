# GTCRN denoiser A/B handback

## Verdict

Do not ship GTCRN from this evidence. The spike hit its inconsistency STOP condition: effects within the same row varied by more than 2× across fixtures, including changes in opposite directions. The small corpus cannot support a general denoising decision.

The aggregate numbers also give no reason to take that risk. GTCRN worsened clean, quiet, and 0 dB SNR WER, left 10 dB SNR unchanged, and added about 200–275 ms of inference per fixture. No production build plan should follow this spike.

## Why the spike stopped

The quiet row contains the clearest counterexample. Denoising changed `cmu-arctic/arctic_a0001.wav` from 2 edits to 8, improved `ljspeech/LJ001-0001.wav` from 1 edit to 0, and improved `spoken-commands/clip-b.wav` from 2 edits to 1. Those fixture effects differ in sign and exceed the plan's 2× variance limit.

The clean row also regressed only `spoken-commands/clip-b.wav`, from 0 edits to 2, while every other clean fixture was unchanged. At 0 dB SNR, the same fixture worsened from 2 edits to 3 while the other fixtures stayed unchanged. Averaging these outcomes would hide that the result depends on the speaker or recording.

This handback records the completed measurements but makes no claim beyond “do not ship from this corpus.” A future spike needs a larger, more varied noisy-speech corpus and should define its variance statistic before running.

## A/B results

The scratch prototype reused Plan 001's deterministic transforms and word-error scoring. It denoised each transformed 16 kHz mono fixture, then passed raw and denoised utterances through the unchanged default recognizer. Aggregate counts cover 116 reference words.

| Row | Raw WER | Denoised WER | Delta | Mean denoise time |
|---|---:|---:|---:|---:|
| clean | 2.59% (3 edits) | 4.31% (5) | +1.72 pp | 197.14 ms |
| gain_x0_02 | 5.17% (6) | 8.62% (10) | +3.45 pp | 274.86 ms |
| noise_snr10 | 2.59% (3) | 2.59% (3) | 0.00 pp | 212.11 ms |
| noise_snr0 | 4.31% (5) | 5.17% (6) | +0.86 pp | 202.44 ms |

Per-fixture edit counts are shown as raw → denoised:

| Fixture | clean | gain_x0_02 | noise_snr10 | noise_snr0 |
|---|---:|---:|---:|---:|
| cmu-arctic/arctic_a0001.wav | 1 → 1 | 2 → 8 | 1 → 1 | 1 → 1 |
| cmu-arctic/arctic_a0002.wav | 1 → 1 | 1 → 1 | 1 → 1 | 1 → 1 |
| cmu-arctic/arctic_a0003.wav | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 |
| cmu-arctic/arctic_a0004.wav | 0 → 0 | 0 → 0 | 0 → 0 | 1 → 1 |
| cmu-arctic/arctic_a0005.wav | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 |
| ljspeech/LJ001-0001.wav | 1 → 1 | 1 → 0 | 0 → 0 | 0 → 0 |
| ljspeech/LJ001-0002.wav | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 |
| spoken-commands/clip-a.wav | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 |
| spoken-commands/clip-b.wav | 0 → 2 | 2 → 1 | 1 → 1 | 2 → 3 |
| spoken-commands/clip-c.wav | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 |

The raw results exactly reproduced Plan 001's aggregate baselines. GTCRN crossed the existing 8% quiet-row guardrail: `gain_x0_02` rose to 8.62%.

## Runtime and output shape

The denoiser reported a 16,000 Hz output rate for every input. Output duration stayed close to input duration but was quantized to model frames: each output was 49–241 samples shorter than its input, or at most 15.1 ms at 16 kHz. This satisfies the stand-up check for same-rate, approximately equal-length audio.

Measured per-fixture denoise times ranged from 59.30 ms to 776.13 ms in an unoptimized development build. Row means ranged from 197.14 ms to 274.86 ms. These figures measure only GTCRN inference and exclude ASR.

## Model asset

- Model: `gtcrn_simple.onnx`
- Official URL: <https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/gtcrn_simple.onnx>
- Size: 535,638 bytes
- SHA-256: `e77603ac0c23dac3227dd2d7135b3a585cbee2679048aecfa886657d3ae1b534`
- Runtime API: sherpa-onnx 1.13.2 `OfflineSpeechDenoiser`
- Model sample rate: 16 kHz

The model stayed under `/tmp` and was not added to the repository. The scratch example was deleted after measurement.

## Pipeline consequence

Plan 006 also returned a no-go verdict, so there is no VAD/denoiser ordering decision to make. GTCRN should not gain a setting, catalog entry, daemon stage, or UI from this spike.

## Reopening this question

Reopen only with a corpus that includes several speakers and noise types per condition. Set a minimum practical WER improvement, a clean/quiet non-regression bound, and a precise fixture-variance rule before measurement. Run optimized-build latency separately if quality first clears those gates.
