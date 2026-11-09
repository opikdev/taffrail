# Measure performance

Measure frame times, memory, CPU and wake-ups against a simulated cluster. Use a release build, on AC power, with no other heavy work running. Keep the app window uncovered for the whole run.

## Run the benchmark

Start a [simulated cluster](simulate-a-large-cluster.md), then run:

```sh
scripts/bench.sh
```

Each run scrolls the pod table three rows per frame while pods churn: once alone, once with a log tail across 50 pods, and once following the churning Deployment's pods with `app=sim-churn`, so pods join and leave the tail during rollouts. It prints two JSON lines per run:

| Output | Meaning |
|---|---|
| `frames`, `p50_ms`, `p95_ms`, `p99_ms`, `max_ms` | Time between presented frames |
| `over_17ms`, `over_33ms` | Frames slower than these limits |
| `log_sources`, `log_lines` | Containers the tail has streamed in all, and the last line number |
| `followed_pods`, `matching_pods` | Pods a follow tails at the end, and pods that match |
| `footprint_mb`, `peak_footprint_mb` | Physical footprint, the figure Activity Monitor shows |
| `cpu_percent` | CPU time as a percentage of one core |
| `wakeups_per_second` | Idle and interrupt wake-ups |

Pass a duration in seconds to change the default of 20: `scripts/bench.sh 60`. For a cluster started with another `SIM_CLUSTER`, pass the same name: `SIM_CLUSTER=m3 scripts/bench.sh`.

## Measure the log parser

Time finding the severity and timestamp of a million lines in common formats:

```sh
cargo test --release -p taffrail-core _speed -- --ignored --nocapture --test-threads=1
```

`parse_speed` prints the time per line. `store_speed` prints the time to find severities, store, read, and find them again on read, and the share of disk the stored severity takes.

## Sample a running process

On macOS, sample any process and its descendants:

```sh
scripts/sample-process-tree.py <pid> [<pid> ...] --seconds 20
```

## Avoid false results

- `--features bench` removes GPUI's frame limits. Without it, an unfocused window runs at 30 fps and every frame measures 33 ms.
- `--features bench` also ignores saved state, so a run opens at the default size on the kubeconfig's current context.
- Footprint moves in steps of one window buffer, because the GPU holds a varying number of them, and each one scales with the window. Run `footprint <pid>` and read `IOSurface` to see this. Repeat a run, and compare apps at the same window size.
- `top` under-reports idle wake-ups. Use `sample-process-tree.py`, which reads `proc_pid_rusage`.
- macOS stops frames for a covered window. A run fails with `Invalid run` if frames stop, rather than reporting a false result.
- Apps built on WebKit run helper processes under `launchd`, so a process tree misses them. Pass their PIDs too.

## Configure

| Variable | Default | Description |
|---|---|---|
| `TAFFRAIL_BENCH_SECONDS` | `30` | How long to scroll |
| `TAFFRAIL_BENCH_TAIL` | `1` | Set to `0` to skip the log tail |
| `TAFFRAIL_BENCH_FOLLOW` | Empty | A label selector to follow instead of tailing the first 50 pods |
