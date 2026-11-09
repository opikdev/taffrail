#!/usr/bin/env bash
# Run a command until it succeeds, at most a few times: scripts/retry.sh <command> [args...]
# The release workflow wraps cargo-packager in it, because the tools it downloads from
# GitHub releases, such as NSIS and WiX, sometimes fail with a 5xx error.
set -uo pipefail

attempts="${RETRY_ATTEMPTS:-3}"
delay="${RETRY_DELAY_SECONDS:-30}"

for attempt in $(seq 1 "$attempts"); do
  "$@" && exit 0
  status=$?
  if [ "$attempt" -lt "$attempts" ]; then
    echo "Attempt $attempt of $attempts failed with status $status. Trying again in ${delay}s." >&2
    sleep "$delay"
  fi
done
echo "All $attempts attempts failed." >&2
exit "$status"
