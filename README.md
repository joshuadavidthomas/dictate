# Dictate

Voice-to-text transcription for Linux.

Dictate is a desktop app that records your voice and transcribes it locally using Whisper or Parakeet models. No cloud services required — all processing happens on your machine.

## Features

- **Local transcription** — Whisper and Parakeet models run entirely on your device
- **Global hotkey** — Start/stop recording from anywhere (default: Ctrl+Shift+Space)
- **Multiple output modes** — Print to stdout, copy to clipboard, or insert directly into focused window
- **On-screen display** — Minimal overlay shows recording status (Wayland layer-shell)
- **Transcription history** — Browse and search past transcriptions
- **Model manager** — Download and manage transcription models from the app

## Requirements

- Linux (Wayland recommended, X11 supported)
- Audio input device

## Installation

Download the latest release from the [Releases](https://github.com/joshuadavidthomas/dictate/releases) page.

## Usage

1. Launch Dictate
2. Download a transcription model from Settings → Models
3. Press Ctrl+Shift+Space (or your configured hotkey) to start recording
4. Speak, then press the hotkey again to stop and transcribe

## Configuration

Settings are stored in `~/.config/dictate/settings.toml`.

## License

MIT
