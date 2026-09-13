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

install-dev: build-dev
    mkdir -p "$HOME/.local/bin"
    if [ target/debug/dictate -ef "$HOME/.local/bin/dictate-dev" ]; then rm target/debug/dictate; else mv -f target/debug/dictate "$HOME/.local/bin/dictate-dev"; fi
    install -Dm644 systemd/dictate-dev.service "$HOME/.config/systemd/user/dictate-dev.service"
    install -Dm644 desktop/dev.joshthomas.dictate_dev.desktop "$HOME/.local/share/applications/dev.joshthomas.dictate_dev.desktop"
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

fmt *ARGS:
    cargo +nightly fmt {{ ARGS }}

[positional-arguments]
hawk *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    assume_yes=false
    hawk_args=()
    while (($#)); do
        case "$1" in
            -y|--yes) assume_yes=true ;;
            --) hawk_args+=("$@"); break ;;
            *) hawk_args+=("$1") ;;
        esac
        shift
    done
    if ! command -v cargo-hawk >/dev/null 2>&1; then
        if [[ "$assume_yes" == false ]]; then
            if [[ ! -t 0 ]]; then
                echo "cargo-hawk is missing. Run just hawk interactively or pass --yes (-y) to install it." >&2
                exit 1
            fi
            read -r -p "Download and run the latest Hawk installer from github.com/astral-sh/hawk? [y/N] " answer || exit 1
            case "$answer" in
                [yY]|[yY][eE][sS]) ;;
                *) exit 1 ;;
            esac
        fi
        echo "Installing cargo-hawk"
        curl --proto '=https' --tlsv1.2 -LsSf \
            https://github.com/astral-sh/hawk/releases/latest/download/cargo-hawk-installer.sh | sh
    fi
    channel=$(sed -n 's/^channel = "\([^"]*\)"/\1/p' tools/hawk/rust-toolchain.toml)
    # Avoid astral-sh/hawk#74 rustc-info cache poisoning.
    # Keep Hawk focused on visibility; clippy owns dead-code and unused checks.
    RUSTFLAGS="${RUSTFLAGS:-} -A dead_code -A unused_imports" CARGO_CACHE_RUSTC_INFO=0 \
        cargo "+$channel" hawk check \
        --manifest-path "{{ justfile_directory() }}/Cargo.toml" \
        --target-dir "{{ justfile_directory() }}/target/hawk" "${hawk_args[@]}"

# run pre-commit on all files
lint *ARGS:
    @just --fmt
    uvx prek run --all-files --show-diff-on-failure --color always {{ ARGS }}

run *ARGS:
    cargo run -p dictate -- {{ ARGS }}

test *ARGS:
    cargo test {{ ARGS }}
    npm test

test-integration *ARGS:
    cargo test -p dictate-speech --features integration --test integration {{ ARGS }}
