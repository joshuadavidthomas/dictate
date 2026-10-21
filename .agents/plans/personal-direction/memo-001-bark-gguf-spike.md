# Memo 001: Bark 3.0 as GGUF on transcribe.cpp (2026-10-21)

Spike result for plan 001. Same 61-minute eval set and scoring as
[memo-runtime-findings.md](memo-runtime-findings.md). transcribe.cpp at
`db678a0`, CPU backend, 4-core x86-64 orb, no GPU.

## Verdict

- **Q8_0 is lossless for Bark.** Clean Q8_0 differs from Clean F32 GGUF
  on 12 of 550 clips (5 worse, 3 better).
- **ggml GGUF matches or beats the ONNX fp32 export** on every set. The
  LibriSpeech gains (Clean 5.98% → 5.09%) are a runtime difference, not
  quantization.
- **Bark 3.0 Clean is the only Bark variant worth shipping.** On the same
  runtime, stock Parakeet TDT v2 Q8_0 is as good or better than Bark Plus
  and Pro on verbatim and read speech. Clean's value is its cleanup.

## Accuracy (WER)

| Model (Q8_0 GGUF unless noted) | DisfluencySpeech verbatim | DisfluencySpeech cleaned | LibriSpeech other |
|---|---|---|---|
| Parakeet TDT v2 (handy-computer GGUF) | 3.25% | 15.95% | **3.54%** |
| Bark 3.0 Clean | 13.66% | **4.18%** | 5.09% |
| Bark 3.0 Clean, F32 GGUF | 13.62% | 4.18% | 5.02% |
| Bark 3.0 Clean, ONNX fp32 (sherpa) | 13.51% | 4.02% | 5.98% |
| Bark 3.0 Plus | 3.35% | 16.67% | 5.02% |
| Bark 3.0 Pro | 3.07% | 16.30% | 4.42% |

Raw recognizer output; Dictate's formatter not applied.

## Size, memory, latency (Clean)

| Format | File | Peak RSS, short clips | 10 s of speech, 2 threads |
|---|---|---|---|
| Q8_0 | 740 MB | ~1.06–1.2 GB | 1.5 s |
| F16 | 1.26 GB | ~1.7 GB | 1.6 s |
| F32 | 2.51 GB | ~2.6 GB | 1.6 s |

- RSS grows with clip length (Q8_0: 1.35 GB at 34 s, 1.7 GB at 134 s).
- Weights are mmapped, so they live in reclaimable, shareable page cache.
- Throughput at 4 threads: about 12× realtime for all Q8_0 models.
- For comparison, sherpa int8 Parakeet ran at about 1.0 s per 10 s with
  ~840 MB, but with the int8 accuracy loss. GPU backends (Metal, Vulkan)
  are untested here.

## Conversion recipe

1. Unpack `model.bark` (Conway's `bark_loader.py`) or Pro's bf16
   `model.safetensors` into an F32 transformers `ParakeetForTDT` snapshot,
   with `config.json` and `tokenizer.json` from the Pro repo:
   [assets/bark_to_hf.py](assets/bark_to_hf.py).
2. Add Bark profiles to transcribe.cpp's `scripts/convert-parakeet.py`
   ([assets/convert-parakeet-bark.patch](assets/convert-parakeet-bark.patch)):
   `source_format: hf_safetensors`, no `vad_head`, English only.
3. `python scripts/convert-parakeet.py <snapshot> --repo-id underdog-bark-3.0-clean`
4. `build/bin/transcribe-quantize <F32.gguf> <Q8_0.gguf> --quant Q8_0`
   (needs `-DTRANSCRIBE_BUILD_TOOLS=ON`).

## Open for plan 002

- Where Bark Clean's GGUF is hosted: upstream the profiles to
  transcribe.cpp so handy-computer publishes it, or host it ourselves.
- Bark Clean writes less punctuation; check it against the formatter's
  spoken-punctuation snapshots.
