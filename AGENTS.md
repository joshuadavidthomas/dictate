# Agent Guidelines

## Commands

- **Dev**: `cargo run`
- **Check**: `cargo check --all-targets`
- **Tests**: `cargo test`
- **Format**: `cargo fmt`
- **Build**: `cargo build --release`

## Code Style

### Rust

- Edition 2024
- Prefer typed domain seams over stringly configuration or compatibility shims
- Errors: `thiserror` for typed errors when useful, `anyhow` for ad-hoc application errors
- Logging: prefer the `log` crate or `tracing` once logging is wired
- UI: GPUI views implement `Render`; reusable components implement `RenderOnce + IntoElement`
- Components: use `ParentElement` for child slots where needed
