# GPUI Overlay Plan

## Goal

Prove the dictation overlay can be rebuilt in GPUI as a real Wayland layer-shell surface before considering any broader application port.

The proof must use GPUI layer-shell support, not a normal managed window. Keeping iced for the overlay is out of scope.

## Current decisions

- Use upstream Zed GPUI pinned by git revision because crates.io GPUI did not expose layer-shell support.
- Use `gpui_platform` with the `wayland` feature.
- Build the real implementation in `src/`; use examples only for major isolated risks.
- `app.rs` owns the resident GPUI event loop and opens the layer-shell overlay on demand. Alias `gpui::App` at imports when needed.
- `OverlayView` is the stateful GPUI root view for the layer-shell surface.
- `Panel` is the visual rounded container component.
- Components use the Zed pattern: `#[derive(IntoElement)]`, `RenderOnce`, and `ParentElement` where children are supported.
- Keep the first visual proof intentionally small: the overlay renders only a waveform inside the panel.
- `SpectrumLevels` is the overlay-local waveform state. The daemon feeds it through the in-process overlay handle; `Waveform` renders from it.

## Current structure

```text
src/
  main.rs              # binary entrypoint
  cli.rs               # command-line parser and dispatch
  lib.rs               # module exports
  app.rs               # resident GPUI event loop and on-demand layer-shell overlay controller
  audio.rs             # headless WAV loading into captured utterances
  daemon.rs            # resident daemon: Unix command socket, command loop, microphone worker, transcription, and delivery
  delivery.rs          # stdout and Wayland clipboard delivery targets
  dictation.rs         # dictation phase/session/control, captured utterance, and sample-rate types
  mic.rs               # CPAL microphone capture, downmixing, resampling, and waveform feed
  models.rs            # centralized transcription/VAD model catalog and local model install
  overlay.rs           # overlay view and overlay-local spectrum state
  settings.rs          # TOML settings load/parse and typed runtime settings conversion
  spectrum.rs          # FFT speech-band analyzer
  text.rs              # deterministic raw-transcript to formatted dictation text
  transcription.rs     # ASR decode and raw transcript filtering
  components.rs        # component exports
  components/
    panel.rs           # rounded container, RenderOnce + ParentElement
    waveform.rs        # compact vertical bar waveform, RenderOnce
```

## Completed proof points

### Layer-shell window

The overlay runs as a Wayland layer-shell surface using:

- `WindowKind::LayerShell`
- `Layer::Overlay`
- `Anchor::BOTTOM`
- `KeyboardInteractivity::None`
- transparent window background

This behaves like a real overlay on niri instead of being tiled as a normal window.

### Panel

`Panel` renders a transparent full-window root with a rounded content-sized pill. The pill uses padding rather than fixed dimensions.

### Overlay state

`SpectrumLevels` owns the overlay spectrum frame. `OverlayView` requests a repaint every frame and passes the latest daemon-fed spectrum frame to the waveform.

### Animated waveform

`Waveform` renders a compact row of mirrored rounded vertical bars from live audio spectrum bands. The GPUI prototype reuses the old speech-optimized FFT analyzer shape so the waveform responds to microphone input instead of fake elapsed-time animation.

### Command-triggered dictation prototype

`dictate` / `dictate daemon` starts a resident daemon that owns the GPUI event loop, microphone capture, model loading, TOML settings, the local Unix control socket, and dictation state. GPUI stays resident with no window while idle. `dictate record start|stop|toggle|cancel` controls a manually bounded dictation session. The daemon opens a GPUI layer-shell overlay only while recording/transcribing and feeds live spectrum frames through an in-process overlay handle; there is no idle transparent overlay. Stopping capture transcribes the captured 16kHz utterance with `transcribe.cpp` (GGUF models), routes raw text through deterministic dictation text formatting, and delivers non-empty text to stdout or the Wayland clipboard. The current default transcription model is Underdog Bark 3.0 Clean Q8_0 and is auto-downloaded if needed. The centralized model catalog includes the four Bark 3.0 variants, Parakeet TDT v2/v3 and TDT-CTC 110M, Whisper tiny through medium (English and multilingual), SenseVoice Small, and Moonshine tiny/base, all as Q8_0 GGUF. Bind the compositor/global shortcut to `dictate record toggle`. Insert-at-cursor delivery is still future work.

