# Memo: Bark 3.0 on the sherpa-onnx / onnxruntime stack (2026-10-21)

Findings from evaluating Underdog Bark 3.0 in Dictate. They are the
evidence behind the runtime decision in [README.md](README.md). All runs
were on a 4-core x86-64 orb with no GPU.

## What Bark 3.0 is

- Every Bark model is `nvidia/parakeet-tdt-0.6b-v3` with post-trained
  weights: the same 0.6B FastConformer encoder, TDT decoder, tokenizer,
  and config. The lineage is NVIDIA → `moondream/parakeet-ultra` →
  Conway Research.
- Variants: **Clean** (Plus with filler and false-start removal), **3.0**
  (verbatim, 163 MB), **Plus** (verbatim, 180 MB), and **Pro** (bf16
  safetensors, 1.25 GB). All are English only, CC-BY-4.0, and need the
  `NOTICE` attribution.
- `model.bark` (format `bark-levels-v3`): encoder linear weights store
  **K=2 levels per row** (260 of 264 tensors; 4 have K=3), so each row
  holds only the values {0, ±a, ±b}, at about 2.2 bits per weight after
  ANS entropy coding with `constriction`. Other matrices are 5–6-bit
  per-row integers; small tensors are fp16. It unpacks to float exactly.
  Conway's card says 1.25 GB in memory once unpacked.
- Conversion to ONNX worked by loading the unpacked weights into the stock
  NeMo Parakeet v3 model (a renaming map only, with no missing,
  unexpected, or mis-shaped tensors) and running sherpa-onnx's
  `scripts/nemo/parakeet-tdt-0.6b-v3/export_onnx.py` steps. Encoder export
  needed about 12 GB of memory (swap). The conversion script is in each
  (now private) Hugging Face repo as `convert.py`.
- sherpa-onnx gotcha: it enables TDT decoding only if the encoder's `url`
  metadata contains `tdt`. Otherwise loading fails with
  `vocab_size: 8193 != output_size: 8198`.

## Eval set

- 250 DisfluencySpeech test clips (Apache-2.0), scored against both the
  verbatim annotation (fillers and fragments kept) and the cleaned
  `transcript_c`.
- 300 LibriSpeech test-other utterances.
- 61 minutes total. WER uses lowercase, punctuation-stripped word edit
  distance. Decoded with sherpa-onnx 1.13.7, the same version Dictate
  links. None of it is committed; it is fetched from the HF
  datasets-server.

## Results

Speed column = realtime factor with 4 threads.

| Model / format | DisfluencySpeech verbatim | DisfluencySpeech cleaned | LibriSpeech other | Speed |
|---|---|---|---|---|
| Pro fp32 | 3.27% | 16.50% | 5.56% | 9.4× |
| Pro fp16 | 3.27% | 16.50% | 5.56% | 10.2× |
| Pro int8 | 3.64% | 16.73% | 6.18% | 7.9× |
| Plus fp32 | 3.31% | 16.57% | 5.93% | 9.3× |
| Plus fp16 | 3.31% | 16.57% | 5.93% | 10.0× |
| Plus int8 (per-tensor dynamic) | 4.41% | 17.42% | 6.81% | 11.8× |
| Plus int8 (per-channel dynamic) | 4.42% | 17.73% | 8.18% | 10.0× |
| Plus weight-only int8 (`MatMulNBits`, 8-bit, block 32) | 3.35% | 16.63% | 5.91% | 3.5× |
| **Clean fp32** | 13.51% | **4.02%** | 5.98% | 9.4× |
| Clean fp16 | 13.51% | 4.02% | 5.98% | 10.4× |
| Clean int8 | 16.05% | 6.46% | 6.88% | 11.6× |

- **fp16 is identical to fp32:** 0 of 550 clips differ, for all three
  models.
- **Dynamic int8 loses accuracy for every model.** Comparing per clip
  with fp32, int8 Clean was worse on 143 clips and better on 52; Plus
  122/42; Pro 84/45.
- **The cause is activation rounding, not double weight rounding.**
  Per-channel int8, which keeps Bark's weights almost exactly, was no
  better. Weight-only int8, with fp32 activations, matched fp32 (15 of
  550 clips differ, 4 worse and 3 better).
- **The fp32 conversion is faithful:** Clean fp32 scored 4.02% against
  cleaned DisfluencySpeech; Conway publishes 4.08% on the same set.
- Adding 0.25 s of leading silence (Conway's runners do this for non-Clean
  models) made every model the same or slightly worse.

## Memory and latency (Plus)

| Format | Model files | Steady RAM | 10 s of speech, 2 threads (Dictate's setting) |
|---|---|---|---|
| int8 (dynamic) | 640 MB | ~840 MB | 1.0 s |
| weight-only int8 | ~950 MB (conv layers stay fp32) | ~1.09 GB | 3.4 s |
| fp16 storage | 1.25 GB | ~2.7 GB (onnxruntime converts to fp32 on load; peak 3.7 GB) | — |
| fp32 | 2.4 GB | ~2.5 GB | 1.2 s |

## Small earlier comparison (via `dictate transcribe`)

On the repo fixtures (115 words) plus 30 filler-heavy clips, int8,
measured through Dictate's formatter:

| Model | Fixture WER | Disfluent clips vs cleaned, after formatter | um/uh left |
|---|---|---|---|
| Parakeet TDT v2 (current default) | 2.61% | 12.82% | 45 |
| Bark 3.0 Clean | 4.35% | 6.86% | 0 |

Bark Clean also writes less punctuation and spelled out "etcetera" for
"etc.". Its interaction with the spoken-punctuation formatter snapshots
in `text.rs` is unchecked.

## Implications

- On onnxruntime CPU, no format is lossless *and* small *and* fast for
  Bark. That is why the direction moves to ggml/`transcribe-cpp`:
  quantized weights stay quantized in memory, activations are rounded per
  32-value block, and it has Metal and Vulkan backends.
- Open item for plan 001: ggml's Q8_0 on Bark is unmeasured. Use this
  memo's fp32 rows as the baseline; Q8_0 should be within noise of them.
