# Dictate

Native voice-to-text dictation for Linux.

Dictate is being rebuilt as a Rust/GPUI app with a Wayland layer-shell overlay, local/offline transcription, live audio visualization, and a dictation text core that turns raw speech into useful text.

## Current state

The GPUI rewrite currently provides:

- daemon-controlled Wayland layer-shell overlay
- live microphone waveform from speech-band FFT analysis
- local/offline transcription through `sherpa-onnx`
- centralized model catalog for Whisper, Parakeet, SenseVoice, and Moonshine models
- command-triggered bounded dictation: keep `dictate daemon` running, then run `dictate record toggle` to start/stop capture
- deterministic text formatting for cleanup, spoken punctuation, dictionary/replacement rules, modes, and technical terms
- stdout or clipboard delivery for formatted dictation
- headless WAV transcription with `dictate transcribe <wav> [--raw] [--model <id>]`

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. The daemon keeps GPUI running in the background with no window while idle, then opens the layer-shell overlay only while recording/transcribing. Use `dictate daemon --delivery stdout|clipboard` to override configured delivery for that daemon run.

Manual recordings auto-stop after 10 minutes to cap memory growth. The default `parakeet-tdt-0.6b-v2-int8` model transcribes the full capture; Whisper models from the catalog only transcribe the first ~30 seconds in sherpa-onnx's offline recognizer.

## Configuration

Dictate loads settings from `~/.config/dictate/config.toml` when the daemon starts. Restart `dictate daemon` after changing config.

```toml
model = "parakeet-tdt-0.6b-v2-int8"
mode = "technical"
spoken_formatting = "punctuation-and-lines"
delivery = "clipboard"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my email"
written = "josh@joshthomas.dev"
```

`mode` accepts `raw`, `literal`, `message`, `email`, `note`, `technical`, or `command`. `spoken_formatting` accepts `disabled`, `punctuation-only`, or `punctuation-and-lines`. `delivery` accepts `stdout` or `clipboard`.

`model` selects any catalog entry. Current model ids are `whisper-tiny-en`, `whisper-tiny`, `whisper-base-en`, `whisper-base`, `whisper-small-en`, `whisper-small`, `whisper-medium-en`, `whisper-medium`, `parakeet-tdt-0.6b-v2-int8`, `parakeet-tdt-0.6b-v3-int8`, `parakeet-tdt-ctc-110m-int8`, `sense-voice-small-int8`, `moonshine-tiny-en`, `moonshine-base-en`, `moonshine-v2-tiny-en`, and `moonshine-v2-base-en`.

## Development

```bash
just run
just check
just test
just fmt
```

## Requirements

- Linux Wayland compositor with layer-shell support
- Audio input device
- Rust toolchain from `rust-toolchain.toml`

## License

MIT
