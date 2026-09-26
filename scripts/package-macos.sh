#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# Cargo.toml's [package] version is the single source of truth; it is the first
# top-level `version` key in the manifest.
version="$(awk -F'"' '/^version = / { print $2; exit }' "$root/Cargo.toml")"
if [[ -z "$version" ]]; then
    printf 'Could not read the package version from Cargo.toml\n' >&2
    exit 1
fi
target="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
binary="$root/target/release/whisper-bro"
if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
    binary="$root/target/$CARGO_BUILD_TARGET/release/whisper-bro"
fi
app="$root/dist/$target/Whisper Bro.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/whisper-bro"
cp "$root/packaging/Info.plist" "$app/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
chmod +x "$app/Contents/MacOS/whisper-bro"
codesign --force --options runtime --entitlements "$root/packaging/entitlements.plist" \
    --sign "${MACOS_SIGNING_IDENTITY:--}" "$app"
archive="$root/dist/whisper-bro-$target.zip"
if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
    ditto -c -k --keepParent "$app" "$archive"
    xcrun notarytool submit "$archive" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
    xcrun stapler staple "$app"
fi
cp "$root/packaging/Configure Whisper Bro.command" "$root/dist/$target/Configure Whisper Bro.command"
chmod +x "$root/dist/$target/Configure Whisper Bro.command"
ditto -c -k --norsrc "$root/dist/$target" "$archive"
printf 'Packaged %s\n' "$archive"
