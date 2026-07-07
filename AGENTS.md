# Agent Guidelines

## Commands

Prefer `just` commands over direct `cargo` commands.

- `just run`
- `just check`
- `just test`
- `just fmt`
- `just build`
- `just build --release`

Transcription/formatting behavior is verified headlessly with `dictate transcribe <wav> [--raw] [--model <id>]` against `tests/fixtures/` audio; prefer it over live-daemon testing.

Every interactive debug affordance ships with a headless/agent-drivable equivalent: CLI flags, machine-readable output, and meaningful exit codes.

## Code Style

- Prefer typed domain seams over stringly configuration or compatibility shims
- Errors: `thiserror` for typed errors when useful, `anyhow` for ad-hoc application errors
- Logging: prefer the `log` crate or `tracing` once logging is wired
- UI: GPUI views implement `Render`; reusable components implement `RenderOnce + IntoElement`
- Components: use `ParentElement` for child slots where needed
