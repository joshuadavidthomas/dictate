set dotenv-load
set unstable

# List all available commands
[private]
default:
    @just --list --list-submodules

build *ARGS:
    cargo build -p dictate {{ ARGS }}

build-dev *ARGS:
    DICTATE_BUILD=dev cargo build -p dictate --features dev-tools {{ ARGS }}

build-release *ARGS:
    DICTATE_BUILD=stable cargo build --release -p dictate --no-default-features {{ ARGS }}

install: build-release
    mkdir -p "$HOME/.local/bin"
    if [ target/release/dictate -ef "$HOME/.local/bin/dictate" ]; then rm target/release/dictate; else mv -f target/release/dictate "$HOME/.local/bin/dictate"; fi
    install -Dm644 systemd/dictate.service "$HOME/.config/systemd/user/dictate.service"
    install -Dm644 desktop/dev.joshthomas.dictate.desktop "$HOME/.local/share/applications/dev.joshthomas.dictate.desktop"
    systemctl --user daemon-reload
    systemctl --user enable dictate.service
    systemctl --user restart dictate.service

[private]
install-dev-files: build-dev
    mkdir -p "$HOME/.local/bin"
    if [ target/debug/dictate -ef "$HOME/.local/bin/dictate-dev" ]; then rm target/debug/dictate; else mv -f target/debug/dictate "$HOME/.local/bin/dictate-dev"; fi
    install -Dm644 systemd/dictate-dev.service "$HOME/.config/systemd/user/dictate-dev.service"
    install -Dm644 desktop/dev.joshthomas.dictate_dev.desktop "$HOME/.local/share/applications/dev.joshthomas.dictate_dev.desktop"

install-dev: install-dev-files
    systemctl --user daemon-reload
    systemctl --user enable dictate-dev.service
    systemctl --user restart dictate-dev.service

install-pi-extension:
    pi install "{{ justfile_directory() }}"

check *ARGS:
    cargo check --locked --all-targets --all-features {{ ARGS }}

clean:
    cargo clean

clippy *ARGS:
    cargo clippy --locked --all-targets --all-features --fix --allow-dirty {{ ARGS }} -- -D warnings

debug-eval:
    DICTATE_BUILD=dev cargo run --quiet -p dictate --features dev-tools -- debug --screen overlay --scenario recording-sine --stats json --duration 2s --exit | jq -s -e 'map(select(.type == "frame")) as $frames | map(select(.type == "aggregates")) as $aggregates | ($frames | length) > 0 and ($aggregates | length) == 1 and ($aggregates[0].measured_fps > 0) and ($aggregates[0].frame_count == ($frames | length))'

rustfmt_channel := `sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/rustfmt/rust-toolchain.toml`

fmt *ARGS:
    cargo "+{{ rustfmt_channel }}" fmt --manifest-path "{{ justfile_directory() }}/Cargo.toml" --all {{ ARGS }}

# cargo-hawk must run on the toolchain it was built against.
# Keep this paired with the Hawk version in mise.toml.
hawk_channel := `sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/hawk/rust-toolchain.toml`

[positional-arguments]
hawk *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo "+{{ hawk_channel }}" hawk check \
        --manifest-path "{{ justfile_directory() }}/Cargo.toml" \
        --target-dir "{{ justfile_directory() }}/target/hawk" \
        -D warnings "$@"

# run pre-commit on all files
lint *ARGS:
    @just --fmt
    prek run --all-files --show-diff-on-failure --color always {{ ARGS }}

run *ARGS:
    cargo run -p dictate -- {{ ARGS }}

test *ARGS:
    cargo test {{ ARGS }}
    npm test

test-integration *ARGS:
    cargo test -p dictate-speech --features integration --test integration {{ ARGS }}
