# Dictate

Voice-to-text dictation for modern Linux desktops.

Many desktop dictation apps start on macOS or spread their early work across every operating system. Dictate is starting with modern Linux and Wayland. Systemd runs the daemon, compositor hotkeys control recording, Wayland focus guards text insertion, and local models transcribe speech. GPUI leaves room for other platforms later without thinning out the Linux integration today. The app adds a layer-shell overlay, live audio visualization, and text formatting for spoken punctuation, messages, notes, and technical work.

## Current state

The shipped app lives in `crates/dictate`; its speech engine and transcription fixtures live in `crates/dictate-speech`.

The app currently provides:

- daemon-controlled Wayland layer-shell overlay
- live microphone waveform from speech-band FFT analysis
- local/offline transcription through `sherpa-onnx`
- centralized model catalog for Whisper, Parakeet, SenseVoice, and Moonshine models
- command-triggered bounded dictation: keep `dictate daemon` running, then run `dictate record toggle` to start/stop capture
- optional portal-managed push-to-talk on desktops that implement XDG GlobalShortcuts
- retained last transcript with `dictate paste` for explicit insertion at the current cursor
- deterministic text formatting for cleanup, spoken punctuation, dictionary/replacement rules, modes, and technical terms
- insert, clipboard, or stdout delivery for formatted dictation
- headless WAV transcription with `dictate transcribe <wav> [--raw] [--model <id>]`

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. Bind a second shortcut to `dictate paste` to insert the last completed transcript at the current cursor. The daemon keeps GPUI running in the background with no window while idle. When automatic insertion is skipped or safely fails, use `dictate paste` after choosing a destination. Persistent recovery notices can be cleared with `dictate dismiss`. Use `dictate daemon --delivery insert|clipboard|stdout` to override configured delivery for that daemon run.

Manual recordings auto-stop after 10 minutes to cap memory growth. The daemon runs two models: `partials_model` (a streaming model, default `fast-conformer-ctc-en-80ms-int8`) decodes speech as it is captured and logs live hypotheses, while `model` (any catalog entry, default `parakeet-tdt-0.6b-v2-int8`) produces the final text at stop. The final decode takes about a quarter of the recording length, so a 10-second dictation is ready in ~2.5 seconds; live partials appear within a few hundred milliseconds. Offline catalog models (Whisper, SenseVoice, Moonshine, offline Parakeet) cannot drive live partials but remain valid final models and stay available to `dictate transcribe` for headless evaluation.

## Installation

Install Dictate from a source checkout:

```bash
just install
```

This builds the stable release, moves it to `~/.local/bin/dictate`, installs its desktop identity for portal permissions, and installs, enables, and starts the `dictate.service` systemd user unit. Re-run the command to install a new build. The service starts with the graphical session and restarts after a failure.

Add shortcuts to your compositor after making sure it inherits a `PATH` that contains `~/.local/bin`. For Niri:

```kdl
binds {
    Mod+D { spawn "dictate" "record" "toggle"; }
    Mod+Shift+D { spawn "dictate" "paste"; }
}
```

Check the daemon with `systemctl --user status dictate.service`. Follow its logs with `journalctl --user -u dictate.service -f`.

Run `just build-release` when you want the release binary without installing it. Stable builds use the `dictate` name and `~/.config/dictate/config.toml`. They omit the development harness and its `debug` subcommand.

## Configuration

Dictate loads settings from `~/.config/dictate/config.toml` when the daemon starts. Restart `dictate daemon` after changing config.

