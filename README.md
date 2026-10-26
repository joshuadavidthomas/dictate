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

On macOS, `dictate permissions` reports Microphone, Input Monitoring, and Accessibility readiness; add `--json` for machine-readable output, `--request-microphone` for the system microphone request, or `--open microphone|input-monitoring|accessibility` to open the corresponding Privacy & Security pane. A bundled app can use `dictate login-item enable|disable|status|open-settings` to manage launch at login through `SMAppService`. `just build-macos-app` builds and signs `target/release/Dictate.app` with the same identity choice as `just dev` (see Development); set `DICTATE_CODESIGN_IDENTITY` to a Developer ID identity for a distributable build. Command Line Tools-only development machines can set `DICTATE_RUNTIME_SHADERS=1` when building the bundle. Building does not install, launch, or register the app.

Platform-neutral focus identity, clipboard transaction safety, insertion outcomes, and delivery policy live in `dictate-desktop`. Native mechanisms and dependencies are isolated in `dictate-desktop-linux`, `dictate-desktop-macos`, and `dictate-desktop-windows`; `dictate` selects exactly one backend with target-specific dependencies.

The app currently provides:

- daemon-controlled Wayland layer-shell overlay
- native GPUI desktop shell with Settings and History navigation
- live microphone waveform from speech-band FFT analysis
- local/offline transcription of GGUF models through `transcribe.cpp` (Metal on macOS, Vulkan on Linux and Windows, CPU fallback)
- centralized model catalog for Whisper, Parakeet, SenseVoice, and Moonshine models
- command-triggered bounded dictation: keep Dictate running, then run `dictate record toggle` to start/stop capture
- optional portal-managed push-to-talk on desktops that implement XDG GlobalShortcuts
- retained last transcript with `dictate paste` for explicit insertion at the current cursor
- deterministic text formatting for cleanup, spoken punctuation, dictionary/replacement rules, modes, and technical terms
- insert, clipboard, or stdout delivery for formatted dictation
- headless WAV transcription with `dictate transcribe <wav> [--raw] [--model <id>]`

Bind your compositor/global shortcut to `dictate record toggle` to start and stop dictation. Bind a second shortcut to `dictate paste` to insert the last completed transcript at the current cursor. Dictate keeps running in the background while idle, with or without its window open. Recording commands never start Dictate: without a running instance they exit 1 at once, so a keybinding never hangs. When automatic insertion is skipped or safely fails, use `dictate paste` after choosing a destination. A delivery that failed or fell back to the clipboard shows in the overlay for three seconds; `dictate dismiss` hides it sooner. `dictate quit` stops the running daemon and returns once it has exited. Use `dictate daemon --delivery insert|clipboard|stdout` to override configured delivery for that daemon run.

Run `dictate` (or open Dictate from the app launcher) to open the window; when Dictate is already running, this brings its window forward instead of starting a second copy. When nothing is running, it starts Dictate through the installed systemd user unit on Linux, or by opening the app bundle on macOS so permissions belong to Dictate rather than your terminal; otherwise it runs Dictate in the same process. `dictate daemon` always runs in the same process. `dictate daemon --background` starts it without the window, which is what the systemd unit and login items use.

While Dictate runs it shows a tray icon: a StatusNotifierItem on Linux (Waybar, KDE, and other SNI hosts) and a menu bar icon on macOS. Its menu opens the window or Settings, starts and stops dictation, and quits; the icon is Dictate's bars, grey (or the menu bar's color on macOS) while idle and orange while recording or transcribing. Closing the window leaves Dictate running; quit from the tray, the window's `.desktop` Quit action, or `dictate quit`. On macOS, `show_in_dock = false` in `config.toml` (or `dictate config set-show-in-dock false`) keeps Dictate in the menu bar only: no Dock icon, and it starts without its window; it applies after Dictate restarts. Windows has no tray icon yet.

