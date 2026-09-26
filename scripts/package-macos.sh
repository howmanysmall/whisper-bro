#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
binary="$root/target/release/whisper-bro"
if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
    binary="$root/target/$CARGO_BUILD_TARGET/release/whisper-bro"
fi
app="$root/dist/$target/Whisper Bro.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/whisper-bro"
cp "$root/packaging/Info.plist" "$app/Contents/Info.plist"
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
ditto -c -k "$root/dist/$target" "$archive"
printf 'Packaged %s\n' "$archive"
