#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "build-macos-app.sh must run on macOS" >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# DICTATE_BUILD=dev builds Dictate Dev: a debug build with dev tools and its own
# bundle identifier, so its permissions and login item stay apart from Dictate's.
case "${DICTATE_BUILD:-stable}" in
    stable)
        profile=release
        cargo_args=(--release)
        bundle_name="Dictate"
        bundle_id="dev.joshthomas.dictate"
        executable="dictate"
        ;;
    dev)
        profile=debug
        cargo_args=(--features dev-tools)
        bundle_name="Dictate Dev"
        bundle_id="dev.joshthomas.dictate-dev"
        executable="dictate-dev"
        ;;
    *)
        echo "DICTATE_BUILD must be stable or dev, got ${DICTATE_BUILD}" >&2
        exit 1
        ;;
esac
app="${1:-$root/target/$profile/$bundle_name.app}"
identity="$("$root/tools/macos-signing-identity.sh")"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/crates/dictate/Cargo.toml" | head -n 1)"
build_number="${DICTATE_BUILD_NUMBER:-1}"
icon_source="$root/packaging/icons/dictate.icon"
icon_fallback="$root/packaging/icons/dictate.icns"
icon_build=""

cleanup() {
    if [[ -n "$icon_build" ]]; then
        rm -rf "$icon_build"
    fi
}
trap cleanup EXIT

if [[ -z "$version" ]]; then
    echo "could not read Dictate version from crates/dictate/Cargo.toml" >&2
    exit 1
fi

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
if [[ "${DICTATE_RUNTIME_SHADERS:-0}" == "1" ]]; then
    cargo_args+=(--features gpui_platform/runtime_shaders)
fi
DICTATE_BUILD="${DICTATE_BUILD:-stable}" cargo build --locked -p dictate "${cargo_args[@]}"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 0755 "$root/target/$profile/dictate" "$app/Contents/MacOS/$executable"
install -m 0644 "$root/packaging/macos/Info.plist" "$app/Contents/Info.plist"
for key in CFBundleName CFBundleDisplayName; do
    /usr/libexec/PlistBuddy -c "Set :$key $bundle_name" "$app/Contents/Info.plist"
done
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $bundle_id" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $executable" "$app/Contents/Info.plist"

if [[ -d "$icon_source" ]] \
    && [[ -x "${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}/usr/bin/xcodebuild" ]] \
    && [[ "$(DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}" xcodebuild -version | sed -n 's/^Xcode \([0-9][0-9]*\).*/\1/p')" -ge 26 ]]; then
    export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
    icon_build="$(mktemp -d "${TMPDIR:-/tmp}/dictate-icon.XXXXXX")"
    mkdir -p "$icon_build/output"
    xcrun actool \
        --compile "$icon_build/output" \
        --platform macosx \
        --minimum-deployment-target "$MACOSX_DEPLOYMENT_TARGET" \
        --app-icon dictate \
        --output-partial-info-plist "$icon_build/partial.plist" \
        "$icon_source"
    install -m 0644 "$icon_build/output/Assets.car" "$app/Contents/Resources/Assets.car"
    install -m 0644 "$icon_build/output/dictate.icns" "$app/Contents/Resources/dictate.icns"
    plutil -replace CFBundleIconFile -string \
        "$(plutil -extract CFBundleIconFile raw "$icon_build/partial.plist")" \
        "$app/Contents/Info.plist"
    plutil -insert CFBundleIconName -string \
        "$(plutil -extract CFBundleIconName raw "$icon_build/partial.plist")" \
        "$app/Contents/Info.plist"
else
    install -m 0644 "$icon_fallback" "$app/Contents/Resources/dictate.icns"
fi

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

codesign --force --options runtime --timestamp=none \
    --entitlements "$root/packaging/macos/Dictate.entitlements" \
    --sign "$identity" "$app"
codesign --verify --deep --strict "$app"

echo "Built $app"
