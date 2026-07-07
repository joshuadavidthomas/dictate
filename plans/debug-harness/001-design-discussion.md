---
type: design-discussion
repo: dictate
branch: main
sha: b8166cb6ee4c (parent 706d760d, main)
status: accepted
source_research: .agents/research/formatting-parity-and-debug-harness.md (Track 2)
---

# Design Discussion: `dictate debug` harness

## Summary of Change Request

Add a `dictate debug` subcommand that opens a normal GPUI window (no daemon,
no socket, no layer-shell) offering two things:

1. A **component gallery** with scenario cycling — preview the overlay's
   visual states (`Idle / Recording(spectrum) / Transcribing / Unavailable`)
   with deterministic synthetic data instead of a live mic.
2. A **transcription bench** — pick a WAV from the fixture corpus (or any
   path), run it through `load_wav_utterance → transcribe → DictationFormatter`,
   and show raw vs formatted side by side with timing.

**Hard constraint (user seed): dual-use.** Every scenario reachable by
clicking must be reachable headless — CLI flags, machine-readable output,
meaningful exit codes — so agents get the same feedback loop a human gets.

Source roadmap item: `.agents/ROADMAP.md` Now #1 (`dictate debug` harness).

## Review Status

- **Status:** Accepted (maintainer review, 2026-07-06) — DQ1–DQ3 all
  resolved
- **Review needed:** none
- **Next artifact:** Structure outline

## What Better Means

- Overlay visual states are inspectable in seconds without starting the
  daemon, toggling the socket, or making noise at a microphone.
- The fixture corpus becomes an interactive formatter eval bench: a
  Parakeet-punctuated clip can be eyeballed through the formatter without
  dictating live.
- An agent can drive every scenario a human can, and assert on the output.
- Regression bar: zero changes to daemon/overlay production behavior; the
  harness is additive. `just check / test / clippy / fmt` stay green.

## Standards / Design Pressure

- **modules** (`coding-standards/references/modules.md`): the harness must be
  a *second consumer of existing seams*, not a new abstraction layer. The
  overlay handle (`Overlay` + `SpectrumLevels`) and the transcription
  composition already exist; the design pressure is to feed them, not wrap
  them. Functional-core/imperative-shell applies directly: scenarios are pure
  data, the window is a thin shell that renders them.
- **verification** (`references/verification.md`): "production surface is not
  for tests" — debug affordances live behind the `debug` subcommand in their
  own module, and production types gain constructors/injection points only
  where a scenario genuinely needs them. "Test through real seams" — the
  bench must run the *same* composition as `dictate transcribe`, not a
  parallel reimplementation.

## Reconnaissance Summary

