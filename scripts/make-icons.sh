#!/usr/bin/env bash
# Regenerates the app icons from packaging/icon.svg. macOS only (sips, iconutil).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
packaging="$root/packaging"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

png="$packaging/icon-1024.png"
if command -v rsvg-convert > /dev/null 2>&1; then
	rsvg-convert -w 1024 -h 1024 "$packaging/icon.svg" -o "$png"
elif command -v magick > /dev/null 2>&1; then
	magick -background none -density 384 "$packaging/icon.svg" -resize 1024x1024 "$png"
else
	qlmanage -t -s 1024 -o "$tmp" "$packaging/icon.svg" > /dev/null
	mv "$tmp/icon.svg.png" "$png"
fi

iconset="$tmp/Yap.iconset"
mkdir -p "$iconset"
resize() {
	sips -z "$1" "$1" "$png" --out "$iconset/$2" > /dev/null
}
resize 16 icon_16x16.png
resize 32 icon_16x16@2x.png
resize 32 icon_32x32.png
resize 64 icon_32x32@2x.png
resize 128 icon_128x128.png
resize 256 icon_128x128@2x.png
resize 256 icon_256x256.png
resize 512 icon_256x256@2x.png
resize 512 icon_512x512.png
resize 1024 icon_512x512@2x.png
iconutil -c icns "$iconset" -o "$packaging/Yap.icns"

sizes=(256 48 32 16)
pngs=()
for size in "${sizes[@]}"; do
	sips -z "$size" "$size" "$png" --out "$tmp/ico-$size.png" > /dev/null
	pngs+=("$tmp/ico-$size.png")
done
# magick writes the 256px entry uncompressed (285 KB); PNG entries are 16 KB.
python3 -I "$root/scripts/make-ico.py" "${pngs[@]}" "$packaging/yap.ico"
printf 'Wrote icons to %s\n' "$packaging"
