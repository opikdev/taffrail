#!/usr/bin/env bash
# Measure the app on a running simulated cluster: scripts/bench.sh [seconds]
set -euo pipefail

seconds="${1:-20}"
settle=8
root="$(cd "$(dirname "$0")/.." && pwd)"
context="kwok-${SIM_CLUSTER:-taffrail}"
kubeconfig="$(mktemp)"
rollouts=""

cleanup() {
  if [ -n "$rollouts" ]; then
    kill "$rollouts" 2>/dev/null || true
  fi
  rm -f "$kubeconfig"
}
trap cleanup EXIT

kubectl config view --raw --minify --flatten --context="$context" >"$kubeconfig"
"$root/scripts/sim-cluster.sh" churn >/dev/null
cargo build --release --locked --features bench --manifest-path "$root/Cargo.toml"

run() {
  local label="$1" tail="$2" follow="${3:-}" view="${4:-}" app
  (
    while true; do
      "$root/scripts/sim-cluster.sh" churn >/dev/null
      sleep 8
    done
  ) &
  rollouts=$!
  KUBECONFIG="$kubeconfig" TAFFRAIL_BENCH_TAIL="$tail" TAFFRAIL_BENCH_FOLLOW="$follow" \
    TAFFRAIL_BENCH_VIEW="$view" \
    TAFFRAIL_BENCH_SECONDS="$seconds" "$root/target/release/taffrail" &
  app=$!
  sleep "$settle"
  "$root/scripts/sample-process-tree.py" "$app" --seconds "$((seconds - settle))" --label "$label"
  wait "$app"
  kill "$rollouts"
  rollouts=""
}

# A view named in the environment is measured alone, such as TAFFRAIL_BENCH_VIEW=Jobs.
if [ -n "${TAFFRAIL_BENCH_VIEW:-}" ]; then
  run "scroll-$(printf '%s' "$TAFFRAIL_BENCH_VIEW" | tr '[:upper:]' '[:lower:]')" 0 "" "$TAFFRAIL_BENCH_VIEW"
  exit
fi

run scroll-churn 0
run scroll-churn-tail 1
run scroll-churn-follow 1 app=sim-churn
run scroll-deployments 0 "" Deployments
