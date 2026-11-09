#!/usr/bin/env bash
# Start the app, check it is still running after a few seconds, then stop it:
# scripts/smoke-launch.sh <command> [args...]
# The release workflow runs this on each built app. It fails if the app exits early, such as
# when it finds no GPU it can render with.
set -euo pipefail

seconds="${SMOKE_SECONDS:-10}"
log="$(mktemp)"

"$@" >"$log" 2>&1 &
pid=$!
sleep "$seconds"

if ! kill -0 "$pid" 2>/dev/null; then
  status=0
  wait "$pid" || status=$?
  cat "$log"
  echo "The app exited within ${seconds}s with status $status." >&2
  exit 1
fi

kill "$pid"
wait "$pid" 2>/dev/null || true
cat "$log"
echo "The app was still running after ${seconds}s."
