set dotenv-load
set unstable

# List all available commands
[private]
default:
    @just --list --list-submodules

check *ARGS:
    cargo check {{ ARGS }}

clean:
    cargo clean

clippy *ARGS:
    cargo clippy --all-targets --all-features --benches --fix {{ ARGS }} -- -D warnings

fmt *ARGS:
    cargo +nightly fmt {{ ARGS }}

run *ARGS:
    cargo run -- {{ ARGS }}

test *ARGS:
    cargo test {{ ARGS }}
