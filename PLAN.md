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

`dictate` / `dictate daemon` starts a resident daemon that owns the GPUI event loop, microphone capture, model loading, TOML settings, the local Unix control socket, and dictation state. GPUI stays resident with no window while idle. `dictate record start|stop|toggle|cancel` controls a manually bounded dictation session. The daemon opens a GPUI layer-shell overlay only while recording/transcribing and feeds live spectrum frames through an in-process overlay handle; there is no idle transparent overlay. Stopping capture transcribes the captured 16kHz utterance with the official `sherpa-onnx` Rust crate, routes raw text through deterministic dictation text formatting, and delivers non-empty text to stdout or the Wayland clipboard. The current default transcription model is Parakeet TDT 0.6B v2 int8 and is auto-downloaded if needed. The centralized model catalog includes Whisper tiny/tiny.en/base/base.en/small/small.en/medium/medium.en, Parakeet TDT v2/v3 int8, Parakeet TDT-CTC 110M int8, SenseVoice Small int8, Moonshine Tiny/Base English, and Moonshine v2 Tiny/Base English. Bind the compositor/global shortcut to `dictate record toggle`. Insert-at-cursor delivery is still future work.

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
