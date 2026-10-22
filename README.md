# Dictate

Voice-to-text dictation for modern Linux desktops.

Many desktop dictation apps start on macOS or spread their early work across every operating system. Dictate is starting with modern Linux and Wayland. Systemd runs the daemon, compositor hotkeys control recording, Wayland focus guards text insertion, and local models transcribe speech. GPUI leaves room for other platforms later without thinning out the Linux integration today. The app adds a layer-shell overlay, live audio visualization, and text formatting for spoken punctuation, messages, notes, and technical work.

## Current state

The shipped app lives in `crates/dictate`; its speech engine and transcription fixtures live in `crates/dictate-speech`.

### Platform status

- **Linux/Wayland** is the supported, installable platform. It uses layer-shell overlays, Niri focus identity, Wayland clipboard transactions, `wtype`, XDG GlobalShortcuts, and PulseAudio-compatible audio ducking.
- **macOS** is the next platform and currently has source-level desktop integrations for GPUI overlays, AppKit clipboard transactions, Accessibility focus identity and selected-text insertion, uncertain Quartz input fallback, Quartz push-to-talk event taps, permission diagnostics, and conflict-safe CoreAudio ducking. Speech capture uses CoreAudio. Launch-at-login service wiring, polished onboarding, release signing/notarization, and end-to-end runtime validation remain before it is a supported release.
- **Windows** has source-level desktop integrations for foreground-window focus identity, Win32 clipboard transactions that stay out of clipboard history, `SendInput` paste and Unicode typing fallback, low-level keyboard-hook push-to-talk, non-activating topmost GPUI overlays, and conflict-safe Core Audio endpoint ducking. Speech capture uses WASAPI, and the daemon socket uses Windows' native Unix domain sockets. Startup registration, packaging, and a windowless resident daemon remain before it is a supported release. Text cannot be inserted into elevated windows from an unelevated daemon.

The macOS build needs a full Xcode installation for GPUI's normal ahead-of-time Metal shader build. Developers with Command Line Tools only can check it with GPUI's `runtime_shaders` feature, but that is not the release packaging policy.

On macOS, `dictate permissions` reports Microphone, Input Monitoring, and Accessibility readiness; add `--json` for machine-readable output, `--request-microphone` for the system microphone request, or `--open microphone|input-monitoring|accessibility` to open the corresponding Privacy & Security pane. A bundled app can use `dictate login-item enable|disable|status|open-settings` to manage launch at login through `SMAppService`. `just build-macos-app` builds and ad-hoc signs `target/release/Dictate.app`; set `DICTATE_CODESIGN_IDENTITY` to use a stable Developer ID identity instead. Command Line Tools-only development machines can set `DICTATE_RUNTIME_SHADERS=1` when building the bundle. Building does not install, launch, or register the app.

Platform-neutral focus identity, clipboard transaction safety, insertion outcomes, and delivery policy live in `dictate-desktop`. Native mechanisms and dependencies are isolated in `dictate-desktop-linux`, `dictate-desktop-macos`, and `dictate-desktop-windows`; `dictate` selects exactly one backend with target-specific dependencies.

The app currently provides:

- daemon-controlled Wayland layer-shell overlay
- native GPUI desktop shell with Settings and History navigation
- live microphone waveform from speech-band FFT analysis
- local/offline transcription of GGUF models through `transcribe.cpp` (Metal on macOS, Vulkan on Linux and Windows, CPU fallback)
- centralized model catalog for Whisper, Parakeet, SenseVoice, and Moonshine models
- command-triggered bounded dictation: keep `dictate daemon` running, then run `dictate record toggle` to start/stop capture
- optional portal-managed push-to-talk on desktops that implement XDG GlobalShortcuts
- retained last transcript with `dictate paste` for explicit insertion at the current cursor
- deterministic text formatting for cleanup, spoken punctuation, dictionary/replacement rules, modes, and technical terms
- insert, clipboard, or stdout delivery for formatted dictation
- headless WAV transcription with `dictate transcribe <wav> [--raw] [--model <id>]`

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. Bind a second shortcut to `dictate paste` to insert the last completed transcript at the current cursor. The daemon keeps GPUI running in the background with no window while idle. When automatic insertion is skipped or safely fails, use `dictate paste` after choosing a destination. Persistent recovery notices can be cleared with `dictate dismiss`. Use `dictate daemon --delivery insert|clipboard|stdout` to override configured delivery for that daemon run.

Run `dictate window` to open the desktop window through the resident daemon, starting it when needed. The first slice displays the loaded settings read-only and provides the empty History surface; settings persistence and transcription history storage are not wired yet.

