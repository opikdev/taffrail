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
| `log_lost`, `log_repeated`, `log_gaps` | Lines missing from each source's simulated `seq` numbers, lines stored twice, and rows marking lines that may be missing |
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

## Measure the log timeline

Count as many lines as a full store holds over time, then time building the bars:

```sh
cargo test --release -p taffrail-core timeline_speed -- --ignored --nocapture
```

It prints the time to count each line as it is stored, the memory the counts take, and the time to build the bars the dock shows.

## Measure log reconnects

Tail 50 simulated pods through a local proxy that cuts every connection at an interval, without opening a window:

```sh
TAFFRAIL_SIM_CONTEXT=kwok-taffrail cargo test --release -p taffrail-core sim_reconnects -- --ignored --nocapture
```

It prints one JSON line with the lines stored and per second, the lines `lost` and `repeated` by each source's `seq` numbers, the `gaps` marked, and the `notes` Taffrail wrote, one per ended stream. `TAFFRAIL_SIM_SECONDS` sets the duration, 60 by default, and `TAFFRAIL_SIM_CUT_SECONDS` the interval, 10 by default or `0` for none. To test a restarted API server, run `docker restart kwok-taffrail-kube-apiserver` during a run.

## Measure the log filter

Fill a store with 1 GiB of mixed plain and JSON lines from 50 sources, then time text, regular expression, severity, JSON field and source filters over it:

```sh
cargo test --release -p taffrail-core filter_speed -- --ignored --nocapture
```

Each filter runs twice. It prints the time to the first match, to the first 50 matches, which fill the dock, and to the end of the scan, with the memory the matches and their timeline take. The store has just been written, so the OS has its files cached, as it would in the app.

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
