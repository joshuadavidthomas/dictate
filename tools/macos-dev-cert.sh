#!/usr/bin/env bash
# Creates a self-signed "Dictate Local Dev" code signing certificate in the login
# keychain, for builds without an Apple Development certificate. macOS keeps
# permission grants for apps signed with it as long as the certificate stays.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "macos-dev-cert.sh must run on macOS" >&2
    exit 1
fi

name="Dictate Local Dev"
keychain="${DICTATE_DEV_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"

if security find-identity -p codesigning "$keychain" | grep -q "\"$name\""; then
    echo "$name already exists in $keychain"
    exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/dictate-dev-cert.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# The system LibreSSL writes a PKCS#12 file that `security import` reads; a newer
# OpenSSL on PATH defaults to ciphers it rejects.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -subj "/CN=$name" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -name "$name" -passout pass:dictate \
    -inkey "$work/key.pem" -in "$work/cert.pem" -out "$work/identity.p12"
security import "$work/identity.p12" -k "$keychain" -P dictate -T /usr/bin/codesign

echo "Created $name in $keychain"
echo "The first signature may ask to use the key; choose Always Allow."