To run the CLI that ships inside `Dictate.app`, use Settings → Command line → Install, or `dictate install-cli` (`--check` reports without changing anything). It links the binary into `~/.local/bin` and, when that folder is not on PATH, adds one marked line to your shell's startup file (`~/.zshrc`, `~/.bash_profile` or `~/.bashrc`, or a fish `conf.d` file). It never replaces another file of the same name. The Dictionary page adds and removes words and replacements, the Style page picks the writing style, and Settings picks the push-to-talk shortcut, microphone, delivery, model, and writing style; each change is saved to `config.toml` at once.

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

Dictate loads settings from `~/.config/dictate/config.toml`, and a running daemon reloads the file within a second of it changing, from any editor. The dictionary, replacements, `mode`, `spoken_formatting`, `recognizer`, and `[plugins.<id>]` tables apply at once; `model`, `backend`, `threads`, `delivery`, `duck_audio`, `input_device`, and `[shortcuts]` apply after Dictate restarts, and the log says so. A file that does not load leaves the last good settings running and logs why.

The window and `dictate config` change one setting at a time without disturbing the rest of the file: comments, order, and anything another program added are kept. It refuses an edit that would leave the settings invalid and writes nothing. `--json` prints the file it wrote.

```bash
dictate config add-dictionary "gee pee you eye" GPUI   # or remove-dictionary "gee pee you eye"
dictate config add-replacement "my email" josh@joshthomas.dev
dictate config set-mode technical
dictate config set-delivery insert
dictate config set-model whisper-base-en-q8
dictate config set-microphone <id>                    # an id from `dictate devices`; omit it for the system default
dictate config set-shortcut '<Command><Shift>d'
```

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

`recognizer` (default `local`) names the speech-to-text engine dictation uses, from the recognizers plugins register; `local` is the bundled `recognizer` plugin, which runs `model` through transcribe.cpp. `mode` accepts `raw`, `literal`, `message`, `email`, `note`, `technical`, or `command`. `spoken_formatting` accepts `disabled`, `punctuation-only`, or `punctuation-and-lines`. `delivery` accepts `insert`, `clipboard`, or `stdout`.

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

Dictate loads Lua 5.4 plugins from `plugins/*.lua` next to `config.toml` (for example `~/.config/dictate/plugins/`). Plugins are trusted code that runs inside the daemon; Dictate does not sandbox them or time them out. Dictation itself is a plugin: with no working `dictation.lua`, pressing the shortcut does nothing, and the load error is in the daemon log and in `dictate plugins`. Plugin code that never returns, such as an endless loop or a blocking `os.execute`, stops every plugin, dictation included, until the daemon restarts; a recording it left open still ends at the 10-minute limit. A plugin's `print`, `io.write`, and child processes write to the daemon log.

Saving, adding, or deleting a plugin file reloads plugins within a second. Plugins before the changed one in load order stay loaded; the changed one and every one after it are unloaded and loaded again in order. Unloading a plugin removes everything it registered, cancels the recording it has open, and kills the commands it is waiting on. A file that fails to load leaves its previous version running, and is not run again until it changes. A transform that errors, or returns something other than a list of entries, disables its plugin until its file changes; the lists are rebuilt without it. A handler that errors fails only that call. A `transcript` or `deliver` hook that errors fails the dictation: later hooks do not run, nothing is delivered, and the overlay shows that delivery failed.

```lua
-- ~/.config/dictate/plugins/sign.lua
dictate.dictionary.transform(function(entries)
  table.insert(entries, { spoken = "rust lang", written = "Rust" })
end)

dictate.on("transcript", function(e)
  -- e.raw is the recognizer output and e.mode is the configured mode.
  e.text = e.text .. " (dictated)"
end)
```

