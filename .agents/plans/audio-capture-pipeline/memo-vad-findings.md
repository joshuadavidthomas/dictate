# Silero VAD findings

## Verdict

Do not add Silero VAD to the transcription path.

The effective default threshold (`0.5`) rejects or truncates quiet speech. Lowering the threshold to `0.05` lets every ×0.02 fixture retain at least 95% of its samples, but VAD trimming still doubles aggregate ×0.02 WER from 5.17% to 10.34% and raises ×0.005 WER from 7.76% to 25.86%. At that threshold, generated broadband noise is classified as speech, so Silero cannot act as a reliable pre-decode no-speech gate. The experiment did not produce a nonempty blocked transcript, so replacing the post-decode string blocklist remains untested. Speech-only RMS differs too little from whole-utterance RMS on these fixtures to justify adding the model for metrics alone.

Do not write a follow-up build plan. Keep the current ASR path and `transcript_is_noise` behavior. If future evidence reopens VAD work, evaluate TEN VAD against this memo before tuning Silero again.

## Prototype and model

The scratch prototype used sherpa-onnx 1.13.2's `VoiceActivityDetector`, fed normalized mono 16 kHz samples in 512-sample chunks and flushed after the final chunk. It was deleted after measurement; no production code remains.

The prototype set only the model path, threshold, and sample rate in Rust. sherpa-onnx's native boundary supplied the effective values for the zero-valued Rust defaults:

| Setting | Value |
|---|---:|
| Sample rate | 16,000 Hz |
| Silero model window size | 512 samples |
| Input feed chunk | 512 samples |
| Minimum silence | 0.5 s |
| Minimum speech | 0.25 s |
| Maximum speech | 20 s |
| Detector buffer | 600 s |
| Threads | 1 |
| Provider | CPU |
| Threshold | varied: 0.50, 0.35, 0.20, 0.10, 0.05 |

The 600-second detector buffer was chosen to exceed the longest fixture; it does not trim or pad emitted segments.

Model asset:

- URL: <https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx>
- SHA-256: `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6`
- Downloaded size: 628.7 kB

On `cmu-arctic/arctic_a0001.wav`, the default configuration returned one segment starting at sample 1,120 with 50,592 samples. That segment covers 97.74% of the fixture and brackets its speech as expected.

## Quiet-speech detection

Each cell reports `detected / percent of fixture samples retained`. The selected low threshold is included beside the effective default because it is the first tested threshold that met the plan's ×0.02 retention rule for every fixture.

| Fixture | Gain | Threshold 0.5 | Threshold 0.05 |
|---|---:|---:|---:|
| cmu-arctic/arctic_a0001 | 1.0 | yes / 97.74% | yes / 98.73% |
|  | 0.02 | yes / 77.96% | yes / 98.73% |
|  | 0.005 | yes / 33.45% | yes / 92.80% |
| cmu-arctic/arctic_a0002 | 1.0 | yes / 97.48% | yes / 97.48% |
|  | 0.02 | yes / 62.40% | yes / 97.48% |
|  | 0.005 | yes / 16.89% | yes / 79.47% |
| cmu-arctic/arctic_a0003 | 1.0 | yes / 96.82% | yes / 97.77% |
|  | 0.02 | yes / 86.36% | yes / 96.82% |
|  | 0.005 | yes / 38.81% | yes / 96.82% |
| cmu-arctic/arctic_a0004 | 1.0 | yes / 95.45% | yes / 97.94% |
|  | 0.02 | yes / 48.23% | yes / 96.70% |
|  | 0.005 | no / 0.00% | yes / 96.70% |
| cmu-arctic/arctic_a0005 | 1.0 | yes / 93.43% | yes / 95.90% |
|  | 0.02 | yes / 34.13% | yes / 95.90% |
|  | 0.005 | yes / 31.66% | yes / 95.90% |
| ljspeech/LJ001-0001 | 1.0 | yes / 97.71% | yes / 99.37% |
|  | 0.02 | yes / 97.05% | yes / 97.05% |
|  | 0.005 | yes / 91.08% | yes / 97.05% |
| ljspeech/LJ001-0002 | 1.0 | yes / 99.08% | yes / 99.39% |
|  | 0.02 | yes / 92.34% | yes / 99.39% |
|  | 0.005 | no / 0.00% | yes / 99.39% |
| spoken-commands/clip-a | 1.0 | yes / 99.61% | yes / 99.69% |
|  | 0.02 | yes / 98.82% | yes / 99.69% |
|  | 0.005 | yes / 97.23% | yes / 99.69% |
| spoken-commands/clip-b | 1.0 | yes / 97.31% | yes / 99.44% |
|  | 0.02 | yes / 90.64% | yes / 99.31% |
|  | 0.005 | yes / 39.25% | yes / 90.64% |
| spoken-commands/clip-c | 1.0 | yes / 97.63% | yes / 99.56% |
|  | 0.02 | yes / 94.59% | yes / 99.56% |
|  | 0.005 | yes / 46.63% | yes / 94.59% |

Threshold sweep summary:

