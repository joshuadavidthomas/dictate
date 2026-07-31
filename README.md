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
- retained last transcript with `dictate paste` for explicit insertion at the current cursor
- deterministic text formatting for cleanup, spoken punctuation, dictionary/replacement rules, modes, and technical terms
- insert, clipboard, or stdout delivery for formatted dictation
- headless WAV transcription with `dictate transcribe <wav> [--raw] [--model <id>]`

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. Bind a second shortcut to `dictate paste` to insert the last completed transcript at the current cursor. The daemon keeps GPUI running in the background with no window while idle. When automatic insertion is skipped or safely fails, use `dictate paste` after choosing a destination. Persistent recovery notices can be cleared with `dictate dismiss`. Use `dictate daemon --delivery insert|clipboard|stdout` to override configured delivery for that daemon run.

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

`mode` accepts `raw`, `literal`, `message`, `email`, `note`, `technical`, or `command`. `spoken_formatting` accepts `disabled`, `punctuation-only`, or `punctuation-and-lines`. `delivery` accepts `insert`, `clipboard`, or `stdout`.

`insert` snapshots the regular Wayland clipboard before delivery, including every advertised MIME representation. It checks every representation after capture, then checks the full snapshot once more just before publishing the transcript as text alongside a private transaction marker. If either check finds a change, Dictate direct-types without publishing. Across a short settle window, it repeatedly checks both the marker and the exact transcript, and asks `wtype` to send one Ctrl+Shift+V clipboard paste chord only while the temporary selection remains stable. After a short grace period, it checks ownership once more and restores the snapshot only when the marker and transcript, or the exact transcript when a clipboard manager removed the marker, still identify Dictate's offer. Empty clipboards are restored to empty. This works with `wl-clip-persist` when it preserves the private marker and uses exact transcript identity when it does not.

Clipboard ownership checks and publication or restoration are separate Wayland operations. Another client can take ownership after Dictate's final check but before its set request; in that narrow race, Dictate can overwrite the newer clipboard. The checks shrink this window but cannot make compare-and-set atomic.

Clipboard capture is fail-closed: more than 64 MIME types, more than 8 MiB of total data, a transfer timeout, an incomplete representation, or a concurrent clipboard change stops the transaction before the paste chord. Transcripts over 1 MiB also skip clipboard publication. For these pre-paste failures only, Dictate runs `wtype -` and sends the transcript through stdin as direct virtual-keyboard input. If direct `wtype` cannot start, insert delivery ends with that failure; it does not copy to the ordinary clipboard or write to stdout. If the clipboard paste chord process starts, Dictate never direct-types, retries, or uses another delivery route, even when the chord result is uncertain. This point-of-no-return rule prevents duplicate insertion. Clipboard insert transactions are serialized and clipboard contents are never logged.

The clipboard payload transfer deadlines do not bound `wl-clipboard-rs`'s synchronous Wayland connection, registry, and request-setup roundtrips. A stalled compositor can still block an insert transaction during those setup calls.

Insert delivery requires a single-seat Wayland session. `wl-clipboard-rs` can read from an unspecified selected seat and publish to all seats, but its public API does not expose the selected seat name so Dictate can restore that exact seat. Dictate makes no multi-seat clipboard safety claim.

For insert delivery, Dictate captures the focused window when the stopping `record stop` or `record toggle` command starts, then checks focus again immediately before insertion. It submits text only when both observations identify the same compositor window. Changed or unverifiable focus retains the transcript and tries ordinary clipboard delivery; run `dictate paste` after choosing a destination. Niri window IDs are supported now. Window identity still cannot prove that a text field accepted the paste, and a narrow check-to-launch race remains because compositor IPC, clipboard ownership, and `wtype` are separate processes.

`model` selects any catalog entry. Current model ids are `whisper-tiny-en`, `whisper-tiny`, `whisper-base-en`, `whisper-base`, `whisper-small-en`, `whisper-small`, `whisper-medium-en`, `whisper-medium`, `parakeet-tdt-0.6b-v2-int8`, `parakeet-tdt-0.6b-v3-int8`, `parakeet-tdt-ctc-110m-int8`, `sense-voice-small-int8`, `moonshine-tiny-en`, `moonshine-base-en`, `moonshine-v2-tiny-en`, and `moonshine-v2-base-en`.

## Development

```bash
just run
just check
just test
just fmt
```

Build and install the development channel as `~/.local/bin/dictate-dev`:

```bash
just install-dev
```

The install recipe also installs, enables, and restarts the `dictate-dev.service` systemd user unit. The development build uses its own config at `~/.config/dictate-dev/config.toml`, daemon socket, and Wayland app identity. It shares downloaded speech models with stable builds. Re-run `just install-dev` after changing the code; the recipe restarts the daemon with the new executable.

If `~/.local/bin` is in Niri's inherited `PATH`, compositor bindings can invoke the client by name:

```kdl
binds {
    Mod+D { spawn "dictate-dev" "record" "toggle"; }
    Mod+Shift+D { spawn "dictate-dev" "paste"; }
}
```

Inspect daemon output with `journalctl --user -u dictate-dev.service -f`.

Create a stable release build with `just build-release`. Stable builds use the `dictate` name and the existing `~/.config/dictate/config.toml` configuration.

## Requirements

- Linux Wayland compositor with layer-shell and `ext-data-control` or `wlr-data-control` support
- Single Wayland seat for `insert` delivery
- Audio input device
- `wtype` for `insert` delivery
- Rust toolchain from `rust-toolchain.toml`

## License

MIT
