#!/usr/bin/env bash
# Render the README screenshot from the offscreen previews, which use fake data: scripts/screenshot.sh
set -euo pipefail

width=1600
root="$(cd "$(dirname "$0")/.." && pwd)"
previews="$(mktemp -d)"
trap 'rm -rf "$previews"' EXIT
image="$root/docs/images/taffrail.png"

# The log dock shows local time, so render in UTC to match the sample timestamps.
TZ=UTC TAFFRAIL_PREVIEW_DIR="$previews" cargo run --locked -p taffrail-ui --features preview \
  --manifest-path "$root/Cargo.toml" >/dev/null
mkdir -p "$(dirname "$image")"

if command -v sips >/dev/null; then
  sips --resampleWidth "$width" "$previews/readme-dark.png" --out "$image" >/dev/null
elif command -v magick >/dev/null; then
  magick "$previews/readme-dark.png" -resize "$width" -strip "$image"
else
  echo "Install ImageMagick to resize the screenshot." >&2
  exit 1
fi
echo "$image"