- `dictate.on("transcript", fn(event))` runs on every utterance after the formatter. Hooks run in plugin order on one shared `event`, so each sees what earlier ones changed. Set `event.text` to change the text. `event` also has `raw` (the recognizer's text), `mode`, and, for insert delivery, `focus` for the window that had focus when recording stopped; `focus.app` has its `id` and `title`.
- `dictate.on("deliver", fn(event))` runs just before text is delivered, the same way, on `{ text, target, focus }`. Change `event.text` or `event.target`, or set `event.text = ""` to skip delivery.
- Dictation is the bundled `dictation.lua` plugin: it records on the push-to-talk shortcut and on `dictate record start|stop|toggle|cancel`, transcribes the recording, runs the transcript hooks, and delivers. A `dictation.lua` of your own in the plugins directory replaces it.
- `dictate.on("hotkey", fn(event))` runs on every press and release of a named hotkey, each call in its own coroutine. `event` has `name`, `phase` (`"down"` or `"up"`), and, on release, `focus`. `dictate` is the push-to-talk shortcut; `dictate hotkey <name> down|up` sends any name, so compositor bindings with press and release events can drive both push-to-talk and plugin hotkeys. `dictate.on("record", fn(event))` gets `dictate record <command>` as `{ command, target, focus }`, where `target` is `--delivery`.
- `dictate.session.open()` starts recording from the microphone and returns a session, or `nil, message` when Dictate is already recording or transcribing. `session:stop(focus)` stops recording and returns the audio (empty when nothing was captured) and the focus when recording stopped; pass a release's `focus` to deliver to the window the key went up in. `session:cancel()` drops the audio. When a recording ends on its own, for example at the length limit, `dictate.on("session", fn(event))` gets `{ id = session.id, kind = "ended" }`, and `session:stop()` returns its audio at once.
- Audio is 16 kHz mono samples from -1 to 1: `#audio` is the number of samples, `audio[i]` reads or writes sample `i` (from 1), `audio.rate` is 16000, `audio:slice(from, to)` copies a range, and `audio:wav()` writes a 16-bit WAV for engines that take a file and returns its path; the file is deleted when the hook or handler that wrote it ends. `dictate.transcribe(audio)` runs the `audio` hooks and then the configured recognizer, and returns its text. `dictate.on("audio", fn(event))` hooks get `{ audio }`: edit the samples or assign other audio, such as a slice. A Lua loop over every sample of 10 s of audio takes about 40 ms.
- `dictate.capture.open({ source = "mic" | "system" })` records the microphone, or what the computer plays, apart from dictation and alongside it, until closed; it returns a capture, or `nil, message` when the device could not open. `capture:read(seconds)` waits until that many seconds of new audio are buffered (default 1; 0 takes what is there) and returns everything since the last read, or `nil, reason` once the capture ended and everything was read. `capture:close()` stops it. A capture keeps at most 10 minutes of unread audio, and closes when its plugin unloads. `system` is the default output device's loopback: the default sink's monitor on Linux, a Core Audio process tap on macOS 14.6+, and WASAPI loopback on Windows. `dictate.data_dir()` returns the plugin's own directory for files it keeps, creating it.
- `dictate.recognizers.transform(fn(list))` adds recognizers: `table.insert(list, { id = "my-engine", transcribe = function(audio) return text end })`, then set `recognizer = "my-engine"`. `transcribe` may wait on `dictate.exec`. `dictate.speech.decode(audio)` is the core's transcribe.cpp engine, which the bundled `recognizer` plugin registers as `local`. The daemon loads `model` at startup only when `recognizer` is `local`; otherwise it loads, and downloads, it the first time something calls `dictate.speech.decode`.
- `dictate.transcript.run(text, { focus = focus })` runs the formatter and every `transcript` hook. `dictate.desktop.focus()` returns the focused window, and `dictate.desktop.deliver(text, { target = "insert", focus = focus })` delivers text the way Dictate delivers dictation, falling back from `insert` to the clipboard when focus moved; it returns where the text went. `dictate.overlay.hide()` hides the overlay.
- `dictate.dictionary.transform(fn(entries))` and `dictate.replacements.transform(fn(entries))` edit the `[[dictionary]]` and `[[replacements]]` lists from `config.toml`. Every load rebuilds each list from config and runs each transform once, in plugin order. A transform may edit `entries` in place or return a new list, and each entry must be `{ spoken = ..., written = ... }`. `dictate.dictionary.entries()` and `dictate.replacements.entries()` return the lists as of the last rebuild, after every transform.
- `dictate.exec({ program, args... }, { timeout_ms = 10000, stdin = "...", cwd = "..." })` runs a command and returns `{ code, stdout, stderr, timed_out }`, or `nil, message` if it could not start. Dictate enforces the timeout (default 10 s, up to 120 s) and kills the command and everything it started when it runs out; on Unix the command gets its own process group, and processes it leaves behind are killed when it exits. The hook waits for the command without holding up other plugin work. Call it from the hook itself, not from a coroutine the plugin created; that raises an error. `code` is `nil` when the command was killed.
- `dictate.json.encode(value)` and `dictate.json.decode(text)` convert between Lua values and JSON; JSON `null` decodes to `nil`.
- A `[plugins.<id>]` table in `config.toml` is passed to the plugin file named `<id>` as its argument: `local options = ...`. `enabled = false` in it turns the plugin off, bundled ones included: `[plugins.agent]` with `enabled = false` removes agent cleanup, and the same for `dictation` removes dictation.
- `dictate.ui.page(id, title, render)` adds a page to Dictate's window, listed in the sidebar under `title`. `render(query)` runs each time the page is shown and returns `{ groups = { { title, rows = { { text, detail, meta } } } }, empty }`: rows under headings, with an optional quieter `detail` line and a short `meta` note beside each row. `query` has `search` and `limit`. `render` runs to completion, so it may read files but not wait on commands or the core; if it errors, the page shows the error. Page ids are unique: a plugin that adds an id another already took fails to load.
- `dictate.text.format(text)` runs the built-in formatter with the current mode, dictionary, and replacements. `dictate.log(message)` writes to the daemon log.

The bundled plugins load in this order: `recognizer`, `openai`, `formatter`, `agent`, `dictation`, `meeting`, `history`. A plugin file with one of those names replaces the bundled one. Other plugins run after them in file-name order.

#### Cloud transcription

The bundled `openai` plugin registers an `openai` recognizer that sends each recording to an OpenAI-compatible `/audio/transcriptions` endpoint with `curl`. Nothing is sent until you select it:

```toml
recognizer = "openai"

[plugins.openai]                    # all optional
model = "gpt-transcribe"
url = "https://api.openai.com/v1"   # Groq, a self-hosted server, ...
key_env = "OPENAI_API_KEY"          # "" for a server without a key
timeout_ms = 60000
keywords = true                     # default: on for gpt-transcribe models
```

With `keywords` on, the written form of every `[[dictionary]]` entry, including ones plugins add, is sent as a keyword hint so names and jargon come back spelled your way. Other models may reject the field, so it is off for them unless you set `keywords = true`. The key is read from the environment variable when a recording is transcribed and passed to `curl` on stdin, never on its command line. With another recognizer selected, the local model is not loaded or downloaded until something asks for it. If the request fails, the dictation fails and the overlay says so.

#### Meetings

The bundled `meeting` plugin records a meeting on the `meeting` hotkey: `dictate hotkey meeting down` starts it and the next one stops it. It captures the microphone and what the computer plays, transcribes each every `segment_seconds` with the configured recognizer, and appends the text to a Markdown file in its data directory (`~/.local/share/dictate/plugins/meeting/` on Linux), labeled by source and time:

```markdown
**You** [00:00:05]: Not at this particular case, Tom.

**Call** [00:00:10]: For the twentieth time that evening the two men shook hands.
```

```toml
[plugins.meeting]          # all optional
segment_seconds = 15
silence = 0.01             # quieter segments are not transcribed
you = "You"
call = "Call"
```

Wear headphones: through speakers the microphone hears the call too, and its words appear twice. Dictation keeps working during a meeting, and what you dictate is in the meeting's transcript too. Segments are cut at fixed times, so a sentence that crosses a cut is split there and a word on the cut may be misheard. Quiet segments are skipped, so a `recognizer = "openai"` meeting does not pay to transcribe silence. If system audio cannot be captured, the meeting records the microphone alone and the log says why.

#### History

The bundled `history` plugin keeps every dictation, as the recognizer heard it and as it was typed after every hook, with the time and the window it went to. It appends one JSON line per dictation to `history.jsonl` in its data directory (`~/.local/share/dictate/plugins/history/` on Linux) and adds a History page to Dictate's window, newest first and grouped by day; Home shows the latest few. Nothing leaves the computer. To keep nothing:

```toml
[plugins.history]
enabled = false
```

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

The prompt asks the agent to fix punctuation, capitalization, and recognition errors without adding or answering anything, styled for the mode and naming the focused app. `raw` and `literal` modes are never sent. If the agent fails, times out, or prints nothing, the formatted text is delivered unchanged. `dictate plugins [--json]` loads every plugin, reports errors, and exits non-zero if any plugin failed. `dictate plugins --types` writes lua-language-server definitions, generated from the same declarations that install the API, to `plugins/types/dictate.lua`, plus a `.luarc.json` that uses them if you don't have one, so editors complete and check `dictate.*`. `dictate transcribe` runs the same audio hooks, recognizer, and transcript hooks as the daemon; `--raw` prints the recognizer's text.

## Development

The repository is a Cargo workspace. `crates/dictate` owns the sole binary and composes the `dictate-dev`, `dictate-desktop`, `dictate-signal`, `dictate-speech`, and `dictate-ui` libraries.

The toolchains and tools come from [devenv](https://devenv.sh/), so you need [Nix](https://nixos.org/download/) and devenv. [direnv](https://direnv.net/) is optional: with it, `direnv allow` loads the environment whenever you `cd` into the repository. Without it, run `devenv shell`. On macOS, Xcode still compiles the native code and the app bundle, so you also need Xcode and its Metal Toolchain (`xcodebuild -downloadComponent MetalToolchain`).

Run `just` to see the development commands, and `devenv test` to run everything CI checks.

```bash
just run
just check
just test
just fmt
just hawk
```

`just hawk` audits unnecessary public visibility across the closed workspace. `devenv.nix` pins the Hawk version and runs it with the compiler pinned in `tools/hawk/rust-toolchain.toml`. Keep those two pins paired when upgrading Hawk.

Run the model-backed corpus in `crates/dictate-speech/tests/fixtures` with `just test-integration`.

Rebuild the development channel, Dictate Dev, and swap it for the running instance:

```bash
just dev
```

On Linux, `just dev` runs `just install-dev`, which builds `~/.local/bin/dictate-dev`. The install recipe also installs the development desktop identity, then installs, enables, and restarts the `dictate-dev.service` systemd user unit. The development build uses its own config at `~/.config/dictate-dev/config.toml`, daemon socket, and Wayland app identity. It shares downloaded speech models with stable builds. It enables the `dictate debug` development harness through the internal `dev-tools` feature. Re-run `just dev` after changing the code; the recipe restarts the daemon with the new executable.

On macOS, `just dev` builds `Dictate Dev.app` with its own bundle identifier, quits the running copy with `dictate-dev quit`, replaces `~/Applications/Dictate Dev.app`, and opens it. macOS keeps Microphone, Accessibility, and Input Monitoring grants only while the signature stays the same, so the bundle is signed with `DICTATE_CODESIGN_IDENTITY` when set, else the first Apple Development certificate in the keychain (any Apple ID can create one in Xcode under Settings → Accounts), else a self-signed "Dictate Local Dev" certificate. Create that one once with `just dev-cert`; it needs no Apple account. Without any of them the build is ad-hoc signed and macOS asks for permissions again after every rebuild.

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
- Nix and devenv to build from source (see [Development](#development))

## License

MIT