## Next phase: dictation lifecycle and text formatting core

The next hard problem is not platform plumbing. The core application problem is turning a bounded dictation utterance into the right final text for the situation.

The old continuous transcription worker was useful for proving local transcription and for future meeting transcription, but it is the wrong default shape for dictation because VAD mistakes, background sounds, and short noisy segments can become text. Primary dictation is now manually bounded:

```text
start dictation
  -> capture microphone samples
stop dictation
  -> transcribe captured utterance
  -> process raw ASR text
  -> deliver final output
```

Build the dictation lifecycle and text formatting as pure Rust before wiring in insertion, hotkeys, or platform-specific active-app detection:

```text
CapturedUtterance
  -> RawTranscript
  -> deterministic cleanup
  -> spoken punctuation/newline handling
  -> dictionary and replacement rules
  -> command interpretation
  -> mode/profile formatter
  -> optional LLM rewrite
  -> ProcessedDictation
```

Keep captured audio, raw transcript, intermediate decisions, and final output separate. The ASR engine should produce raw text; Dictate owns formatting, punctuation, replacements, commands, and profile behavior.

### Initial lifecycle and text types

Add small domain types shaped around:

- `CapturedUtterance`
- `RawTranscript`
- `DictationContext`
- `DictationMode`
- `CustomDictionary`
- `ReplacementRule`
- `ProcessedDictation`
- `DictationFormatter`

Start with deterministic behavior and golden tests. Do not make LLM rewriting mandatory; it should be an optional stage after local rules.

### Dictation versus continuous transcription

Keep these paths distinct:

```text
Dictation:  manual start/stop -> one utterance -> one processed output
Continuous: microphone -> VAD segments -> transcript segments
```

VAD should not be mandatory for primary dictation. It can still be used later for optional silence trimming, hands-free mode, live partials, and meeting transcription.

Meeting transcription should build on the continuous path, not the dictation path:

```text
long audio stream/file
  -> VAD speech regions
  -> optional diarization
  -> ASR segments with timestamps
  -> transcript assembly
  -> summaries/action items
```

### Initial modes

Start with a small mode set:

- `Raw`: trim/normalize only
- `Literal`: preserve words, avoid commands except explicit punctuation if enabled
- `Message`: casual, cleaned prose
- `Email`: polished prose
- `Note`: paragraphs and bullets
- `Technical`: preserve acronyms, product names, and code-ish terms
- `Command`: selected-text or action-oriented transforms later

### Initial deterministic rules

Implement and test local rules first:

- trim and whitespace normalization
- safe spoken punctuation: `comma`, `period`, `question mark`, `colon`, `semicolon`
- line controls: `new line`, `new paragraph`
- custom dictionary terms: spoken phrase -> written phrase
- replacement/snippet rules
- basic sentence capitalization
- filler cleanup in non-literal modes

### Command handling principle

Avoid over-magical destructive command detection in normal dictation. Always-on commands should be safe formatting commands only. Destructive/editing commands like `scratch that`, `delete last sentence`, or `rewrite this professionally` should live in explicit command mode or require stronger context.

### Runtime integration sequence

1. Replace the always-on worker with a command-triggered dictation transcriber.
2. Use `DictationSession` to own the active captured sample buffer.
3. Decode `CapturedUtterance` into `RawTranscript` after stop.
4. Route prototype output through the dictation formatter.
5. Deliver formatted dictation through the configured delivery target.
6. Later wire insert-at-cursor delivery.
7. Keep VAD-only continuous transcription available as a separate future mode for meetings and hands-free use.

### Golden-test examples

Validate the core with fixtures before runtime integration:

- casual message formatting
- email formatting
- literal mode preserving command words
- technical terms like `GPUI`, `sherpa-onnx`, and `Wayland`
- custom dictionary replacements
- spoken punctuation and new paragraphs
- filler removal
- command/literal ambiguity such as `write the words new paragraph`

## Existing app behavior inventory

This is a full rewrite, not a compatibility migration. The Tauri app is only a product-behavior reference: capture the useful behaviors, then implement the clean GPUI version directly. Do not preserve old APIs, old data shapes, webview/IPC boundaries, or iced implementation details unless they still fit the new design.

### Behaviors to keep

