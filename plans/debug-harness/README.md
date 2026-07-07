# `dictate debug` harness

**Source roadmap item:** `.agents/ROADMAP.md` Now #1 — `dictate debug` harness *(research)*
**Source research:** `.agents/research/formatting-parity-and-debug-harness.md` (Track 2)
**Planned at:** 2026-07-06, working copy `wkvnwuzp` atop `main` `706d760d`
**Status:** ready for execution — building directly from the outline (maintainer decision, 2026-07-06; no final plan artifact)
**Current gate:** none — phase-by-phase execution with per-phase verification, coding-standards specialist review, and commits

## Purpose

Deterministic overlay-state preview and an interactive WAV→transcribe→format
eval bench, without daemon/socket/mic — dual-use so agents get the same loop
via CLI flags and machine-readable output. De-risks plan 005 (overlay phase
states) and the formatter work.

## What Better Means

- Overlay visuals inspectable in seconds, no daemon or mic.
- Fixture corpus browsable through the real transcribe+format composition.
- Every interactive scenario agent-reachable with asserted output.
- Regression bar: no production daemon/overlay behavior changes; standard
  checks stay green.

## Artifact Index

| Artifact | Status | Purpose | Notes |
|---|---|---|---|
| [001-design-discussion](001-design-discussion.md) | Accepted | Decide registry shape, 005 sequencing, agent contract | DQ1–DQ3 resolved 2026-07-06; DQ3 revised to drive-the-real-window |
| [002-structure-outline](002-structure-outline.md) | Accepted | Five vertical slices: spike → skeleton → overlay preview → stats/agent drive → bench | Phases 3–5 share registry.rs; sequential execution; executes without a final plan |

## Current Shape

- `dictate debug` subcommand with its own GPUI bootstrap (normal window);
  `app::run()` stays daemon-specific.
- Zed-shape `DebugComponent` trait + static registry list; components keep
  internal scenario enums mapped to string ids (resolved DQ1).
- Scenarios defined over `DictationPhase` × `SpectrumSource` ahead of plan
  005's rendering (resolved DQ2); synthetic feeders reuse the existing
  spectrum seam.
- Agent contract (resolved DQ3): drive the **real window** unattended —
  launch flags pick screen/scenario, the live render loop streams actual
  per-frame stats (bands, gate state, frame timing, real fps) as JSON with
  exit codes, auto-exit via `--duration`/`--frames`; one data model feeds
  both the in-window readout and the JSON stream. Timeboxed spike covers
  capture, `TestAppContext`, and headless-compositor CI feasibility.
- Bench and `dictate transcribe` share one extracted composition helper.

## Accepted Decisions

- **DQ1 — registry shape:** Zed-shape `DebugComponent` trait + static list
  (maintainer decision, 2026-07-06); per-component scenario enums behind
  string ids.
- **DQ2 — 005 sequencing:** harness first; scenarios defined over
  `DictationPhase` ahead of 005's rendering (accepted 2026-07-06).
- **DQ3 — agent contract:** drive the real interactive window, nothing
  simulated — "no human" not "no window"; live-loop frame-stats JSON +
  auto-exit flags; capture/`TestAppContext`/headless-compositor checks stay
  in the spike (revised in maintainer review 2026-07-06; the earlier
  simulated-frame amendment is superseded and kept only as fallback).

## Open Gates

- None — executing. Phase 1's spike verdicts decide whether a `--capture` /
  render-test Phase 6 gets added.

## Phase 1 spike verdicts

- Render tests: viable as a best-effort Phase 6 tier for structural
  view/interaction coverage with `gpui/test-support`; `TestAppContext` can
  construct `OverlayView`-style windows and `VisualTestContext::draw` can draw
  dictate elements, but Linux pixel screenshots are not available there.
- `--capture`: not viable on Wayland at pinned gpui rev `50d001fe0a38`;
  `App::screen_capture_sources()` reports `Wayland screen capture not yet
  implemented`, and `Window::render_to_image()` is test-support/headless-
  renderer-only.
- CI headless route: GPUI's own Linux `headless()` platform cannot open
  windows; unattended real-window CI needs an external headless Wayland
  compositor. Weston/sway were not installed in the spike environment, so the
  route remains untested here. Candidate for Phase 4: `cage` *is* installed
  and wlroots compositors support `WLR_BACKENDS=headless` — try
  `WLR_BACKENDS=headless cage -- dictate debug ...` once the subcommand
  exists.

## Implementation Routing

Direct execution from the accepted outline (maintainer decision) — the
conversation-driven loop replaces the final-plan artifact: implement →
verify → coding-standards specialist review → taste review → commit, per
phase.

## Rejected or Deferred

| Item | Reason | Revisit if |
|---|---|---|
| `inventory` distributed registration | Linker-magic dep for two screens | Component count makes the static list annoying |
| Plain screen/scenario enums (DQ1-C) | Maintainer chose the trait registry's open-for-extension shape | — |
| Generalizing `app::run` over window kinds | Two callers, no shared behavior | A third window kind appears |
| Time controls (pause/step), stat panels | Later niceties per research | Harness lands and iteration demands them |
| Daemon audio injection / socket acks | Separate roadmap items | Their own planning efforts |
