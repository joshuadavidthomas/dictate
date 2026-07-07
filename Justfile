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

debug-eval:
    cargo run --quiet -- debug --screen overlay --scenario recording-sine --stats json --duration 2s --exit | jq -s -e 'map(select(.type == "frame")) as $frames | map(select(.type == "aggregates")) as $aggregates | ($frames | length) > 0 and ($aggregates | length) == 1 and ($aggregates[0].measured_fps > 0) and ($aggregates[0].frame_count == ($frames | length))'

test *ARGS:
    cargo test {{ ARGS }}

test-integration *ARGS:
    cargo test --features integration --test integration {{ ARGS }}