- Settings contract and TOML persistence: output mode, audio device, sample rate, preferred model, shortcut, theme, and overlay position.
- Recording lifecycle: idle, recording, transcribing, error, elapsed time, cancellation, and repeated toggles.
- Audio device UX: list devices, choose device, sample-rate options, device test, and audio-level preview.
- Model management UX: list models, preferred model, download/remove, storage info, download progress, and startup preload.
- History/database: list/search/delete/count transcriptions, recording metadata, audio paths, and lazy DB initialization.
- Output behavior: print, copy, insert, and fallback/error reporting.
- Shortcut/trigger behavior: configured shortcut, CLI toggle command, and graceful fallback where global shortcuts are unavailable.
- Tray/window lifecycle: background app, close hides window, tray/menu quit, and single-instance behavior.
- Overlay state contract: idle, recording with spectrum/timer, transcribing, error, and top/bottom position.
- Settings/history UI behavior from Svelte as UX reference, not code to port.

### Rewrite choices

- Design history storage around raw transcript, processed output, processing mode/profile, and applied rule metadata from the start.
- Use typed in-process services instead of Tauri commands.
- Use native Rust/platform services instead of Tauri plugin integrations.
- Use the GPUI layer-shell overlay instead of iced.
- Use the centralized `sherpa-onnx` catalog instead of deprecated `sherpa-rs` model code.
- Use typed settings APIs instead of stringly settings access.
- Treat Tauri frontend screens as product references for GPUI views, not contracts.

### Do not carry forward

- Tauri IPC wrappers and webview event bridge.
- Svelte/SvelteKit frontend code and shadcn components.
- iced/iced_layershell rendering code.
- Tauri plugin-specific clipboard, global-shortcut, dialog, opener, SQL, log, and single-instance glue.
- Window-decoration behavior that does not apply to GPUI.

## Later phases

### Next up: put the app's pieces together

The pieces exist (daemon, main window, overlay, global shortcuts, CLI) but they are wired the way the Linux prototype needed and nowhere else. Dictate started on Arch with niri: a compositor keybinding runs `dictate record toggle`, a systemd user service keeps the daemon alive, and the window was added later as another subcommand. Every other platform inherited that shape:

- Launching `Dictate.app` starts the daemon hidden: no window, and no tray or menu bar icon, so there is nothing to click.
- The main window only opens through `dictate window`.
- The CLI fails unless the daemon is already running; only `dictate window` starts one.
- On macOS it is unclear whether a permission belongs to the CLI or the app bundle.
- The development loop is painful: build, `rm -rf` and copy into `/Applications`, quit, relaunch, and each ad-hoc signed build asks for microphone access again.

Linux (Arch + niri, Wayland) and macOS are both first-class; Windows follows the same shape. There is almost no good dictation app on Linux, so the Linux experience has to be excellent rather than a port. Fixing the macOS shape must not regress the Wayland path: compositor keybindings driving the CLI, the systemd user service, layer-shell overlay, and portals stay supported and polished.

Target shape on every platform: one resident app process that owns the warm model, microphone, global shortcuts, overlay, window, and a tray icon; plus a CLI that drives any part of it and starts the app when it is not running.

- A plain launch (Finder, Dock, app launcher, `.desktop` entry) opens the window; a second launch focuses the running instance over the existing socket. `daemon --background` (or similar) starts hidden for login items and the systemd unit.
- Tray: `NSStatusItem` on macOS, StatusNotifierItem on Linux (Waybar and other SNI hosts), notification area on Windows. Open window, start/stop dictation, settings, quit; shows recording state.
- The CLI auto-starts the app (`open -a` on macOS, the systemd unit or a spawned process on Linux) instead of erroring.
- Settings offers "Install CLI" on macOS/Windows, linking the bundled binary onto PATH. The CLI only talks to the socket, so permissions stay with the app.
- A `just dev` loop that rebuilds and swaps the running instance, and a stable local signing identity (`DICTATE_CODESIGN_IDENTITY`) so macOS permissions survive rebuilds.

Keep the resident process: unlike a screen recorder, dictation needs a warm model and always-on shortcuts, so a standalone per-command CLI is the wrong model.

