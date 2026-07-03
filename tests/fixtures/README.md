# Transcription fixtures

These fixtures exercise Dictate's raw ASR path with committed audio. They are intentionally small, licensed, and reproducible.

## Fixture rules

- Commit only 16 kHz mono WAV files.
- Keep one sibling `.txt` transcript next to each `.wav` file.
- Keep a `LICENSE` file in each corpus directory.
- Do not add GPL, non-commercial, unclear-provenance, or generated-by-unknown-source clips.
- Record source selection in `manifest.toml`.
- Record byte identity in `manifest.lock`.
- Keep manifests minimal: source URLs, selected source clip paths, transcript source paths, committed license paths, mirror revisions when used, and conversion commands belong in `manifest.toml`; checksums belong in `manifest.lock`.
- Raw recognizer hypotheses are tracked by per-fixture model-backed integration snapshots and guarded by WER/CER thresholds. Formatter behavior belongs in `src/text.rs` snapshots.

## Source order

Prefer sources in this order:

1. CMU ARCTIC: direct 16 kHz WAVs with a BSD-style license.
2. LJ Speech v1.1: public-domain audio and text, converted from 22.05 kHz mono WAV to 16 kHz mono WAV.
3. Common Voice: only if a future download path allows committed clips without violating Mozilla Data Collective redistribution terms.

Common Voice is CC0, but current Mozilla Data Collective terms ask users not to post, distribute, or mirror Common Voice datasets in whole or in part outside MDC. Do not commit Common Voice audio fixtures unless that policy changes or Josh explicitly approves another source with suitable consent and provenance.

## Current corpora

### CMU ARCTIC

Source: `http://festvox.org/cmu_arctic/packed/cmu_us_bdl_arctic.tar.bz2`

The committed CMU fixtures are copied from the source archive without audio conversion. They already satisfy Dictate's 16 kHz mono WAV fixture contract.

### LJ Speech

Source: `https://data.keithito.com/data/speech/LJSpeech-1.1.tar.bz2`

To avoid downloading the full archive for small fixture updates, the current source audio was downloaded from the `flexthink/ljspeech` Hugging Face mirror revision recorded in `manifest.toml`. The official LJ Speech page remains the license and provenance source.

Conversion command shape:

```sh
ffmpeg -y -i <source-wav> -ac 1 -ar 16000 -sample_fmt s16 <fixture-wav>
```

### Spoken commands (synthesized)

Source: self-generated with Piper TTS using `en_US-ljspeech-high` and `en_US-ljspeech-medium` voices from `rhasspy/piper-voices`.

These clips cover formatter collisions where native ASR punctuation lands next to spoken punctuation commands such as "question mark", "exclamation mark", "comma", and "period". The voices are trained from the public-domain LJ Speech dataset, and the generated audio in this repo is dedicated under CC0 1.0.

These are self-generated TTS clips with known source and provenance, so they are not covered by the generated-by-unknown-source ban. Piper synthesis is non-deterministic; the committed WAVs are canonical, and the recorded generation commands are provenance documentation only.

## Adding a fixture

1. Confirm the source license permits committed redistribution.
2. Add the source corpus directory if needed, including `LICENSE`.
3. Add a 16 kHz mono `.wav` and sibling `.txt` transcript.
4. Add the source selection to `manifest.toml`.
5. Add the source and committed-file checksums to `manifest.lock`.
6. Run `just test`.
7. If a local default model is installed, run `just test-integration` and review any per-fixture hypothesis snapshot diffs.
