#!/usr/bin/env bash
# Rebuilds Dictate Dev and swaps it for the running instance.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# A fixed bundle path keeps one Launch Services registration for the dev app.
app="$HOME/Applications/Dictate Dev.app"
build="$root/target/debug/Dictate Dev.app"

DICTATE_BUILD=dev "$root/tools/build-macos-app.sh" "$build"

if [[ -x "$app/Contents/MacOS/dictate-dev" ]]; then
    "$app/Contents/MacOS/dictate-dev" quit 2>/dev/null || true
fi
mkdir -p "$(dirname "$app")"
rm -rf "$app"
ditto "$build" "$app"
open "$app"
echo "Launched $app"
