# Agent Guidelines

## Commands

Prefer `just` commands over direct `cargo` commands.

- `just run`
- `just check`
- `just test`
- `just fmt`
- `just build`
- `just build --release`

Transcription/formatting behavior is verified headlessly with `dictate transcribe <wav> [--raw] [--model <id>]` against `crates/dictate-speech/tests/fixtures/` audio; prefer it over live-daemon testing.

Every interactive debug affordance ships with a headless/agent-drivable equivalent: CLI flags, machine-readable output, and meaningful exit codes.

## Crate Responsibilities

- `dictate`: sole binary; CLI, daemon orchestration, settings, build identity
- `dictate-debug`: debug window, scenarios, benchmark previews, and stats output
- `dictate-desktop`: focus observation and text delivery through Wayland and `wtype`
- `dictate-signal`: spectrum analysis and waveform smoothing shared by audio and UI
- `dictate-speech`: microphone capture, dictation state, models, recognition, formatting, and evaluation
- `dictate-ui`: production GPUI overlay and reusable views

Keep dependencies pointed toward these owning crates. Do not add shared/common/types crates or re-export supporting-crate interfaces through `dictate`.

## Code Style

- Prefer typed domain seams over stringly configuration or compatibility shims
- Errors: `thiserror` for typed errors when useful, `anyhow` for ad-hoc application errors
- Logging: prefer the `log` crate or `tracing` once logging is wired
- UI: GPUI views implement `Render`; reusable components implement `RenderOnce + IntoElement`
- Components: use `ParentElement` for child slots where needed
