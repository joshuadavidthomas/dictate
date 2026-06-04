# Dictate

Native voice-to-text dictation for Linux.

Dictate is being rebuilt as a Rust/GPUI app with a Wayland layer-shell overlay, local/offline transcription, live audio visualization, and a dictation-processing core that turns raw speech into useful text.

## Current state

The GPUI rewrite currently provides:

- Wayland layer-shell OSD overlay
- live microphone waveform from speech-band FFT analysis
- local/offline transcription through `sherpa-onnx`
- centralized model catalog for Whisper, Parakeet, SenseVoice, and Moonshine models
- continuous microphone transcription prototype

The next focus is the dictation processing core: raw transcript separation, deterministic cleanup, spoken punctuation, dictionary/replacement rules, modes/profiles, and optional LLM rewriting.

## Development

```bash
cargo run
cargo check --all-targets
cargo test
cargo fmt
```

## Requirements

- Linux Wayland compositor with layer-shell support
- Audio input device
- Rust toolchain from `rust-toolchain.toml`

## License

MIT
