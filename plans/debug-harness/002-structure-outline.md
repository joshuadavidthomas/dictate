---
type: structure-outline
repo: dictate
branch: main
sha: 0fb2ca21 (wkvnwuzp, atop main 706d760d)
status: accepted
source_design_discussion: plans/debug-harness/001-design-discussion.md (accepted)
---

# Structure Outline: `dictate debug` harness

## Review Status

- **Status:** Accepted (2026-07-06) — maintainer chose to build directly
  from this outline; no final plan artifact
- **Review needed:** none
- **Execution loop:** per phase — worker implements, read-only worker
  verifies, specialist coding-standards review, orchestrator taste review,
  worker commits

## Desired End State

- `dictate debug` opens a normal window; screens come from a
  `DebugComponent` trait + static registry; scenarios cycle by keyboard.
- Overlay preview renders every `DictationPhase` scenario with synthetic
  spectrum feeders through the existing seam.
- Agents drive the real window unattended: `--screen/--scenario`,
  `--stats json`, `--duration`/`--frames`, `--exit`, `--list`, meaningful
  exit codes; live-loop stats feed both the in-window readout and stdout.
- Transcribe bench and `dictate transcribe` share one extracted composition
  helper; bench shows raw vs formatted with timing.

## Implementation Overview

- [ ] Phase 1: Timeboxed rendering spike (capture / `TestAppContext` /
      headless compositor)
- [ ] Phase 2: `dictate debug` skeleton — subcommand, bootstrap, registry,
      `--list`
- [ ] Phase 3: Overlay preview screen — scenarios + spectrum feeders
- [ ] Phase 4: Stats tap + unattended agent drive
- [ ] Phase 5: Transcribe bench — shared composition helper + screen

## Phase 1: Timeboxed rendering spike

Answer the three unverified claims at the pinned gpui rev, empirically:
(1) can `TestAppContext`/`VisualTestContext` render `OverlayView`-style
views headless; (2) can a real window's contents be captured to an image
from inside the app; (3) does the app run under a headless Wayland
compositor (weston or sway headless backend). Timebox: one working session;
inconclusive = "does not survive."

### File Changes

- `.agents/research/formatting-parity-and-debug-harness.md` — write results
  into the open-claims list (both positive and negative), per the design's
  standing-policy note
- `plans/debug-harness/README.md` — record which best-effort tiers Phase 4+
  may add
- Scratch code only; nothing lands in `src/`

### Validation

#### Automated

- [ ] `just check && just test` — spike leaves the tree untouched

#### Manual

- [ ] Each of the three claims has a documented verdict with the exact
      commands/code used

## Phase 2: `dictate debug` skeleton

Vertical slice: subcommand exists, opens its own normal focusable GPUI
window listing registered screens, and `--list` emits machine-readable
screens × scenarios. Registry starts with one stub screen so the trait and
window shell are proven end to end.

### File Changes

- `src/cli.rs` — `Command::Debug` with `--list`, `--screen`, `--scenario`
  (drive flags parsed now, wired in Phase 4)
- `src/debug/mod.rs` — `run(args)` owning `application().run(...)`; normal
  window (titlebar, focusable, keyboard-interactive) — `app::run()` untouched
- `src/debug/registry.rs` — trait + static list:

```rust
pub trait DebugComponent {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn scenarios(&self) -> &'static [&'static str];
    fn preview(&self, scenario: &str, window: &mut Window, cx: &mut App) -> AnyElement;
}

pub fn registry() -> Vec<Box<dyn DebugComponent>>;
```

- `src/main.rs` / `src/lib.rs` — module wiring
- `AGENTS.md` — record the dual-use standing policy (every interactive debug
  affordance ships a headless/agent equivalent), per the accepted design

### Validation

#### Automated

- [ ] `just check`, `just test`, `cargo clippy --all-targets -- -D warnings`,
      `cargo +nightly fmt --check`
- [ ] Unit test: `--list` output parses as JSON and enumerates the registry
- [ ] `dictate debug --list` exits 0; unknown `--screen`/`--scenario` exit
      non-zero with a useful message

#### Manual

- [ ] `dictate debug` opens the window on niri; screens listed; `q`/close
      works

## Phase 3: Overlay preview screen — scenarios + feeders

