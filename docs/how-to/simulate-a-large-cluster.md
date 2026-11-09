# Simulate a large cluster

Run 5,000 fake pods with live logs on a laptop. Pods are simulated by [KWOK](https://kwok.sigs.k8s.io/), so no containers run for them.

## Prerequisites

- A running Docker engine (tested with OrbStack on Apple silicon)
- `brew install kwok`
- `kubectl` and `python3`

## Start and stop

```sh
scripts/sim-cluster.sh up
scripts/sim-cluster.sh status
scripts/sim-cluster.sh down
```

Your current kubectl context does not change. Use the `kwok-taffrail` context:

```sh
kubectl --context=kwok-taffrail logs -f pod-000000 --tail=0
```

## Create pod churn

Each call rolls a 500-pod Deployment, which produces a stream of watch events:

```sh
scripts/sim-cluster.sh churn
```

## Configure

| Variable | Default | Description |
|---|---|---|
| `SIM_CLUSTER` | `taffrail` | Cluster name; the context is `kwok-<name>`. Clusters with different names can run together |
| `SIM_NODES` | `100` | Fake nodes, each holding up to 110 pods |
| `SIM_PODS` | `5000` | Fake pods |
| `SIM_LOG_RATE` | `200` | Lines per second in the shared log file |
| `SIM_CHURN_PODS` | `500` | Pods in the churn Deployment |

## Reach 10,000 lines per second

Every pod reads the same log file, so each followed pod adds `SIM_LOG_RATE` lines per second. Follow 50 pods at the default rate.

## Baseline

Measured 2026-11-07 on an M-series Mac with 16 GB of memory and 8 cores, with OrbStack limited to about 8 GB.

| Measure | Result |
|---|---|
| `up` from empty, images cached | 43 s, all 5,000 pods `Running` |
| Cluster memory at rest | About 820 MB |
| 50 `kubectl logs -f` followers | 10,027 lines per second |
| Rolling 500 pods | About 4,000 update and delete events in 20 s |
| `kubectl get pods -A -o json`, 5,500 pods | 36 MB in 2.85 s |

## Limits

- All pods in a cluster share one log file, so their log content is identical. A pod that joins a followed tail reads that file from its start, up to the 10,000-line cap. Lines are JSON with `seq`, `level`, `route`, `status` and `ms` fields.
- Simulated pods do not reflect real workload behaviour. Use a real cluster to validate the log path before launch.
- KWOK keeps no previous container logs and reports no container IDs, so container restarts cannot be simulated. Unit tests cover them with a fake API server.
- The log file must stay in the Docker VM. On a file shared from macOS, KWOK logs `Received unexpected fsnotify event` and `kubectl logs -f` delivers no new lines.
