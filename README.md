# dictate

Lightweight CLI voice transcription service for Linux with fast, local speech-to-text conversion.

## Requirements

- Audio input device (microphone)
- One of the following for text insertion:
  - **Wayland**: `wtype`
  - **X11**: `xdotool`
- One of the following for clipboard support:
  - **Wayland**: `wl-clipboard`
  - **X11**: `xclip`

## Installation

### Install from Source

```bash
git clone https://github.com/joshuadavidthomas/dictate
cd dictate
cargo install --path .
```

#### Systemd Service

Install the user service for automatic startup and crash recovery:

```bash
# Install service
cp systemd/dictate.service ~/.config/systemd/user/
systemctl --user daemon-reload

# Enable auto-start at login
systemctl --user enable dictate.service

# Start service immediately
systemctl --user start dictate.service
```

## Quick Start

### 1. Download a Whisper Model

Before transcribing, download a Whisper model:

```bash
# List available models
dictate models list

# Download the base model (recommended for most users)
dictate models download base

# Or download the tiny model (faster, less accurate)
dictate models download tiny
```

**Model comparison:**
- `tiny` - Fastest, ~75MB, good for quick notes
- `base` - Balanced, ~142MB, recommended for most users
- `small` - More accurate, ~466MB, slower
- `medium` - Most accurate, ~1.5GB, slowest

### 2. Start Transcribing

Dictate works in two modes:

**Service Mode (Recommended):** The systemd service runs automatically for fast, push-to-talk operation:

```bash
# Basic transcription (prints to terminal)
dictate transcribe

# Type at cursor position
dictate transcribe --insert

# Copy to clipboard
dictate transcribe --copy

# Both insert and copy
dictate transcribe --insert --copy
```

**Standalone Mode:** If the systemd service isn't running, dictate automatically falls back to standalone mode:

```bash
# Standalone mode with custom silence detection
dictate transcribe --silence-duration 3 --max-duration 60
```

**Recording controls:**
- Speak naturally—recording auto-stops after 2 seconds of silence (configurable with `--silence-duration`)
- Maximum recording duration: 30 seconds (configurable with `--max-duration`)
- Press Ctrl+C to cancel

### 3. Check Audio Devices (Optional)

List available audio recording devices:

```bash
dictate devices
```

## Commands

### `dictate transcribe`

Record audio and transcribe to text. Works in service mode (fast) or standalone mode (fallback).

**Options:**
- `--insert` - Type transcribed text at cursor position
- `--copy` - Copy transcribed text to clipboard  
- `--format <FORMAT>` - Output format: `text` (default), `json`
- `--max-duration <SECONDS>` - Maximum recording duration (default: 30)
- `--silence-duration <SECONDS>` - Silence duration before auto-stopping in standalone mode (default: 2)
- `--socket-path <PATH>` - Custom socket path (default: `/run/user/$UID/dictate.sock`)

**Examples:**

```bash
# Basic transcription (service mode)
dictate transcribe

# Insert at cursor with 60 second max duration
dictate transcribe --insert --max-duration 60

# Copy to clipboard
dictate transcribe --copy

# JSON output for scripting
dictate transcribe --format json

# Standalone mode with custom silence detection
dictate transcribe --silence-duration 3 --max-duration 60
```

### `dictate service`

Start the transcription service. Usually not needed—auto-starts on first transcription request when running under systemd.

**Options:**
- `--socket-path <PATH>` - Unix socket path (default: `/run/user/$UID/dictate/dictate.sock`)
- `--model <NAME>` - Model to load (default: `whisper-base`)
- `--sample-rate <HZ>` - Audio sample rate (default: 16000)
- `--idle-timeout <SECONDS>` - Unload model after inactivity (default: 300)

**Examples:**

```bash
# Start service (runs in foreground, systemd handles backgrounding)
dictate service

# Custom idle timeout (10 minutes)
dictate service --idle-timeout 600

# Use tiny model for faster transcription
dictate service --model whisper-tiny

# Custom socket path for testing
dictate service --socket-path /tmp/dictate-test.sock
```

### `dictate status`

Check service health and configuration.

**Options:**
- `--socket-path <PATH>` - Custom socket path

**Example output:**

```json
{
  "service_running": true,
  "model_loaded": true,
  "model_path": "/home/user/.local/share/dictate/models/base.bin",
  "audio_device": "default",
  "uptime_seconds": 3600,
  "last_activity_seconds_ago": 45
}
```

### `dictate stop`

Gracefully stop the background service.

**Options:**
- `--socket-path <PATH>` - Custom socket path

**Example:**

```bash
dictate stop
```

### `dictate devices`

List available audio recording devices and their capabilities.

**Example output:**

