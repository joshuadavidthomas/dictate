set dotenv-load
set unstable

# List all available commands
[private]
default:
    @just --list --list-submodules

build *ARGS:
    cargo build -p dictate {{ ARGS }}

build-dev *ARGS:
    DICTATE_BUILD=dev cargo build -p dictate {{ ARGS }}

build-release *ARGS:
    DICTATE_BUILD=stable cargo build --release -p dictate {{ ARGS }}

install-dev: build-dev
    mkdir -p "$HOME/.local/bin"
    if [ target/debug/dictate -ef "$HOME/.local/bin/dictate-dev" ]; then rm target/debug/dictate; else mv -f target/debug/dictate "$HOME/.local/bin/dictate-dev"; fi
    install -Dm644 packaging/systemd/dictate-dev.service "$HOME/.config/systemd/user/dictate-dev.service"
    systemctl --user daemon-reload
    systemctl --user enable dictate-dev.service
    systemctl --user restart dictate-dev.service

check *ARGS:
    cargo check --locked --all-targets --all-features {{ ARGS }}

clean:
    cargo clean

clippy *ARGS:
    cargo clippy --locked --all-targets --all-features --fix --allow-dirty {{ ARGS }} -- -D warnings

debug-eval:
    cargo run --quiet -p dictate -- debug --screen overlay --scenario recording-sine --stats json --duration 2s --exit | jq -s -e 'map(select(.type == "frame")) as $frames | map(select(.type == "aggregates")) as $aggregates | ($frames | length) > 0 and ($aggregates | length) == 1 and ($aggregates[0].measured_fps > 0) and ($aggregates[0].frame_count == ($frames | length))'

fmt *ARGS:
    cargo +nightly fmt {{ ARGS }}

# run pre-commit on all files
lint *ARGS:
    @just --fmt
    uvx prek run --all-files --show-diff-on-failure --color always {{ ARGS }}

run *ARGS:
    cargo run -p dictate -- {{ ARGS }}

test *ARGS:
    cargo test {{ ARGS }}

test-integration *ARGS:
    cargo test -p dictate --features integration --test integration {{ ARGS }}
