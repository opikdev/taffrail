#!/usr/bin/env bash
# Run a simulated cluster for benchmarks: scripts/sim-cluster.sh up|down|status|churn
set -euo pipefail

cluster="${SIM_CLUSTER:-taffrail}"
nodes="${SIM_NODES:-100}"
pods="${SIM_PODS:-5000}"
deployments="${SIM_DEPLOYMENTS:-500}"
log_rate="${SIM_LOG_RATE:-200}"
root="$(cd "$(dirname "$0")/.." && pwd)"
sim="$root/.sim"
context="kwok-$cluster"
generator="kwok-$cluster-gen-logs"
# Must live inside the Docker VM. Files on the macOS share emit combined
# fsnotify events that KWOK rejects, which silently breaks `kubectl logs -f`.
# Each cluster has its own file, so clusters with different names can run together.
log_dir="/var/lib/taffrail-sim/$cluster"
k() { kubectl --context="$context" "$@"; }

up() {
  local previous
  previous="$(kubectl config current-context 2>/dev/null || true)"
  mkdir -p "$sim"

  docker run --rm -v "$log_dir:$log_dir" python:3.13-alpine \
    sh -c "rm -f $log_dir/app.log && touch $log_dir/app.log"

  cat > "$sim/kwok-$cluster.yaml" <<YAML
kind: ClusterLogs
apiVersion: kwok.x-k8s.io/v1alpha1
metadata:
  name: sim-logs
spec:
  selector: {}
  logs:
  - logsFile: $log_dir/app.log
    follow: true
YAML

  kwokctl create cluster --name="$cluster" --config="$sim/kwok-$cluster.yaml" --disable-qps-limits
  if [ -n "$previous" ]; then
    kubectl config use-context "$previous" >/dev/null
  fi

  kwokctl --name="$cluster" scale node --replicas "$nodes"
  kwokctl --name="$cluster" scale pod --replicas "$pods"
  add_deployments

  docker run -d --name "$generator" --network none \
    -v "$log_dir:$log_dir" -v "$root/scripts:/scripts:ro" python:3.13-alpine \
    python /scripts/gen-logs.py "$log_dir/app.log" --rate "$log_rate" >/dev/null
  status
}

# Deployments scaled to zero, so the Deployments table has rows without adding pods.
add_deployments() {
  local i
  for ((i = 0; i < deployments; i++)); do
    printf -- '---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: sim-deploy-%04d\nspec:\n  replicas: 0\n  selector:\n    matchLabels:\n      app: sim-deploy-%04d\n  template:\n    metadata:\n      labels:\n        app: sim-deploy-%04d\n    spec:\n      containers:\n      - name: app\n        image: busybox\n' "$i" "$i" "$i"
  done | k apply -f - >/dev/null
}

down() {
  docker rm -f "$generator" >/dev/null 2>&1 || true
  kwokctl delete cluster --name="$cluster"
  docker run --rm -v "$(dirname "$log_dir"):/sim" python:3.13-alpine rm -rf "/sim/$cluster"
}

churn() {
  if ! k get deployment sim-churn >/dev/null 2>&1; then
    k create deployment sim-churn --image=busybox --replicas="${SIM_CHURN_PODS:-500}" >/dev/null
    k rollout status deployment/sim-churn --timeout=120s
  fi
  k rollout restart deployment/sim-churn
}

status() {
  k get pods -A --no-headers | awk '{print $4}' | sort | uniq -c
}

"${1:-status}"