```
Available Audio Devices:
Name                          Default    Sample Rates         Formats
--------------------------------------------------------------------------------
default                       YES        44100, 48000         I16, F32
pulse                         NO         44100, 48000         I16, F32
hw:0,0                        NO         8000, 16000, 44100  I16
```

**Use cases:**
- Troubleshooting audio device issues
- Verifying microphone availability
- Checking supported sample rates

### `dictate models`

Manage Whisper models.

#### `dictate models list`

List all available models and their download status.

**Example output:**

```
Available Models:
Name            Type       Size        Downloaded  Path
tiny            whisper    75 MB       YES         /home/user/.local/share/dictate/models/tiny.bin
base            whisper    142 MB      NO          N/A
small           whisper    466 MB      NO          N/A
medium          whisper    1.5 GB      NO          N/A

Storage Information:
Models Directory: /home/user/.local/share/dictate/models/
Downloaded: 1/4 models
Total Size: 75 MB
```

#### `dictate models download <MODEL>`

Download a Whisper model from HuggingFace.

**Examples:**

```bash
# Download base model
dictate models download base

# Download tiny model
dictate models download tiny
```

**Features:**
- Progress bar with download speed and ETA
- SHA256 verification (when available)
- Automatic retry on network errors
- Disk space checking before download

#### `dictate models remove <MODEL>`

Delete a downloaded model to free disk space.

**Example:**

```bash
dictate models remove tiny
```

## Operation Modes

### Service Mode (Recommended)

Fast, push-to-talk operation with preloaded model:

```bash
# Service runs via systemd (no manual start needed)
systemctl --user start dictate

# Fast transcription
dictate transcribe --insert
```

**Benefits:**
- ~500ms total latency
- Model stays loaded in memory
- Push-to-talk behavior
- Automatic service management via systemd

### Standalone Mode

Automatic fallback when service isn't running:

```bash
# Works without service - slower but reliable
dictate transcribe --insert --silence-duration 3
```

**Behavior:**
- Loads model for each transcription
- Higher latency (~2-3 seconds)
- Useful for occasional use
- No background service required

## Advanced Usage

### Custom Socket Path

Useful for testing multiple instances or non-standard setups:

```bash
# Start service with custom socket
dictate service --socket-path /tmp/dictate-test.sock

# Use custom socket for transcription
dictate transcribe --socket-path /tmp/dictate-test.sock --insert
```

### Audio Device Selection

```bash
# List available devices
dictate devices

# Use specific device (if supported in future versions)
# dictate transcribe --device "hw:1,0"
```

### Scripting with JSON Output

```bash
#!/bin/bash
# Record transcription and process with jq

result=$(dictate transcribe --format json)
text=$(echo "$result" | jq -r '.text')
confidence=$(echo "$result" | jq -r '.confidence')

echo "Transcribed: $text (confidence: $confidence)"
```

### Monitoring Service Health

```bash
#!/bin/bash
# Check if service is responsive

status=$(dictate status --format json)
if echo "$status" | jq -e '.service_running == true' > /dev/null; then
    echo "Service is running"
else
    echo "Service is down, restarting..."
    systemctl --user restart dictate
fi
```

## Privacy & Security

- **100% Local**: All transcription happens on your machine
- **No Network**: No data sent to cloud services
- **No Logging**: Transcriptions are not stored (unless you save them)
- **User Isolation**: Socket permissions restrict access to your user
- **Open Source**: Audit the code yourself

## Development

### Building

```bash
cargo build --release
```

### Installing from Source

```bash
cargo install --path .
```

### Running Tests

```bash
cargo test
```

### Debug Mode

```bash
# Build in debug mode
cargo build

# Run with verbose logging
RUST_LOG=debug ./target/debug/dictate service

# Debug standalone transcription
RUST_LOG=debug ./target/debug/dictate transcribe --silence-duration 5
```

### Development Workflow

```bash
# 1. Make changes
cargo build

# 2. Test service mode (in separate terminal)
./target/debug/dictate service

# 3. In another terminal, test transcription
./target/debug/dictate transcribe --insert

# 4. Test standalone mode (stop service first)
./target/debug/dictate transcribe --silence-duration 3
```

## Credits/Inspiration

Prototype partially vibed using [Opencode](https://opencode.ai) and a mixture of Claude Sonnet 4.5 and GLM 4.6.

- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) - Fast Whisper implementation
- [transcribe-rs](https://github.com/thewh1teagle/transcribe-rs) - Rust bindings for Whisper
- [cpal](https://github.com/RustAudio/cpal) - Cross-platform audio I/O
- [Whispering](https://github.com/EpicenterHQ/epicenter/tree/main/apps/whispering) - Architecture inspiration

## License

dictate is licensed under the MIT license. See the [`LICENSE`](LICENSE) file for more information.
