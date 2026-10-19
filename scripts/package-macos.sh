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
if [[ "${YAP_UNIVERSAL:-}" == "1" ]]; then
	target="universal-apple-darwin"
	binary="$root/target/$target/release/yap"
	mkdir -p "$(dirname "$binary")"
	lipo -create \
		"$root/target/aarch64-apple-darwin/release/yap" \
		"$root/target/x86_64-apple-darwin/release/yap" \
		-output "$binary"
else
	target="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
	binary="$root/target/release/yap"
	if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
		binary="$root/target/$CARGO_BUILD_TARGET/release/yap"
	fi
fi
app="$root/dist/$target/Yap.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/yap"
cp "$root/packaging/Info.plist" "$app/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$version" "$app/Contents/Info.plist"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
cp "$root/packaging/Yap.icns" "$app/Contents/Resources/Yap.icns"
chmod +x "$app/Contents/MacOS/yap"
codesign --force --options runtime --entitlements "$root/packaging/entitlements.plist" \
	--sign "${MACOS_SIGNING_IDENTITY:--}" "$app"
archive="$root/dist/yap-$target.zip"
if [[ "$target" == "universal-apple-darwin" ]]; then
	archive="$root/dist/Yap-macos-universal-app.zip"
fi
if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
	ditto -c -k --keepParent "$app" "$archive"
	xcrun notarytool submit "$archive" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
	xcrun stapler staple "$app"
fi
cp "$root/packaging/Configure Yap.command" "$root/dist/$target/Configure Yap.command"
chmod +x "$root/dist/$target/Configure Yap.command"
# Without bin/, mise would put the archive root on PATH because Configure
# Yap.command is executable, and never look inside Yap.app.
mkdir -p "$root/dist/$target/bin"
cp "$root/packaging/yap" "$root/dist/$target/bin/yap"
chmod +x "$root/dist/$target/bin/yap"
ditto -c -k --norsrc "$root/dist/$target" "$archive"
printf 'Packaged %s\n' "$archive"

# Standalone tarballs for `mise use -g github:howmanysmall/yap`: the binary and
# LICENSE at the archive root, named after the repo and the target triple.
tarball() {
	local triple="$1" bin="$2" stage
	stage="$(mktemp -d)"
	cp "$bin" "$stage/yap"
	chmod +x "$stage/yap"
	cp "$root/LICENSE" "$stage/LICENSE"
	COPYFILE_DISABLE=1 tar -czf "$root/dist/yap-$triple.tar.gz" -C "$stage" yap LICENSE
	rm -rf "$stage"
	printf 'Packaged %s\n' "$root/dist/yap-$triple.tar.gz"
}
if [[ "$target" == "universal-apple-darwin" ]]; then
	tarball aarch64-apple-darwin "$root/target/aarch64-apple-darwin/release/yap"
	tarball x86_64-apple-darwin "$root/target/x86_64-apple-darwin/release/yap"
else
	tarball "$target" "$binary"
fi
