# 003 Capture Notes

Captured on 2026-07-03 for plan 003 v2.

**Audio artifacts (relocated post-capture):** the three final WAVs are
committed at `tests/fixtures/spoken-commands/clip-{a,b,c}.wav` (checksums
below verified before relocation). Because Piper synthesis is
non-deterministic (see Clip B note), these files — not the generation
commands — are the source of truth. The original
`plans/formatter-punctuation-compat/clips/` staging directory was removed;
the generation commands below are provenance documentation, recorded in the
manifest as `fixture_transform` with a non-reproducibility note.

Original capture scratch dir (session-temporary; rejected tuning variants
and per-model capture logs):

```text
/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad/codex-003-work/
```

The requested `uvx --offline --from piper-tts piper` path could not be used
inside this sandbox because `uvx` tried to mutate the read-only home cache:

```text
error: failed to open file `/home/josh/.cache/uv/sdists-v9/.git`: Read-only file system (os error 30)
```

The cached Piper binary was usable directly:

```text
/home/josh/.cache/uv/archive-v0/l6HGEr2NPYi45ECfrXBeD/bin/piper
```

## Final Clip Inputs

| Clip | Voice | Piper parameters | Script |
|---|---|---|---|
| A | `en_US-ljspeech-high.onnx` | `--length-scale 1.4 --sentence-silence 0.6` | `is this working question mark yes exclamation mark item audio next item comma text period` |
| B | `en_US-ljspeech-medium.onnx` | `--length-scale 1.0 --sentence-silence 0.0` | `hello comma world period new paragraph thanks period this is a simple test` |
| C | `en_US-ljspeech-high.onnx` | `--length-scale 1.4 --sentence-silence 0.6` | `that is a good question. mark will answer. the sentence has a comma in it.` |

Clip A dropped only `colon` and `semicolon` from the plan seed after HIGH
voice tuning still produced WER-breaking mistranscriptions for those two
command words. The remaining Clip A command coverage is comma, period,
question mark, and exclamation mark.

Clip B uses the exact scratch WAV from the first MEDIUM `length-scale 1.0`
generation. Re-running the same Piper command later produced a nonviable
variant (`Aloha World Period ...`), so the fixture-commit step should use
the retained WAV artifact rather than assume the Piper command is byte-stable.

Final WAV checksums:

```text
b232a448478c4e6ae08d3cacb7653fdf937588d2f8a77a6842694b6a5abc3e70  clip-a.wav
84e4adb0c8a0eadc36ce7a8eca2b9d32e35b14be67e0c32912a469ac24abe466  clip-b.wav
b187a0e25bf29816d44211d474c79848af4b7d64d4a1769ce4e61d3e2cca1e78  clip-c.wav
```

## Final Generation And Conversion Commands

Clip A:

```sh
VOICE_DIR=/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad
WORK=/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad/codex-003-work
printf '%s' 'is this working question mark yes exclamation mark item audio next item comma text period' | /home/josh/.cache/uv/archive-v0/l6HGEr2NPYi45ECfrXBeD/bin/piper -m "$VOICE_DIR/en_US-ljspeech-high.onnx" --length-scale 1.4 --sentence-silence 0.6 -f "$WORK/clip-a.raw.wav"
ffmpeg -y -i "$WORK/clip-a.raw.wav" -ac 1 -ar 16000 -sample_fmt s16 "$WORK/clip-a.wav"
```

Clip B:

```sh
VOICE_DIR=/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad
WORK=/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad/codex-003-work
printf '%s' 'hello comma world period new paragraph thanks period this is a simple test' | /home/josh/.cache/uv/archive-v0/l6HGEr2NPYi45ECfrXBeD/bin/piper -m "$VOICE_DIR/en_US-ljspeech-medium.onnx" --length-scale 1.0 --sentence-silence 0.0 -f "$WORK/clip-b-medium-ls1.0-ss0.0.raw.wav"
ffmpeg -y -i "$WORK/clip-b-medium-ls1.0-ss0.0.raw.wav" -ac 1 -ar 16000 -sample_fmt s16 "$WORK/clip-b-medium-ls1.0-ss0.0.wav"
cp "$WORK/clip-b-medium-ls1.0-ss0.0.raw.wav" "$WORK/clip-b.raw.wav"
cp "$WORK/clip-b-medium-ls1.0-ss0.0.wav" "$WORK/clip-b.wav"
```