The registry's first real component. Scenario enum over `DictationPhase` ×
`SpectrumSource`; synthetic feeders write through the existing
`SpectrumLevels` seam exactly as the mic worker does; keyboard cycles
scenarios. Non-Recording phases show `DictationPhase::label()` text beside
today's panel+waveform (DQ2: placeholder dies when plan 005 lands).

### File Changes

- `src/debug/screens/overlay.rs` — `OverlayScenario` enum
  (`Idle / RecordingSine / RecordingConstant / RecordingFrames /
  Transcribing / Unavailable`), string-id mapping, `DebugComponent` impl
- `src/debug/feeders.rs` — `SpectrumSource` (silent, constant, sine sweep,
  recorded frames) driving `SpectrumLevels::set` on a timer
- `src/debug/registry.rs` — replace stub with the real component

### Validation

#### Automated

- [ ] Standard four commands
- [ ] Unit tests: scenario ↔ string-id round-trip is exhaustive (a new enum
      variant fails the test until listed); each scenario resolves a phase +
      spectrum source
- [ ] Feeder unit test: sine sweep produces bands in [0,1] with expected
      period

#### Evals / Regression Checks

- [ ] `dictate daemon` overlay behavior unchanged (no production file edits
      in this phase beyond none — assert via diff scope)

#### Manual

- [ ] Cycle all scenarios in-window; waveform animates under sine/constant;
      phase labels correct

## Phase 4: Stats tap + unattended agent drive

The DQ3 contract. A stats data model tapped from the live render loop feeds
an in-window readout and, when `--stats json` is set, a stdout stream;
`--duration`/`--frames` + `--exit` make the run unattended.

### File Changes

- `src/debug/stats.rs` — per-frame record (scenario id, target vs smoothed
  bands, gate state, frame delta) + aggregates (frame count, real fps,
  dropped-tick count); serialization
- `src/debug/screens/overlay.rs` — tap the preview loop; in-window readout
  element
- `src/debug/mod.rs` — wire `--screen/--scenario/--stats/--duration/
  --frames/--exit`; exit codes (0 ran-to-completion, non-zero bad scenario /
  window failure)
- `src/cli.rs` — flag plumbing

### Validation

#### Automated

- [ ] Standard four commands
- [ ] Unit tests: stats aggregation math; JSON schema of the frame record

#### Evals / Regression Checks

- [ ] Scripted agent run (documented in the bundle, run where a compositor
      exists): `dictate debug --screen overlay --scenario recording-sine
      --stats json --duration 2s --exit | jq` — stream parses, fps > 0,
      exit 0. Automate as a `just` recipe or display-gated test per the
      Phase 1 headless-compositor verdict; otherwise it stays a documented
      manual eval

#### Manual

- [ ] Interactive readout matches the JSON stream for the same scenario

## Phase 5: Transcribe bench — shared helper + screen

Extract the composition `cli::transcribe_wav` uses into a shared helper;
rewire the CLI onto it (plus `--json`); add the bench screen browsing
`tests/fixtures/**` WAVs and showing raw vs formatted with timing.

### File Changes

- `src/eval.rs` (name per taste) — `transcribe_file(path, model_override,
  settings) -> Result<BenchResult { raw, formatted, timing }>`
- `src/cli.rs` — `transcribe_wav` rewires onto the helper; add `--json`
- `src/debug/screens/bench.rs` — fixture list + file entry, side-by-side
  raw/formatted + timing, `DebugComponent` impl (scenarios = fixture
  corpora)
- `src/debug/registry.rs` — register the bench

### Validation

#### Automated

- [ ] Standard four commands
- [ ] `dictate transcribe <fixture> --json` parses; `--raw`/formatted output
      byte-identical to pre-refactor (characterize before rewiring)
- [ ] Helper unit test against a fixture WAV (model-gated like
      `tests/integration.rs` if needed)

#### Evals / Regression Checks

- [ ] `just test-integration` corpus gate still green

#### Manual

- [ ] Bench renders a spoken-commands fixture raw vs formatted; timing shown

## Open Questions

- None blocking. Phase ordering note: Phases 3–5 all touch
  `src/debug/registry.rs`; execute sequentially, not in parallel. If Phase 1
  finds capture or render-tests viable, `--capture`/render tests become a
  Phase 6 added at final-plan time, not retrofitted into Phase 4.

## Stop Gate

Stop here for outline review. Review the vertical slices and validation —
especially Phase 4's display-dependent eval — before I write the executor
plan.
