# Foreground and speaker probe findings

## Verdict

Continue the native-stereo evidence under Plan 009. Land anonymous attribution separately under Plan 011, and leave optional identity comparison deferred under Plan 012. The laptop exposes distinct stereo signals, CAM++ separated isolated user and television speech in this session, and the valid mixed-dialogue clip transcribed only the nearby user. Do not change production downmixing or choose a speaker threshold from this evidence alone.

Reject the int8 Pyannote segmentation model for now. It detected the user but returned no speech for the TV-only clip that the recognizer transcribed as dialogue. The full model detected both sources and produced plausible anonymous clusters.

## Controlled recordings

All clips were recorded from PipeWire node `alsa_input.pci-0000_07_00.6.HiFi__Mic1__source` as 48 kHz stereo float WAV files. The clips remain under `/tmp/dictate-channel-probe/` and are not repository fixtures.

| Clip | Condition | Duration |
|---|---|---:|
| `tv-only.wav` | Television dialogue; user silent | 10 s |
| `user-only.wav` | Television paused; user read the test phrase | 10 s |
| `mixed-dialogue.wav` | Clear television dialogue plus nearby user | 10 s |

An earlier `mixed.wav` clip was excluded because the television had no dialogue during that section of the movie.

## Channel evidence

| Clip | Left RMS | Right RMS | Difference / mean RMS | Zero-lag correlation | Best lag |
|---|---:|---:|---:|---:|---:|
| TV only | 0.03356 | 0.03410 | 18.17% | 0.98361 | −2 samples (−41.7 µs) |
| User only | 0.03510 | 0.03498 | 11.70% | 0.99316 | 0 samples |
| Mixed dialogue | 0.03539 | 0.03484 | 19.76% | 0.98059 | 0 samples |

The channels are not duplicates. The centered user arrived at zero relative delay while the off-axis TV-only signal had a two-sample best lag. This clears the evidence gate for later spatial experiments. It does not prove a spatial processor will improve ASR.

## ASR outcomes

The stereo clips were averaged to mono and resampled to 16 kHz, matching Dictate's current capture behavior.

| Clip | Raw transcript outcome |
|---|---|
| TV only | Television dialogue: `Yes, sir. What? Oh, I know...` |
| User only | Correct test phrase |
| Mixed dialogue | `This is Josh speaking over the TV. One, two, three, four.` |

The individual left and right channels also produced the correct nearby-user transcript for the mixed clip. The current average did not need spatial processing to recover the user in this sample. The demonstrated defect is TV-only audio becoming a dictation when the user does not speak.

## CAM++ speaker embeddings

Model: `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx`

- Downloaded size: 29,596,978 bytes
- SHA-256: `357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b`
- Embedding dimension: 512
- Runtime: Dictate's exact sherpa-onnx 1.13.2

| Pair | Cosine similarity |
|---|---:|
| User only ↔ TV only | 0.249951 |
| User only ↔ mixed dialogue | 0.774589 |
| TV only ↔ mixed dialogue | 0.310891 |

The mixed clip matched the user much more strongly than the TV. This supports further local profile work but cannot establish a shipping threshold.

The committed fixture sweep showed why several enrollment samples and a larger human-speaker corpus are required:

- Averaged CMU ARCTIC profile: own clips 0.795–0.869; strongest non-CMU clip 0.661.
- Averaged LJ Speech profile: own clips 0.913–0.938; strongest non-LJ clip 0.568.
- A profile built from three Piper clips collided with one unrelated CMU clip at approximately 0.768. The Piper set spans two synthesis models and is not a reliable human speaker-verification corpus.
- Synthetic overlap changed profile scores with source ratio. Whole-utterance verification cannot safely select or remove speakers during overlap.

## Diarization

Models:

- Full Pyannote segmentation 3.0 ONNX
- Int8 Pyannote segmentation 3.0 ONNX
- CAM++ embeddings above
- sherpa-onnx offline diarization with automatic speaker count

The int8 segmentation model returned zero segments for TV-only dialogue and one user segment for the user-only clip. That is too large a behavioral change to adopt from size alone.

The full model returned:

- TV only: one speaker segment covering 0.031–9.953 s.
- User only: one speaker segment covering 2.545–9.953 s.
- Mixed dialogue: a short anonymous segment at 0.031–0.689 s and a second segment at 3.035–9.025 s.
- A synthetic TV → user → same-TV concatenation: several clusters, including one cluster reused at the beginning and end. The television clip contains several actors, so more than two clusters is plausible rather than clear model failure.

## Timestamp and attribution probe

The current Parakeet model returned token text, start times, and durations for the TV-only, user-only, mixed-dialogue, and alternating-speaker clips. Concatenated token text reconstructed each raw transcript after trimming. The timestamps were finite, monotonic, and within the source duration.

The dev-only command below now runs ASR, full Pyannote diarization, CAM++ clustering, token attribution, and optional profile comparison through `dictate-speech::SpeakerAnalyzer`:

```console
dictate speaker-probe <wav> \
  --segmentation-model <pyannote-model.onnx> \
  --embedding-model <campplus-model.onnx> \
  [--profile-wav <wav>]
```

Its JSON keeps the raw transcript and every recognized token. Each token is marked as one anonymous speaker, unknown, ambiguous at a diarization boundary, or overlapping only when every candidate shares one common diarized interval. Invalid or absent ASR intervals stop attribution instead of fabricating alignment. Model, diarization, attribution, and profile failures still emit the raw text with typed failure status; the command exits nonzero after printing that report. Profile embeddings use only a speaker's exclusive segment interiors; competing regions and 200 ms around every target and competing boundary are omitted.