Clip C:

```sh
VOICE_DIR=/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad
WORK=/tmp/claude-1000/-home-josh-projects-joshuadavidthomas-dictate/35a37641-bf1a-421b-95ba-0a5d7986f2e2/scratchpad/codex-003-work
printf '%s' 'that is a good question. mark will answer. the sentence has a comma in it.' | /home/josh/.cache/uv/archive-v0/l6HGEr2NPYi45ECfrXBeD/bin/piper -m "$VOICE_DIR/en_US-ljspeech-high.onnx" --length-scale 1.4 --sentence-silence 0.6 -f "$WORK/clip-c.raw.wav"
ffmpeg -y -i "$WORK/clip-c.raw.wav" -ac 1 -ar 16000 -sample_fmt s16 "$WORK/clip-c.wav"
```

## Capture Commands

Each capture used the clean config prefix:

```sh
XDG_CONFIG_HOME=/tmp/dictate-003-clean-config just run transcribe <clip>.wav --raw
XDG_CONFIG_HOME=/tmp/dictate-003-clean-config just run transcribe <clip>.wav --model parakeet-tdt-0.6b-v2-int8 --raw
XDG_CONFIG_HOME=/tmp/dictate-003-clean-config just run transcribe <clip>.wav --model parakeet-tdt-0.6b-v2-int8
```

## Transcripts And Viability

WER viability counts are normalized word edit counts after lowercasing,
stripping punctuation, and collapsing whitespace.

| Clip | Whisper raw | Whisper count | Parakeet raw | Parakeet count |
|---|---:|---:|---|---:|
| A | `Is this working question mark? Yes, exclamation mark? Item audio? Next, item, comma text period.` | 0 | `Is this working question mark? Yes, exclamation mark item audio next item comma text period.` | 0 |
| B | `A low comma world, period new paragraph thanks period this is a simple test.` | 2 | `Hello, comma world period new paragraph Thanks period This is a simple test.` | 0 |
| C | `That is a good question mark will answer the sentence has a comma in it.` | 0 | `That is a good question mark will answer the sentence has a comma in it.` | 0 |

Formatted Parakeet outputs:

Clip A:

```text
Is this working? Yes! Item audio next item, text.
```

Clip B:

```text
Hello, world.

Thanks. This is a simple test.
```

Clip C:

```text
That is a good? Will answer the sentence has a, in it.
```

Clip A satisfies the collision criterion: the raw Parakeet transcript has
native punctuation attached to/around command words (`question mark?` and
`Yes, exclamation mark`), while the formatted output has no doubled
punctuation and no surviving command words.

Clip C is the known content-word hazard from the plan: Parakeet transcribed
the content words near-verbatim, and the formatter interpreted `question
mark` and `comma` as spoken commands. This is not a 002 dedup failure.

## Tuning Attempts

Clip A, HIGH `--length-scale 1.2 --sentence-silence 0.3`, original script:

```text
Is this working question mark yes exclamation mark item fair audio semophy next item commer text period?
```

Clip A, HIGH `--length-scale 1.4 --sentence-silence 0.6`, original script:

```text
Is this working? Question mark, yes, exclamation mark, item fi audio, semophi nex, item comma text, period.
```

Those attempts showed the desired collision, but `colon` and `semicolon`
remained WER-breaking, so the final Clip A dropped only those two command
words.

Clip B, HIGH `--length-scale 1.4 --sentence-silence 0.6`:

```text
A low commercial world period new paragraph thanks period this is a simple test.
```

Clip B, MEDIUM `--length-scale 1.4 --sentence-silence 0.6`:

```text
Hello, comma world. Period. New paragraph. Thanks. Period. This is a simple test.
```

Clip B selected the first MEDIUM `--length-scale 1.0 --sentence-silence
0.0` artifact because it was near-verbatim and formatted cleanly.
