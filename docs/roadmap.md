# Roadmap

## First milestone

- Load contexts from kubeconfig, including exec auth.
- Show one live pod table with thousands of rows.
- Merge a live log tail across up to 50 pods, with pause and disk-backed scrollback.

## Performance targets

Measure on macOS and Windows against [the simulated cluster](how-to/simulate-a-large-cluster.md). These are design goals, not measurements.

| Measure | Load | Target |
|---|---|---|
| Table scrolling | 5,000 pods, with `sim-cluster.sh churn` running | Smooth, with frame times recorded |
| Log throughput | 50 pods, 10,000 lines per second combined | No dropped lines, UI stays responsive |
| Memory | Both loads together | Under 300 MB |
| Idle wake-ups | App open, no activity | Near zero |
