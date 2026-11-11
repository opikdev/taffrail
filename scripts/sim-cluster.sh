#!/usr/bin/env bash
# Run a simulated cluster for benchmarks: scripts/sim-cluster.sh up|down|status|churn|limited
set -euo pipefail

cluster="${SIM_CLUSTER:-taffrail}"
nodes="${SIM_NODES:-100}"
pods="${SIM_PODS:-5000}"
deployments="${SIM_DEPLOYMENTS:-500}"
statefulsets="${SIM_STATEFULSETS:-200}"
daemonsets="${SIM_DAEMONSETS:-50}"
log_rate="${SIM_LOG_RATE:-200}"
limited_user="${SIM_LIMITED_USER:-jane}"
limited_namespace="${SIM_LIMITED_NAMESPACE:-team-a}"
root="$(cd "$(dirname "$0")/.." && pwd)"
sim="$root/.sim"
context="kwok-$cluster"
generator="kwok-$cluster-gen-logs"
limited_context="$context-$limited_user"
pki="${KWOK_WORKDIR:-$HOME/.kwok}/clusters/$cluster/pki"
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

  # RBAC is on, so a limited user sees what a real cluster would refuse it.
  kwokctl create cluster --name="$cluster" --config="$sim/kwok-$cluster.yaml" --disable-qps-limits \
    --kube-authorization
  if [ -n "$previous" ]; then
    kubectl config use-context "$previous" >/dev/null
  fi

  kwokctl --name="$cluster" scale node --replicas "$nodes"
  kwokctl --name="$cluster" scale pod --replicas "$pods"
  add_deployments
  add_statefulsets
  add_daemonsets

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

# A user who can read pods and their logs in one namespace, and nothing else. Its context,
# kwok-<cluster>-<user>, uses a client certificate signed by the cluster's own CA.
limited() {
  local key="$sim/$limited_context.key" cert="$sim/$limited_context.crt"
  mkdir -p "$sim"
  k create namespace "$limited_namespace" --dry-run=client -o yaml | k apply -f - >/dev/null
  if ! k -n "$limited_namespace" get deployment team-api >/dev/null 2>&1; then
    k -n "$limited_namespace" create deployment team-api --image=busybox --replicas=5 >/dev/null
    k -n "$limited_namespace" rollout status deployment/team-api --timeout=120s >/dev/null
  fi
  k apply -f - >/dev/null <<YAML
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: pod-reader
  namespace: $limited_namespace
rules:
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "list", "watch"]
- apiGroups: [""]
  resources: ["pods/log"]
  verbs: ["get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: $limited_user-pod-reader
  namespace: $limited_namespace
subjects:
- apiGroup: rbac.authorization.k8s.io
  kind: User
  name: $limited_user
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: pod-reader
YAML
  openssl req -new -newkey rsa:2048 -nodes -keyout "$key" -subj "/CN=$limited_user" \
    -out "$sim/$limited_context.csr" 2>/dev/null
  # Extensions make a version 3 certificate, which rustls requires.
  printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=clientAuth\n' \
    > "$sim/$limited_context.ext"
  openssl x509 -req -in "$sim/$limited_context.csr" -CA "$pki/ca.crt" -CAkey "$pki/ca.key" \
    -CAcreateserial -days 7 -extfile "$sim/$limited_context.ext" -out "$cert" 2>/dev/null
  kubectl config set-credentials "$limited_context" --client-certificate="$cert" \
    --client-key="$key" --embed-certs >/dev/null
  kubectl config set-context "$limited_context" --cluster="$context" --user="$limited_context" \
    --namespace="$limited_namespace" >/dev/null
  echo "Context $limited_context can read pods and logs in $limited_namespace only."
  kubectl --context="$limited_context" get pods --no-headers | awk '{print $3}' | sort | uniq -c
}

# Runs the core watches as the limited user, and checks what the API refuses.
check_limited() {
  TAFFRAIL_LIMITED_CONTEXT="$limited_context" TAFFRAIL_LIMITED_NAMESPACE="$limited_namespace" \
    cargo test -p taffrail-core --locked limited_user -- --ignored --nocapture
}

# StatefulSets scaled to zero, so the StatefulSets table has rows without adding pods.
add_statefulsets() {
  local i
  for ((i = 0; i < statefulsets; i++)); do
    printf -- '---\napiVersion: apps/v1\nkind: StatefulSet\nmetadata:\n  name: sim-sts-%04d\nspec:\n  replicas: 0\n  serviceName: sim-sts-%04d\n  selector:\n    matchLabels:\n      app: sim-sts-%04d\n  template:\n    metadata:\n      labels:\n        app: sim-sts-%04d\n    spec:\n      containers:\n      - name: app\n        image: busybox\n' "$i" "$i" "$i" "$i"
  done | k apply -f - >/dev/null
}

# DaemonSets whose node selector matches no node, so the DaemonSets table has rows
# without adding pods.
add_daemonsets() {
  local i
  for ((i = 0; i < daemonsets; i++)); do
    printf -- '---\napiVersion: apps/v1\nkind: DaemonSet\nmetadata:\n  name: sim-ds-%04d\nspec:\n  selector:\n    matchLabels:\n      app: sim-ds-%04d\n  template:\n    metadata:\n      labels:\n        app: sim-ds-%04d\n    spec:\n      nodeSelector:\n        taffrail.dev/sim-daemonset: "%04d"\n      containers:\n      - name: app\n        image: busybox\n' "$i" "$i" "$i" "$i"
  done | k apply -f - >/dev/null
}

down() {
  kubectl config delete-context "$limited_context" >/dev/null 2>&1 || true
  kubectl config delete-user "$limited_context" >/dev/null 2>&1 || true
  rm -f "$sim/$limited_context".*
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

case "${1:-status}" in
  check-limited) check_limited ;;
  *) "${1:-status}" ;;
esac