| Threshold | Lowest ×0.02 retention | Quiet misses | Result |
|---:|---:|---:|---|
| 0.50 | 34.13% | 2 at ×0.005 | Reject |
| 0.35 | 44.01% | 1 at ×0.005 | Reject |
| 0.20 | 53.90% | 0 | Reject |
| 0.10 | 92.80% | 0 | Reject |
| 0.05 | 95.90% | 0 | Continue to WER test |

A threshold of `0.05` is the only candidate from this sweep. It is a measurement floor, not a shipping recommendation.

## WER after trimming

The prototype concatenated Silero's retained segments before passing them to the unchanged recognizer. Results are aggregate edit counts over 116 reference words.

| Corpus row | Raw WER | VAD-trimmed WER | Delta |
|---|---:|---:|---:|
| Clean | 2.59% (3 edits) | 2.59% (3) | 0.00 pp |
| gain_x0_02 | 5.17% (6) | 10.34% (12) | +5.17 pp |
| gain_x0_005 | 7.76% (9) | 25.86% (30) | +18.10 pp |
| noise_snr10 | 2.59% (3) | 2.59% (3) | 0.00 pp |
| noise_snr0 | 4.31% (5) | 3.45% (4) | −0.86 pp |

The ×0.02 result fails the existing 8% degradation-row limit despite meeting the sample-retention rule. Tiny boundary cuts can alter recognition even when retained duration looks safe. The ×0.005 result is worse.

For the longest fixture, `ljspeech/LJ001-0001.wav`, one clean run fell from 1,010 ms raw decode time to 895 ms after trimming, an 11.4% reduction. That single-run saving does not offset the quiet-speech regression.

## No-speech outcome

At threshold `0.05`, deterministic broadband noise at the RMS of the longest clean fixture produced a VAD segment covering 37.91% of the recording. The unchanged recognizer returned `NoTranscript(Empty)`.

Silero therefore cannot prevent this noise from reaching the expensive decode step at the threshold required for quiet speech, while a higher threshold would repeat the quiet-speech loss that prompted this effort.

This input does not settle whether Silero could replace `transcript_is_noise`: the recognizer returned an empty result before the string blocklist ran. The spike produced no nonempty junk transcript matching the blocklist, so the replacement comparison is inconclusive. Keep the current blocklist because the experiment supplied no evidence that removing it is safe.

## Signal metrics

At gain ×0.02, whole-utterance RMS and VAD-retained RMS were close:

| Fixture | Whole RMS | Retained RMS | Difference |
|---|---:|---:|---:|
| cmu-arctic/arctic_a0001 | 0.00159770 | 0.00160794 | +0.64% |
| cmu-arctic/arctic_a0002 | 0.00178063 | 0.00180340 | +1.28% |
| cmu-arctic/arctic_a0003 | 0.00179768 | 0.00182696 | +1.63% |
| cmu-arctic/arctic_a0004 | 0.00249612 | 0.00253838 | +1.69% |
| cmu-arctic/arctic_a0005 | 0.00193706 | 0.00197801 | +2.11% |
| ljspeech/LJ001-0001 | 0.00191256 | 0.00189165 | −1.09% |
| ljspeech/LJ001-0002 | 0.00165736 | 0.00166243 | +0.31% |
| spoken-commands/clip-a | 0.00261663 | 0.00262076 | +0.16% |
| spoken-commands/clip-b | 0.00213808 | 0.00214542 | +0.34% |
| spoken-commands/clip-c | 0.00197862 | 0.00198294 | +0.22% |

The maximum observed change was 2.11%. Whole-recording RMS already describes these tightly cropped fixtures well enough. Adding a model dependency only for speech-frame metrics would cost more than the added diagnostic value.

## Model catalog fit

`models.rs` assumes each catalog item is a compressed ASR archive that extracts into its own directory and then creates an `OfflineRecognizer`. Silero is one ONNX file used by a different constructor. It does not fit `ModelCatalogEntry` without weakening that type or adding conditionals for two unrelated asset shapes.

If VAD is revisited, add a separate typed asset descriptor and share only the low-level download and response-length verification code. Add checksum verification if the new asset contract requires it; the current downloader does not verify hashes. Do not add a fake ASR catalog entry or teach `ModelCatalogEntry` two meanings.

## Rejected alternatives

- **Ship the default threshold:** it badly truncates quiet speech and entirely misses two ×0.005 fixtures.
- **Ship threshold 0.05 for trimming:** it passes the duration criterion but fails WER on quiet rows.
- **Use threshold 0.05 only as a pre-decode no-speech gate:** it marks generated broadband noise as speech.
- **Use Silero only for speech-frame RMS:** measured RMS changes are too small to justify another runtime model.
- **Try TEN VAD in this spike:** Silero failed on both trimming quality and noise rejection, and the plan names TEN as a future second opinion rather than part of the required evaluation.

## Open questions

None block the no-go verdict. If later recordings show long leading or trailing silence causing material decode cost, the maintainer should decide whether that new evidence warrants a TEN VAD spike. A distinct “no speech” overlay state should wait for a reliable pre-decode speech detector.
