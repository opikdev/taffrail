#!/usr/bin/env bash
# Render the app icon from crates/ui/assets/icon/icon.svg to every size the installers need.
# Needs macOS for iconutil, resvg (cargo install resvg --locked) and python3.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
icons="$root/crates/ui/assets/icon"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

render() {
  resvg --width "$1" --height "$1" "$icons/icon.svg" "$2"
}

for size in 16 24 32 48 64 128 256 512 1024; do
  render "$size" "$work/$size.png"
done

# Linux installs these into the hicolor icon theme, and X11 shows one as the window icon.
for size in 32 64 128 256 512; do
  cp "$work/$size.png" "$icons/${size}x${size}.png"
done

iconset="$work/icon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  cp "$work/$size.png" "$iconset/icon_${size}x${size}.png"
  cp "$work/$((size * 2)).png" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil --convert icns --output "$icons/icon.icns" "$iconset"

# A Windows .ico can hold PNG images directly, so no image library is needed.
python3 -I - "$work" "$icons/icon.ico" <<'PY'
import struct, sys
from pathlib import Path

work, out = Path(sys.argv[1]), Path(sys.argv[2])
sizes = [16, 24, 32, 48, 64, 128, 256]
images = [(work / f"{size}.png").read_bytes() for size in sizes]
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = len(header) + 16 * len(sizes)
entries = b""
for size, data in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
out.write_bytes(header + entries + b"".join(images))
PY

echo "Rendered icons into $icons"
