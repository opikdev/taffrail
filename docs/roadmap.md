# Roadmap

Taffrail helps you find what is wrong in a running cluster, starting from its logs. Version 0.1 is read-only. See [the UI design](design.md) for how each feature looks and behaves.

Each milestone ends with `scripts/bench.sh` inside the [performance budget](#performance-budget), and with its tests and docs merged.

| Milestone | Outcome | Status |
|---|---|---|
| 1. Engine | Contexts, a live pod table and a merged log tail | Done |
| 2. App shell | The layout, navigation and keyboard model everything else lives in | Next |
| 3. Log explorer | Find a failure across pods without leaving the app | Planned |
| 4. Cluster coverage | Read every common resource, plus any custom resource | Planned |
| 5. Version 0.1 | People can install it, trust it and update it | Planned |

## Milestone 2: App shell

Build this first, because every later feature depends on its layout.

- Add the theme tokens from the design, with light and dark themes that follow the system.
- Add the shell: title bar, sidebar, content area, details panel, log dock and status bar.
- Save the window layout, the last context and the last namespace.
- Select rows by object identity, so a selection stays on its pod while rows change.
- Add sorting, a namespace filter and custom column visibility to the pod table.
- Add a details panel with a summary, events and read-only YAML.
- Add the command palette and the keyboard map.
- Open one context per window, and add **New window**.

**Done when:** you can reach any pod, read its YAML and open its logs without a mouse.

## Milestone 3: Log explorer

- Follow a workload or label selector, so pods that appear during a rollout join the tail.
- Pick sources from selected rows, a filter or a workload.
- Close reconnect gaps by resuming from the last timestamp, and mark anything that stays missing.
- Parse timestamps and severity, including from JSON lines.
- Add text, regular expression, severity and JSON field filters over all stored lines.
- Add the timeline, and jump to the next error.
- Show lines around a match, and copy or export lines.

**Done when:** you can start from a failing Deployment and find its first error in a rollout, across 50 pods, at 10,000 lines per second.

## Milestone 4: Cluster coverage

- Show live tables for Deployments, StatefulSets, DaemonSets, Jobs, CronJobs, Services, Ingresses, ConfigMaps, Secrets, Nodes, Namespaces and Events.
- Discover custom resources, and show them in generic tables.
- Add owner links, such as from a Pod to its Deployment and its Node.
- Keep Secret values hidden until revealed.
- Show permission errors per resource, without failing the whole view.

**Done when:** a user with limited permissions can browse a cluster and see plainly what they cannot read.

## Milestone 5: Version 0.1

- Sign and notarize macOS builds, and sign Windows builds.
- Add release builds, an update check and a changelog.
- Review accessibility: keyboard reach, focus order, screen reader labels and input methods.
- Test on a real cluster, which the project owner provides.

**Done when:** someone else can install it and tail logs from their own cluster in five minutes.

## Not in version 0.1

| Feature | Decision |
|---|---|
| Edit, apply, scale, delete, restart | Needs a [decision record](decisions/002-read-only-with-no-telemetry.md) with a per-context write mode |
| Shell into a container | Run `kubectl exec` in your own terminal |
| Metrics charts | First candidate after 0.1 |
| Helm, cluster maps, topology views | Revisit after 0.1 |
| Extensions | Wait until the core API has settled |
| AI features, web and mobile versions | Not planned |

## Performance budget

Run [the benchmark](how-to/measure-performance.md) on macOS. Measure Windows if users report problems, or before publishing a Windows claim.

| Measure | Load | Budget | Measured at milestone 1 |
|---|---|---|---|
| Table scrolling | 5,500 pods with rollouts | No more than 1% of frames over 17 ms | 0 of 2,401 |
| Log throughput | 50 pods, 10,000 lines per second | No dropped lines | 0 gaps in 604,780 lines |
| Memory | Both loads together | Under 300 MB | 220 MB peak |
| Idle wake-ups | App open, no activity | Under 1 per second | 0.4 |
| Filter stored lines | 1 GiB of logs | First results within 100 ms, a full scan within 5 s | Not yet built |