- **Research artifact** (Track 2): Zed deleted storybook (zed#53511); at the
  pinned rev the portable pattern is `crates/component` (~200-line trait +
  `inventory` registry). `component_preview` is Zed-bound; copy ideas only.
  `TestAppContext`/`VisualTestContext` exist at the pinned rev but what
  renders headless is **unverified** — flagged for an empirical spike.
- **Oracle pass over the seams** (verified 2026-07-06):
  - `Overlay` handle: `src/app.rs:27-44` — clone-able, `show()/hide()/send_spectrum()`;
    `SpectrumLevels` is `Arc<[AtomicU32; 8]>` (`src/spectrum.rs:27-53`).
    The overlay window is opened/owned inside `app::run()`'s GPUI task
    (`src/app.rs:53-98`), layer-shell, namespace `dictate-overlay`
    (`src/app.rs:103-130`).
  - **The overlay is phase-blind today.** `OverlayView` renders only
    `Panel` + `Waveform(displayed_bands)` (`src/overlay.rs:91-95`); phase is
    expressed solely as show/hide called from daemon/mic code. `DictationPhase`
    (`Initializing/Idle/Recording/Transcribing/Unavailable`) with `label()`
    strings already exists as public data (`src/dictation.rs:27-74`).
  - Headless transcription composition exists but is private CLI glue:
    `transcribe_wav()` at `src/cli.rs:58-90` composes settings → model →
    `audio::load_wav_utterance` (`src/audio.rs:11`) →
    `transcription::transcribe` (`src/transcription.rs:48`) →
    `DictationFormatter::format` (`src/text.rs:179`).
  - Fixtures: `tests/fixtures/{cmu-arctic,ljspeech,spoken-commands}/`, WAV +
    sibling `.txt`, manifest + lock; corpus gate in `tests/integration.rs:56-122`
    scores raw ASR only (formatter behavior is unit/snapshot territory per
    `tests/fixtures/README.md:14`).

## Current State

- Verifying any overlay visual change requires: start daemon → toggle via
  socket → speak into a mic → watch a 2-second layer-shell window.
- Formatter evaluation against real model output requires `dictate transcribe`
  per file with manual diffing of `--raw` vs formatted runs.
- `Command` enum has `Daemon / Record / Transcribe` (`src/cli.rs:21-45`);
  there is no debug entry point and no way to open the overlay's visuals
  outside the daemon.

## Desired End State

- `dictate debug` opens a normal window listing debug screens; arrow keys /
  clicks cycle scenarios within a screen.
- `dictate debug --list` prints screens × scenarios machine-readably;
  `dictate debug --screen overlay --scenario recording-sine --stats json
  --duration 2s --exit` drives the **real window** unattended: the live
  render loop streams its actual per-frame stats (target vs smoothed bands,
  gate state, frame timing, real fps) plus aggregates as JSON, with
  meaningful exit codes (resolved DQ3).
- Scenario definitions are pure data (`DictationPhase` × spectrum source ×
  bench input), so plan 005's phase rendering plugs into an existing socket
  when it lands.
- The bench and `dictate transcribe` share one extracted composition helper.

## What We're Not Doing

- Time controls (pause/step) — later nicety. (Per-frame stats moved *into*
  scope as the agent-readable frame-stats stream — resolved DQ3.)
- Daemon audio injection (`record start --from-file`) and the socket ack
  protocol — separate roadmap items (agentic-loop ladder rungs 2–3).
- Plan 005 itself (overlay phase *rendering* in production) — the harness
  defines phases as scenario data and previews what exists; see DQ2.
- Porting Zed's `component_preview` machinery or adopting `inventory`.
- Any change to daemon/mic/overlay production behavior.

## Proposed End State Architecture

```
src/cli.rs            Command::Debug { screen, scenario, list, headless flags }
src/debug/mod.rs      run(args) — owns its own gpui application().run(...)
src/debug/registry.rs DebugComponent trait + static registry list (resolved DQ1)
src/debug/screens/    overlay preview, transcribe bench
src/debug/feeders.rs  synthetic SpectrumLevels producers (sine sweep, constant,
                      recorded-frame playback) — second producers into the
                      existing seam, no overlay changes
src/debug/stats.rs    per-frame stats tap on the live loop — one data model
                      feeding both the in-window readout and --stats json
src/eval.rs (or similar) extracted transcribe-file composition shared by
                      cli::transcribe_wav and the bench
```

- The debug window gets its **own** GPUI bootstrap (normal window, focusable,
  keyboard-interactive). `app::run()` stays daemon-specific; generalizing it
  with window-kind flags would be shallow-interface complexity for two
  callers with nothing in common but `application().run`.
- Scenario = data: `(screen, scenario-id, DictationPhase, SpectrumSource,
  Option<bench input>)`. The window shell reads it; the headless path prints
  it (and optionally captures it, per DQ3).

## Design Questions

None open — all resolved below.

## Resolved Design Questions

### DQ3 — Agent contract: drive the real window (**accepted 2026-07-06, revised in maintainer review**)

The dual-use seed requires that an agent can drive every scenario a human
can and get assertable output. This question was first framed as "what does
a *headless* run produce?" — the maintainer corrected the frame: the
constraint is **no human**, not no window. "Headless doesn't have to mean
headless."

**Decision: agents drive the real interactive window — one code path,
nothing simulated.**

- Launch-time flags drive the same window a human uses:
  `dictate debug --screen overlay --scenario recording-sine --stats json
  --duration 2s --exit` (or `--frames N`). The **live render loop** streams
  its actual per-frame stats — target vs smoothed bands, gate state, frame
  timing, real compositor-driven fps — plus aggregates as JSON on stdout,
  with meaningful exit codes. The in-window stats readout and the JSON
  stream read one data model tapped from the same loop, so what the agent
  parses is exactly what a human watches.
- The transcribe-bench screen needs no window at all for its agent form
  (text in, text out → JSON on stdout, shared composition with
  `dictate transcribe`).
- Display-less environments (e.g., CI): run the *same real window* under a
  headless Wayland compositor (weston/sway headless backend) rather than a
  code-level simulation — verify feasibility in the spike below.
- **Best-effort tier (unchanged):** a timeboxed spike (first outline slice)
  answers whether window `--capture` screenshots and/or `TestAppContext`
  render tests work at the pinned rev; whichever survives is added. The
  spike also covers the headless-compositor check.

Decision history:

- **Superseded first amendment: simulated N-frame headless loop** (drive the
  animation math without a window, emit tick stats). Dropped because it
  creates a second animation path that can diverge from the real loop — the
  verification standard says test through real seams — and its frame timing
  was admittedly fake. It remains the fallback only if driving the real
  window proves infeasible in some required environment. The
  `advance_waveform` pure-function extraction returns to being an
  independent, already-roadmapped testing improvement, not this contract's
  backbone.
- **Rejected as primary contract** (both stay as spike targets):
  screenshot capture (GPUI off-screen render/capture on Wayland unverified;
  compositor-side capture fragile) and `TestAppContext` render tests
  (unverified at the pinned rev). Gating the harness on either unverified
  claim would have blocked the guaranteed agent loop on a rendering
  question.

### DQ1 — Registry shape: Zed-shape trait + static list (**maintainer decision, 2026-07-06**)

**Decision: Option A.** A Dictate-local `DebugComponent` trait
(`name/description/scenarios/preview(scenario, window, cx)`) with a
hand-maintained static registry list. Open for extension without touching a
central match, and it matches the verified Zed pattern the research ranked
first. Each component keeps an internal scenario enum and maps it to/from
the string ids the trait exposes, so `--list` enumerates the registry while
scenario dispatch inside a component stays exhaustive.

Rejected options, preserved for history:

- **Option B: `inventory` distributed registration** — what Zed actually
  ships; a linker-magic dependency for two components. Revisit if the
  component count makes the static list annoying.
- **Option C: plain screen/scenario enums** (the original recommendation) —
  deepest module at the current two-screen scale, compiler-enforced scenario
  exhaustiveness end to end. Set aside in maintainer review in favor of the
  trait's open-for-extension shape.

### DQ2 — Sequencing against plan 005: harness first (**accepted 2026-07-06**)

`OverlayView` renders no phase today; plan 005
(`plans/product-direction/005-overlay-phase-states.md`) will change that.

**Decision: harness first, scenarios ahead of rendering.** Scenarios are
defined over `DictationPhase` from day one; until 005 lands, non-Recording
phases preview as the phase `label()` text beside today's panel+waveform
rendering. When 005 makes `OverlayView` phase-aware, the preview swaps to
the real component and the placeholder text dies. The harness's
architectural job is forcing phases to exist as *renderable data*, not as
rendered pixels; 005 then lands with a preview bench already waiting for it.

Rejected: folding the minimal `OverlayView` phase refactor into the harness
(entangles two review scopes the roadmap kept separate); blocking on 005
(inverts the de-risking rationale).

### Separate GPUI bootstrap for the debug window

`app::run()` (`src/app.rs:53-98`) stays daemon/overlay-specific. The debug
subcommand owns its own `application().run(...)` with a normal, focusable
window. Rejected: parameterizing `app::run` over window kind — two callers,
zero shared behavior beyond the runtime call, and the overlay path's
callback-plus-channel shape is daemon-driven noise for the debug window.

### Extract the transcribe composition; don't reimplement it

`cli::transcribe_wav` (`src/cli.rs:58-90`) is private glue over public seams.
The bench must call an extracted shared helper (model selection → load →
transcribe → format, returning raw + formatted + timing), and `transcribe`
CLI re-wires onto it. Rejected: the bench composing the seams itself —
guaranteed drift between what the bench shows and what the CLI ships.

### Spectrum injection needs no new abstraction

Synthetic feeders write through the existing `SpectrumLevels::set` /
`Overlay::send_spectrum` seam (`src/spectrum.rs:45-49`, `src/app.rs:42-44`)
exactly as the mic worker does (`src/mic.rs:330-335`). The debug window is
just a second producer. Rejected: a `SpectrumSource` trait in production
code — the seam already exists.

## Patterns to Follow

### Handle-plus-shared-atomics overlay seam

The overlay is driven through a cheap clone-able handle; feeders should look
like the mic worker, not reach into the view.

- `src/app.rs:27-44` — `Overlay { sender, spectrum }`, `send_spectrum`
- `src/mic.rs:330-335` — the existing producer the sine/constant feeders mimic

### Composition-over-seams CLI glue

- `src/cli.rs:58-90` — settings → model → load → transcribe → format; the
  shape the extracted helper preserves verbatim

### Trait registry with per-component scenario enums (resolved DQ1)

```rust
trait DebugComponent {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn scenarios(&self) -> &'static [&'static str];
    fn preview(&self, scenario: &str, window: &mut Window, cx: &mut App) -> AnyElement;
}

fn registry() -> Vec<Box<dyn DebugComponent>> {
    vec![Box::new(OverlayPreview), Box::new(TranscribeBench)]
}

enum SpectrumSource { Silent, Constant(f32), SineSweep, Frames(&'static [[f32; SPECTRUM_BANDS]]) }

enum OverlayScenario { Idle, RecordingSine, RecordingConstant, Transcribing, Unavailable }

impl OverlayScenario {
    fn phase(self) -> DictationPhase { ... }
    fn spectrum(self) -> SpectrumSource { ... }
}
```

## Standing Policy / Eval Recommendations

- **Dual-use rule as repo policy:** "every interactive debug affordance ships
  with a headless equivalent (flag + machine-readable output + exit code)" —
  this recurs for the daemon audio-injection and socket-ack roadmap items.
  Recommend recording it in `AGENTS.md` (or an ADR) when this design is
  accepted, so future planning inherits it instead of re-deriving it.
- **Spike outcome is durable:** whatever the TestAppContext/capture spike
  learns (works / doesn't, at pinned rev) should be written back to
  `.agents/research/formatting-parity-and-debug-harness.md`'s open-claims
  list either way, so it is never re-researched.

## Stop Gate

Stop here for design review. Review DQ1–DQ3 (registry shape, 005 sequencing,
headless contract) and the non-goals. Do not write the structure outline
until this document is accepted or you explicitly say to proceed.
