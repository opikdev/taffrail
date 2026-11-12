#!/usr/bin/env bash
# Capture the README screenshot from a real window of the demo app, which uses fake data:
# scripts/screenshot.sh
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "scripts/screenshot.sh captures a macOS window, so it runs only on macOS." >&2
  exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
image="$root/docs/images/taffrail.png"
# How long the window gets to connect, tail its pods and draw before the capture.
draw_seconds=6
max_bytes=1500000
work="$(mktemp -d)"
app=""

cleanup() {
  if [[ -n "$app" ]] && kill -0 "$app" 2>/dev/null; then
    kill "$app"
    wait "$app" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# Prints the id of the largest normal window that process $1 has on screen, waiting up to
# $2 seconds for one to appear.
cat >"$work/window-id.swift" <<'SWIFT'
import CoreGraphics
import Foundation

let pid = pid_t(CommandLine.arguments[1])!
let deadline = Date().addingTimeInterval(Double(CommandLine.arguments[2])!)

func area(_ window: [String: Any]) -> Double {
    let bounds = window[kCGWindowBounds as String] as? [String: Double] ?? [:]
    return (bounds["Width"] ?? 0) * (bounds["Height"] ?? 0)
}

func windowID() -> CGWindowID? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    return windows
        .filter {
            $0[kCGWindowOwnerPID as String] as? pid_t == pid
                && $0[kCGWindowLayer as String] as? Int == 0
        }
        .max { area($0) < area($1) }
        .flatMap { $0[kCGWindowNumber as String] as? CGWindowID }
}

while Date() < deadline {
    if let id = windowID() {
        print(id)
        exit(0)
    }
    Thread.sleep(forTimeInterval: 0.25)
}
FileHandle.standardError.write("Process \(pid) showed no window.\n".data(using: .utf8)!)
exit(1)
SWIFT

# The demo exists only with the preview feature, so release builds never have it.
binary="$(cargo build --locked -p taffrail-ui --features preview \
  --manifest-path "$root/Cargo.toml" --message-format=json-render-diagnostics |
  sed -n 's/.*"executable":"\([^"]*\/taffrail\)".*/\1/p' | tail -n 1)"
if [[ ! -x "$binary" ]]; then
  echo "Could not find the built taffrail binary." >&2
  exit 1
fi

# The log dock shows local time, so run in UTC to match the sample timestamps.
TZ=UTC TAFFRAIL_DEMO=1 "$binary" >"$work/app.log" 2>&1 &
app=$!

window="$(swift "$work/window-id.swift" "$app" 60)" || {
  cat "$work/app.log" >&2
  exit 1
}
sleep "$draw_seconds"

# Without -o, the capture keeps the window's shadow.
if ! screencapture -x -l "$window" "$work/window.png" || [[ ! -s "$work/window.png" ]]; then
  echo "screencapture failed. Allow your terminal under Privacy & Security > Screen Recording." >&2
  exit 1
fi
kill "$app"
wait "$app" 2>/dev/null || true
app=""

mkdir -p "$(dirname "$image")"
if command -v pngquant >/dev/null; then
  tool=pngquant
  pngquant --force --strip --speed 1 --output "$image" -- "$work/window.png"
else
  tool=sips
  sips -s format png "$work/window.png" --out "$image" >/dev/null
fi

bytes="$(stat -f %z "$image")"
size="$(sips -g pixelWidth -g pixelHeight "$image" | awk '/pixel/ { print $2 }' | paste -sd x -)"
echo "$image: $size px, $bytes bytes, written with $tool"
if ((bytes > max_bytes)); then
  echo "That is over $max_bytes bytes. Install pngquant with 'brew install pngquant' and run again." >&2
  exit 1
fi
