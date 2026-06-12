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
- stdout delivery for formatted dictation

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. The daemon keeps GPUI running in the background with no window while idle, then opens the layer-shell overlay only while recording/transcribing.

Manual recordings auto-stop after 120 seconds to cap memory growth. The current default Whisper model only transcribes the first ~30 seconds of a capture in sherpa-onnx's offline recognizer; longer dictation needs future chunking work.

The next focus is real delivery targets such as copy, insert, and configured output modes.

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
