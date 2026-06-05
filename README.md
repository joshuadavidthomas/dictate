# Dictate

Native voice-to-text dictation for Linux.

Dictate is being rebuilt as a Rust/GPUI app with a Wayland layer-shell overlay, local/offline transcription, live audio visualization, and a dictation-processing core that turns raw speech into useful text.

## Current state

The GPUI rewrite currently provides:

- daemon-controlled Wayland layer-shell overlay
- live microphone waveform from speech-band FFT analysis
- local/offline transcription through `sherpa-onnx`
- centralized model catalog for Whisper, Parakeet, SenseVoice, and Moonshine models
- command-triggered bounded dictation: keep `dictate daemon` running, then run `dictate record toggle` to start/stop capture
- deterministic post-processing for cleanup, spoken punctuation, dictionary/replacement rules, modes, and technical terms

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. The daemon spawns the GPUI child app only while recording/transcribing; there is no idle transparent overlay.

The next focus is replacing stdout output with app-level transcript events and real delivery targets such as copy, insert, and configured output modes.

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