```toml
model = "parakeet-tdt-0.6b-v2-int8"
partials_model = "fast-conformer-ctc-en-80ms-int8"
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

Enable press-and-hold recording through the XDG GlobalShortcuts portal with one preferred trigger:

```toml
[shortcuts]
push_to_talk = "<Super>d"
```

The desktop owns the final binding and may ask you to choose a different one. Pressing the bound shortcut starts recording and releasing it stops recording with the configured delivery target. If the portal session ends while the key is held, Dictate cancels that recording rather than waiting forever for a release event. Removing the `push_to_talk` shortcut disables portal registration.

A desktop must implement the GlobalShortcuts portal for this to work. Niri 26.04 does not, so Niri users should keep the compositor toggle bindings above for now. Dictate's portal interface is ready for Niri or another backend once one ships.

Override delivery for one recording from a separate hotkey:

```bash
dictate record toggle --delivery clipboard
```

Dictate remembers the override when recording starts, so `dictate record stop` can finish it without repeating the flag. A delivery flag on the stopping command takes precedence. Clipboard delivery keeps the completed transcript available to `dictate paste`, copies it to the regular clipboard, and leaves window focus alone.

`insert` snapshots the regular Wayland clipboard before delivery, including every advertised MIME representation. It checks every representation after capture, then checks the full snapshot once more just before publishing the transcript as text alongside a private transaction marker. If either check finds a change, Dictate direct-types without publishing. Across a short settle window, it repeatedly checks both the marker and the exact transcript, and asks `wtype` to send one Ctrl+Shift+V clipboard paste chord only while the temporary selection remains stable. After a short grace period, it checks ownership once more and restores the snapshot only when the marker and transcript, or the exact transcript when a clipboard manager removed the marker, still identify Dictate's offer. Empty clipboards are restored to empty. This works with `wl-clip-persist` when it preserves the private marker and uses exact transcript identity when it does not.

Clipboard ownership checks and publication or restoration are separate Wayland operations. Another client can take ownership after Dictate's final check but before its set request; in that narrow race, Dictate can overwrite the newer clipboard. The checks shrink this window but cannot make compare-and-set atomic.

Clipboard capture is fail-closed: more than 64 MIME types, more than 8 MiB of total data, a transfer timeout, an incomplete representation, or a concurrent clipboard change stops the transaction before the paste chord. Transcripts over 1 MiB also skip clipboard publication. For these pre-paste failures only, Dictate runs `wtype -` and sends the transcript through stdin as direct virtual-keyboard input. If direct `wtype` cannot start, insert delivery ends with that failure; it does not copy to the ordinary clipboard or write to stdout. If the clipboard paste chord process starts, Dictate never direct-types, retries, or uses another delivery route, even when the chord result is uncertain. This point-of-no-return rule prevents duplicate insertion. Clipboard insert transactions are serialized and clipboard contents are never logged.

The clipboard payload transfer deadlines do not bound `wl-clipboard-rs`'s synchronous Wayland connection, registry, and request-setup roundtrips. A stalled compositor can still block an insert transaction during those setup calls.

Insert delivery requires a single-seat Wayland session. `wl-clipboard-rs` can read from an unspecified selected seat and publish to all seats, but its public API does not expose the selected seat name so Dictate can restore that exact seat. Dictate makes no multi-seat clipboard safety claim.

For insert delivery, Dictate captures the focused window when the stopping `record stop` or `record toggle` command starts, then checks focus again immediately before insertion. It submits text only when both observations identify the same compositor window. Changed or unverifiable focus retains the transcript and tries ordinary clipboard delivery; run `dictate paste` after choosing a destination. Niri window IDs are supported now. Window identity still cannot prove that a text field accepted the paste, and a narrow check-to-launch race remains because compositor IPC, clipboard ownership, and `wtype` are separate processes.

`model` selects the final-text model; any catalog entry works. `partials_model` selects the live-preview model and must be streaming. Current model ids are `fast-conformer-ctc-en-80ms-int8` (default partials), `parakeet-unified-0.6b-int8-streaming-560ms` (streaming, heavier), `whisper-tiny-en`, `whisper-tiny`, `whisper-base-en`, `whisper-base`, `whisper-small-en`, `whisper-small`, `whisper-medium-en`, `whisper-medium`, `parakeet-tdt-0.6b-v2-int8`, `parakeet-tdt-0.6b-v3-int8`, `parakeet-tdt-ctc-110m-int8`, `sense-voice-small-int8`, `moonshine-tiny-en`, `moonshine-base-en`, `moonshine-v2-tiny-en`, and `moonshine-v2-base-en`.

## Development

The repository is a Cargo workspace. `crates/dictate` owns the sole binary and composes the `dictate-dev`, `dictate-desktop`, `dictate-signal`, `dictate-speech`, and `dictate-ui` libraries.

```bash
just run
just check
just test
just fmt
just hawk
```

`just hawk` audits unnecessary public visibility across the closed workspace. It requires `cargo-hawk` 0.1.9 and uses the compiler pinned in `tools/hawk/rust-toolchain.toml`.

Run the model-backed corpus in `crates/dictate-speech/tests/fixtures` with `just test-integration`.

Build and install the development channel as `~/.local/bin/dictate-dev`:

```bash
just install-dev
```

The install recipe also installs the development desktop identity, then installs, enables, and restarts the `dictate-dev.service` systemd user unit. The development build uses its own config at `~/.config/dictate-dev/config.toml`, daemon socket, and Wayland app identity. It shares downloaded speech models with stable builds. It enables the `dictate debug` development harness through the internal `dev-tools` feature. Re-run `just install-dev` after changing the code; the recipe restarts the daemon with the new executable.

If `~/.local/bin` is in Niri's inherited `PATH`, compositor bindings can invoke the client by name:

```kdl
binds {
    Mod+D { spawn "dictate-dev" "record" "toggle"; }
    Mod+Shift+D { spawn "dictate-dev" "paste"; }
}
```

Inspect daemon output with `journalctl --user -u dictate-dev.service -f`.

### Pi auto-submit extension

Install the Pi extension from a Dictate checkout, then run `/reload` in each open Pi session:

```bash
pi install .
# or: just install-pi-extension
```

When Pi receives a bracketed paste while Dictate owns the regular clipboard transaction, the extension adds one Enter key after the paste. Dictation therefore starts an agent turn without a second keypress. Ordinary clipboard pastes remain in Pi's editor for review. The extension fails closed: if `wl-paste` cannot confirm Dictate's private clipboard MIME type within 100 ms, it leaves the text unsubmitted. See `pi-extension/README.md` and `pi-extension/CHANGELOG.md` for package details and release notes.

The build script rejects any attempt to combine the `dev-tools` feature with Cargo's release profile.

## Requirements

- Linux Wayland compositor with layer-shell and `ext-data-control` or `wlr-data-control` support
- Single Wayland seat for `insert` delivery
- Audio input device
- `wtype` for `insert` delivery
- XDG GlobalShortcuts portal implementation for optional push-to-talk
- Rust toolchain from `rust-toolchain.toml`

## License

MIT
