# Agent Guidelines

## Commands

Prefer `just` commands over direct `cargo` commands.

- `just run`
- `just check`
- `just test`
- `just fmt`
- `just build`
- `just build --release`

## Code Style

- Prefer typed domain seams over stringly configuration or compatibility shims
- Errors: `thiserror` for typed errors when useful, `anyhow` for ad-hoc application errors
- Logging: prefer the `log` crate or `tracing` once logging is wired
- UI: GPUI views implement `Render`; reusable components implement `RenderOnce + IntoElement`
- Components: use `ParentElement` for child slots where needed
