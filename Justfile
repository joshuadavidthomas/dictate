set dotenv-load
set unstable

# List all available commands
[private]
default:
    @just --list --list-submodules

build *ARGS:
    cargo build {{ ARGS }}

check *ARGS:
    cargo check {{ ARGS }}

clean:
    cargo clean

clippy *ARGS:
    cargo clippy --all-targets --all-features {{ ARGS }} -- -D warnings

clippy-fix *ARGS:
    cargo clippy --all-targets --all-features --fix {{ ARGS }} -- -D warnings

fmt *ARGS:
    cargo +nightly fmt {{ ARGS }}

run *ARGS:
    cargo run -- {{ ARGS }}

test *ARGS:
    cargo test {{ ARGS }}

test-integration *ARGS:
    cargo test --features integration --test integration {{ ARGS }}
