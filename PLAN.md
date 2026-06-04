# GPUI OSD Overlay Plan

## Goal

Prove the dictation OSD can be rebuilt in GPUI as a real Wayland overlay before considering any broader application port.

The proof must use GPUI layer-shell support, not a normal managed window. Keeping iced only for the OSD is out of scope.

## Current decisions

- Use upstream Zed GPUI pinned by git revision because crates.io GPUI did not expose layer-shell support.
- Use `gpui_platform` with the `wayland` feature.
- Build the real implementation in `src/`; use examples only for major isolated risks.
- `App` means GPUI runtime/context, so avoid app-like names for views.
- `Overlay` is the stateful GPUI root view for the layer-shell surface.
- `Panel` is the visual rounded container component.
- Components use the Zed pattern: `#[derive(IntoElement)]`, `RenderOnce`, and `ParentElement` where children are supported.
- Keep the first visual proof intentionally small: the overlay renders only a waveform inside the panel.
- `DictationState` is the shared OSD/app state. `Overlay` owns it; `Waveform` renders from it.

## Current structure

```text
src/
  main.rs              # binary entrypoint
  lib.rs               # module exports
  app.rs               # GPUI runtime + layer-shell window setup
  models.rs            # centralized transcription/VAD model catalog
  overlay.rs           # stateful root view, implements Render
  spectrum.rs          # FFT speech-band analyzer
  state.rs             # shared dictation state model
  prelude.rs           # local GPUI re-exports + h_flex/v_flex
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

This behaves like an OSD on niri instead of being tiled as a normal window.

### Panel

`Panel` renders a transparent full-window root with a rounded content-sized pill. The pill uses padding rather than fixed dimensions.

### Shared state

`DictationState` owns shared spectrum levels. `Overlay` requests a repaint every frame and passes the latest spectrum frame to the waveform.

### Animated waveform

`Waveform` renders a compact row of mirrored rounded vertical bars from live audio spectrum bands. The GPUI prototype reuses the old speech-optimized FFT analyzer shape so the waveform responds to microphone input instead of fake elapsed-time animation.

### Console transcription

The GPUI overlay now starts microphone capture when it is created. A background worker updates shared spectrum levels from the same microphone stream, uses the official `sherpa-onnx` Rust crate, auto-downloads the selected centralized transcription model and VAD model if needed, feeds continuous 16kHz microphone audio through VAD, transcribes completed speech segments, and prints non-empty text to stdout. The current default transcription model is Whisper base.en. The centralized model catalog includes Whisper tiny/tiny.en/base/base.en/small/small.en/medium/medium.en, Parakeet TDT v2/v3 int8, Parakeet TDT-CTC 110M int8, SenseVoice Small int8, Moonshine Tiny/Base English, and Moonshine v2 Tiny/Base English. There is no keyboard toggle and no fixed app/session duration; recording continues for the process lifetime.

## Next phase: dictation processing core

The next hard problem is not platform plumbing. The core application problem is turning raw ASR text into the right final text for the situation.

Build this as a pure Rust pipeline before wiring in insertion, hotkeys, or platform-specific active-app detection:

```text
RawTranscript
  -> deterministic cleanup
  -> spoken punctuation/newline handling
  -> dictionary and replacement rules
  -> command interpretation
  -> mode/profile formatter
  -> optional LLM rewrite
  -> ProcessedDictation
```

Keep raw transcript, intermediate decisions, and final output separate. The ASR engine should produce raw text; Dictate owns formatting, punctuation, replacements, commands, and profile behavior.

### Initial processing types

Add a post-processing module with types shaped around:

- `RawTranscript`
- `DictationContext`
- `DictationMode`
- `CustomDictionary`
- `ReplacementRule`
- `ProcessedDictation`
- `PostProcessor`

Start with deterministic behavior and golden tests. Do not make LLM post-processing mandatory; it should be an optional stage after local rules.

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

- Settings contract and TOML persistence: output mode, audio device, sample rate, preferred model, shortcut, theme, and OSD position.
- Recording lifecycle: idle, recording, transcribing, error, elapsed time, cancellation, and repeated toggles.
- Audio device UX: list devices, choose device, sample-rate options, device test, and audio-level preview.
- Model management UX: list models, preferred model, download/remove, storage info, download progress, and startup preload.
- History/database: list/search/delete/count transcriptions, recording metadata, audio paths, and lazy DB initialization.
- Output behavior: print, copy, insert, and fallback/error reporting.
- Shortcut/trigger behavior: configured shortcut, CLI toggle command, and graceful fallback where global shortcuts are unavailable.
- Tray/window lifecycle: background app, close hides window, tray/menu quit, and single-instance behavior.
- OSD state contract: idle, recording with spectrum/timer, transcribing, error, and top/bottom position.
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

Only after the waveform-first OSD proves the UI shape:

- tune live spectrum response against the old OSD behavior
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