Reference: Cap (https://cap.so, https://github.com/CapSoftware/Cap, `apps/desktop-gpui`) moved from Tauri to GPUI in 0.6.1. Its app opens a window on launch, has a hand-built `NSStatusItem` tray, ships `cap-cli` inside the bundle with an Install CLI setting, and has a `dev.sh` that rebuilds and swaps the running app. Its CLI links the engine crates and runs standalone, which we deliberately do not copy.

Reference: Hex (https://github.com/anomalyco/hex, read at `d2cff440`) is the closest match: Rust dictation on macOS, Linux (X11 and wlroots Wayland), and Windows, one process per user with the CLI as the same binary. Its UI is now its own `rune` crate (GPUI removed). Worth copying:

- Plain launch (`hex`, `.desktop` `Exec=hex`) forwards `Activate` with `XDG_ACTIVATION_TOKEN` to the running instance or becomes it and shows the window; `hex start` (autostart) stays hidden. Racing launches settle on one instance via a lock per data root. Rationale in `docs/plans/single-process.md`.
- Shortcut commands (`dictation start|stop|toggle|cancel`) never auto-start and fail fast with exit 1, so a keybinding never hangs. They are idempotent for `bindsym --release` setups.
- Wayland shortcuts go through the GlobalShortcuts portal (press and release, so hold-to-talk works; Escape bound only while recording; new portal session on rebind). No `input` group or raw device reading. Settings shows the triggers the desktop actually granted. On niri this is via xdg-desktop-portal-gnome, only verified with synthetic keys.
- systemd user unit `Type=notify` with `READY=1` once the socket binds (before the model loads), `PartOf=graphical-session.target`, and a launcher-written `session.env` carrying Wayland, IME, and locale variables.
- A window failure (no display or GPU) is caught; the process keeps running. Fixed-size window (min = max) so tiling compositors float it.
- No Linux tray, by decision: Quit lives in the window and as a `.desktop` `Actions=quit;` entry. macOS has an `NSStatusItem`; Windows uses `tray-icon`.
- `HEX_APPLICATION_SUPPORT_DIR` isolates dev and test instances from the installed one; scripts test single-instance races and Wayland paste under headless Sway.
- macOS onboarding opens the window until permissions and a model are ready, with a floating guide beside the System Settings pane.

Decisions after reading Hex:

- Linux gets a tray (StatusNotifierItem), unlike Hex.
- Support both Wayland shortcut paths equally: the GlobalShortcuts portal, which is where the ecosystem is heading but is not yet available everywhere, and compositor keybindings driving the CLI. Use the portal by default when the desktop supports it; compositor keybindings stay fully supported, not a degraded fallback.
- Keep CLI parity with the GUI: anything the window can do has a command, so keybindings and scripts can drive all of Dictate.
- Copy the macOS floating permission guide beside the System Settings pane.

### After that: correct-the-sentence loop

Turn a user's correction into dictionary entries, so a mistake fixed once stays fixed:

1. Edit a dictation in place on the History page (and the matching CLI command).
2. Compare the corrected text against what the recognizer heard (the raw transcript), not against the formatted output, so formatting changes are not mistaken for recognition errors.
3. Pick the words: from that comparison, propose dictionary or replacement entries (for example "gee pee you eye" → "GPUI") and let the user accept or reject each. This is the hard part: separating real misrecognitions from rewording, punctuation, and casing changes.

History is a Lua plugin, so the edit and the proposal should go through the plugin API rather than special-casing History in the window.

### Window sizing and placement polish

Revisit layer-shell surface size, margins, and panel alignment after content states are represented.

### Real state integration

Only after the waveform-first overlay proves the UI shape:

- tune live spectrum response against the old overlay behavior
- map app/domain events into the overlay state model
- connect real/fake spectrum source behind a clean seam

### Future idea: active-application profiles

Support Superwhisper-style transcription behavior by detecting the active application and selecting a processing profile. Keep this cross-platform: Wayland compositor IPC on Linux where available, NSWorkspace/Accessibility on macOS, foreground-window APIs on Windows, and X11 window metadata on X11. Do not require focused input-field detection; active application plus optional window title is enough for the first version.

## Validation commands

```bash
cargo check --all-targets
cargo run
```

## Open questions

- Should the waveform stay in a pill panel, or render as bare bars?
- Should the final overlay surface size remain fixed, or resize/reopen around content?
- Does GPUI layer-shell support runtime changes to anchor/margin if settings change?
