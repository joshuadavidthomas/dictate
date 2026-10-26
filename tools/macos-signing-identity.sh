#!/usr/bin/env bash
# Prints the codesign identity for local macOS builds. macOS ties Microphone,
# Accessibility, and Input Monitoring grants to the signature, so any stable
# certificate keeps them across rebuilds; an ad-hoc signature does not.
set -euo pipefail

if [[ -n "${DICTATE_CODESIGN_IDENTITY:-}" ]]; then
    echo "$DICTATE_CODESIGN_IDENTITY"
    exit 0
fi

# Any Apple ID, paid or free, can create an Apple Development certificate in Xcode.
apple_development="$(security find-identity -v -p codesigning \
    | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) "Apple Development: .*"$/\1/p' \
    | head -n 1)"
if [[ -n "$apple_development" ]]; then
    echo "$apple_development"
    exit 0
fi

# `just dev-cert` creates this one; it is self-signed, so only the unfiltered list shows it.
local_dev="$(security find-identity -p codesigning \
    | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) "Dictate Local Dev".*$/\1/p' \
    | head -n 1)"
if [[ -n "$local_dev" ]]; then
    echo "$local_dev"
    exit 0
fi

cat >&2 <<'WARNING'
warning: no stable signing identity found, so this build is ad-hoc signed.
warning: macOS will ask for Microphone, Accessibility, and Input Monitoring again after every rebuild.
warning: run `just dev-cert` once, sign in to Xcode for an Apple Development certificate, or set DICTATE_CODESIGN_IDENTITY.
WARNING
echo "-"