Manual recordings auto-stop after 10 minutes to cap memory growth. When recording stops, `model` (default `bark-3.0-clean-q8`) decodes the whole recording into the final text; there is no live transcript while you speak. The default is Underdog Bark 3.0 Clean as a Q8_0 GGUF run by transcribe.cpp. It drops filler words and false starts, downloads 740 MB on first use, and decodes a 10-second dictation in about 1 second on three CPU threads. Every catalog model is also available to `dictate transcribe` for headless evaluation.

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
model = "bark-3.0-clean-q8"
backend = "auto"
# threads = 4
mode = "technical"
spoken_formatting = "punctuation-and-lines"
delivery = "clipboard"
duck_audio = 0.2

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my email"
written = "josh@joshthomas.dev"
```

`mode` accepts `raw`, `literal`, `message`, `email`, `note`, `technical`, or `command`. `spoken_formatting` accepts `disabled`, `punctuation-only`, or `punctuation-and-lines`. `delivery` accepts `insert`, `clipboard`, or `stdout`.

`duck_audio` sets the fraction by which Dictate lowers the default output sink while the microphone is live. It defaults to `0.2`, accepts values from `0.0` through `1.0`, and can be set to `0` to leave system audio unchanged. Dictate restores the sink that it lowered when recording ends. If you change that sink's volume during recording, Dictate keeps your new level instead. Dictate's microphone host and audio ducking both use a PulseAudio-compatible server such as `pipewire-pulse`. A ducking-specific query, update, or recovery failure leaves recording active at the current output volume.

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

`model` selects the transcription model. Every entry is one Q8_0 GGUF file pinned to a Hugging Face commit. Current model ids are `bark-3.0-clean-q8` (default), `bark-3.0-q8`, `bark-3.0-plus-q8`, `bark-3.0-pro-q8`, `parakeet-tdt-0.6b-v2-q8`, `parakeet-tdt-0.6b-v3-q8`, `parakeet-tdt-ctc-110m-q8`, `whisper-tiny-en-q8`, `whisper-tiny-q8`, `whisper-base-en-q8`, `whisper-base-q8`, `whisper-small-en-q8`, `whisper-small-q8`, `whisper-medium-en-q8`, `whisper-medium-q8`, `sense-voice-small-q8`, `moonshine-tiny-q8`, and `moonshine-base-q8`.

`backend` picks where the model runs: `auto` (default) uses a GPU when one is registered and falls back to the CPU, `cpu` never loads a GPU driver, and `gpu` requires Metal on macOS or Vulkan elsewhere and fails to load without one. `threads` sets how many CPU threads decode; it defaults to one per core but one, since decode time scales almost linearly with threads. The daemon logs the device and thread count it loaded with, and `dictate transcribe --backend auto|cpu|gpu --threads N --json` reports them as `device` and `threads`. The daemon also decodes a second of silence after loading, so the first dictation does not pay for graph setup or GPU shader compilation. Building on Linux needs Vulkan headers 1.3.261 or newer, `glslc`, and `spirv-headers`; building on Windows needs the Vulkan SDK; building on macOS needs the Metal toolchain (`xcrun metal`, from Xcode or Apple's separate Metal Toolchain download).

### Plugins

Dictate loads Lua 5.4 plugins from `plugins/*.lua` next to `config.toml` (for example `~/.config/dictate/plugins/`). Plugins are trusted code; Dictate does not sandbox them. It only protects itself. Plugins run in a separate `dictate plugin-host` process that the daemon starts from its own executable. Each plugin's top-level code gets 1 second, and each hook or transform call gets 250 ms unless the hook asks for more. A call that fails or overruns is skipped, and the step before it is kept. An overrun is suspended rather than raised as an error, so a plugin's own `pcall` cannot keep it running. The budget is checked between Lua instructions, so it cannot stop time spent inside one C call (a huge `string.rep`), inside a callback that C code is running (a `string.gsub` replacement function, a `table.sort` comparator, a metamethod), or inside a coroutine the plugin created itself; the plugin host's deadline below still ends those. If the whole plugin host stops answering, for example because a hook is blocked in `os.execute`, or if it crashes, the daemon kills it, delivers the locally formatted transcript, and starts a fresh host for the next dictation. On Unix the kill also reaches the processes the plugins started, unless one moved itself into a new session or process group (as daemonizing tools do); on Windows only the host itself is killed. Saving, adding, or deleting a plugin file starts a fresh host in the background within a second while the daemon is idle; the old host serves until it is ready, and the next transcript waits for the new one. The host talks to the daemon over a private local socket, so plugins can use `print`, `io.write`, `io.stdout`, and child processes freely: their output goes to the daemon log, and the host's stdin is closed.

```lua
-- ~/.config/dictate/plugins/sign.lua
dictate.dictionary.transform(function(entries)
  table.insert(entries, { spoken = "rust lang", written = "Rust" })
end)

dictate.on("transcript", function(text, ctx)
  -- ctx.raw is the recognizer output and ctx.mode is the configured mode.
  return text .. " (dictated)"
end)
```

- `dictate.on("transcript", fn(text, ctx), { timeout_ms = 10000 })` adds a step to the per-utterance chain. Return the new text, or `nil` to keep it. `ctx` has `raw` (the recognizer's text), `mode`, and, for insert delivery, `app = { id, title }` for the window that had focus when recording stopped. The optional `timeout_ms` (1 to 120000, default 250) is for hooks that do slow work in Lua; time spent in `dictate.exec` does not count against it.
- `dictate.dictionary.transform(fn(entries))` and `dictate.replacements.transform(fn(entries))` edit the `[[dictionary]]` and `[[replacements]]` lists from `config.toml`. Every load rebuilds each list from config and runs each transform once, in plugin order. A transform may edit `entries` in place or return a new list, and each entry must be `{ spoken = ..., written = ... }`.
- `dictate.exec({ program, args... }, { timeout_ms = 10000, stdin = "...", cwd = "..." })` runs a command and returns `{ code, stdout, stderr, timed_out }`, or `nil, message` if it could not start. Dictate enforces the timeout (default 10 s, up to 120 s) and kills the command and everything it started when it runs out; on Unix the command gets its own process group, and processes it leaves behind are killed when it exits. The hook waits for the command without holding up other plugin work, and the wait does not count against its own budget. Call it from the hook itself, not from a coroutine the plugin created; that raises an error. `code` is `nil` when the command was killed.
- `dictate.json.encode(value)` and `dictate.json.decode(text)` convert between Lua values and JSON; JSON `null` decodes to `nil`.
- A `[plugins.<id>]` table in `config.toml` is passed to the plugin file named `<id>` as its argument: `local options = ...`.
- `dictate.text.format(text)` runs the built-in formatter with the current mode, dictionary, and replacements. `dictate.log(message)` writes to the daemon log.

The built-in formatter is a bundled plugin that runs first, followed by the bundled `agent` plugin. A plugin file named `formatter.lua` or `agent.lua` replaces the bundled one. Other plugins run after them in file-name order.

#### Agent cleanup

The bundled `agent` plugin sends the formatted transcript to a coding agent you already have installed and signed in, and uses its reply. It does nothing until configured:

```toml
[plugins.agent]
backend = "pi"                        # "pi", "codex", "opencode", or "command"
model = "anthropic/claude-haiku-4-5"  # optional; the agent's default otherwise
timeout_ms = 15000                    # optional
```

- `pi` runs `pi -p --no-session --no-tools`, and `codex` runs `codex exec --ephemeral --skip-git-repo-check --sandbox read-only -`; neither saves a session.
- `opencode` with `server = "http://127.0.0.1:4096"` calls a running OpenCode server's stateless generate API through `curl`, as Hex does. Without `server` it runs `opencode run`, which saves a session for each dictation.
- `command` with `command = ["program", "args..."]` runs your own program with JSON on stdin (`text`, `raw`, `mode`, `app`, `instructions`, `prompt`) and uses its stdout.
- `program` overrides the executable, for example `pi.cmd` for an npm-installed agent on Windows. `instructions` replaces the default prompt.

The prompt asks the agent to fix punctuation, capitalization, and recognition errors without adding or answering anything, styled for the mode and naming the focused app. `raw` and `literal` modes are never sent. If the agent fails, times out, or prints nothing, the formatted text is delivered unchanged. `dictate plugins [--json]` loads every plugin, reports errors, and exits non-zero if any plugin failed. `dictate plugins --types` writes lua-language-server definitions, generated from the same declarations that install the API, to `plugins/types/dictate.lua`, plus a `.luarc.json` that uses them if you don't have one, so editors complete and check `dictate.*`. `dictate transcribe` runs the same plugin chain as the daemon.

## Development

The repository is a Cargo workspace. `crates/dictate` owns the sole binary and composes the `dictate-dev`, `dictate-desktop`, `dictate-signal`, `dictate-speech`, and `dictate-ui` libraries.

```bash
just run
just check
just test
just fmt
just hawk
```

`just hawk` audits unnecessary public visibility across the closed workspace. Mise installs the Hawk version pinned in `mise.toml`, and the recipe runs it with the compiler pinned in `tools/hawk/rust-toolchain.toml` (installed on first use). Keep those two pins paired when upgrading Hawk.

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

## Linux requirements

- Linux Wayland compositor with layer-shell and `ext-data-control` or `wlr-data-control` support
- Single Wayland seat for `insert` delivery
- Audio input device
- PulseAudio-compatible server, such as `pipewire-pulse`, for microphone capture and system-audio ducking
- `wtype` for `insert` delivery
- XDG GlobalShortcuts portal implementation for optional push-to-talk
- Rust toolchain from `rust-toolchain.toml`

## License

MIT