For the current mixed-dialogue clip, the analyzer returned the complete nearby-user transcript. The final punctuation fell just outside the diarization segment and was marked ambiguous rather than dropped. Profile and target embeddings now use only diarized single-speaker interiors. Against the earlier user-only profile, the short television segment scored `0.234505` and the longer nearby-user-plus-TV segment scored `0.472917`. Against the newer near-normal profile, they scored `0.280392` and `0.416075`. The low score for a segment containing the user and overlapping TV confirms that diarization cannot make a clean identity embedding from simultaneous speakers. It cannot support a production threshold or deletion rule.

## Same-speaker level and distance probe

A second consented set recorded the same phrase from the current laptop microphone with the television off. Each source is a 10-second, 48 kHz stereo float WAV under `/tmp/dictate-speaker-corpus/`; analysis used the normal 16 kHz mono conversion. `/tmp/dictate-speaker-corpus/results.json` retains the local machine-readable measurements. The first and last second contained capture-boundary transients, so the level figures below cover seconds 1–9.

| Condition | Mean level | Raw transcript | Similarity to near-normal profile |
|---|---:|---|---:|
| Near, normal voice | −40.0 dBFS | Full phrase; `test` decoded as `Text` | `1.000000` |
| Near, quiet voice | −47.3 dBFS | Main sentence retained; final count omitted | `0.924515` |
| Far, normal voice | −51.3 dBFS | Full phrase; `test` decoded as `text` | `0.883380` |

The earlier user-only recording scored `0.816180` against the new near-normal sample after both profile inputs were diarized and trimmed. TV-only dialogue scored `0.360202`. This session shows useful isolated-speech separation and same-speaker stability across an 11 dB range. It still covers one person, one current device, and one room. The mixed-dialogue score collapse above blocks a threshold despite the clean-speech results.

The initial strict interval validator rejected the near-normal clip because Parakeet ended with two zero-duration tokens at the final timestamp. The validator now derives positive intervals from a sibling token, the next timestamp, or the remaining source duration. It still rejects non-finite, negative, reversed, and out-of-source timing. The model-backed integration test still proves raw-text and token-start parity. All three new clips produced complete attribution after this correction.

A model-backed integration test now proves that the installed default Parakeet model returns a complete monotonic timeline for a committed fixture and that timestamp recognition produces the same raw text as ordinary WAV transcription. Unit tests cover unknown tokens, uncertain boundaries, real overlap, sequential speakers crossed by one token, malformed timestamp metadata, exclusive profile extraction, embedding compatibility, and the JSON shape.

## Distribution and CPU audit

Exact files currently used by the probe:

| Role | File | Size | SHA-256 | Declared license |
|---|---|---:|---|---|
| Speaker embedding | `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx` | 29,596,978 bytes | `357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b` | Apache-2.0 through ModelScope 3D-Speaker |
| Segmentation | `sherpa-onnx-pyannote-segmentation-3-0/model.onnx` | 5,992,913 bytes | `220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079` | MIT |

The combined ONNX payload is 35,589,891 bytes. `dictate-speech` records this pair as the typed model ID `pyannote-segmentation-3.0-campplus-voxceleb-16k`. The dev probe verifies both lengths and hashes before loading either file, so the rejected int8 segmentation file or changed bytes cannot enter this analysis path by accident. Embeddings carry the CAM++ file digest as their compatibility identity rather than a filesystem path.

CAM++ was published in sherpa-onnx's `speaker-recongition-models` release and maps to ModelScope model `iic/speech_campplus_sv_en_voxceleb_16k`, revision `v1.0.2`, trained on VoxCeleb2-dev. The standalone weight does not include its Apache-2.0 license, so Dictate must package that license and provenance beside it.

The Pyannote conversion comes from `pyannote/segmentation-3.0`. Sherpa's archive includes an MIT license with a 2022 CNRS copyright notice; upstream uses a 2023 CNRS notice. Dictate must retain both notices until that discrepancy is resolved rather than replacing one. Upstream access is gated even though the model is MIT; shipping sherpa's ungated conversion should receive an explicit distribution review.

On this machine, one warm debug-build analysis of the 10-second user-only clip, including a profile WAV comparison, took 3.81 seconds wall time, 4.02 seconds user CPU, and 0.85 seconds system CPU. This is a real-time factor of about 0.38, but one host and one clip cannot set a product budget. Release-build timing and peak memory still need measurement on supported hardware.

## Decisions

- Preserve native channels in future diagnostics and spatial spikes.
- Keep current production averaging until a spatial transform improves measured language outcomes.
- Use the full segmentation model for the next diarization evaluation; do not substitute int8 without parity evidence.
- Keep timestamp alignment and anonymous attributed transcripts as analysis output only; no current path filters audio or text.
- Treat whole-utterance speaker verification as a possible TV-only rejection signal, not as overlap separation.
- Do not choose a speaker threshold from the current recordings. Resume broader identity evidence only when normal use establishes a need.

## Next gates

1. Run Plan 009's offline spatial variants against the existing native stereo clips and compare language outcomes.
2. Split anonymous attribution from optional identity before landing Plan 011.
3. Record model provenance, licenses, checksums, package size, and CPU behavior before distributing Pyannote or CAM++.
4. Keep identity thresholds, profile storage, conversation sessions, and all speech filtering deferred until ordinary use establishes a need and a separate plan clears their gates.
