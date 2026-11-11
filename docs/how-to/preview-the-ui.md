# Preview the UI

Render the real app to PNG files, without a display or screen recording permission. This works on macOS and Linux. GPUI has no offscreen renderer on Windows.

## Render the previews

```sh
TAFFRAIL_PREVIEW_DIR=target/preview cargo run -p taffrail-ui --features preview
```

The app connects to a fake API server with sample pods and logs, runs through its main states, and writes one image for each.

With `TAFFRAIL_PREVIEW_CONTEXT` set to a simulated context, such as `kwok-rbac-jane`, it renders that cluster instead. See [Browse as a user with limited permissions](simulate-a-large-cluster.md#browse-as-a-user-with-limited-permissions).

| File | Shows |
|---|---|
| `readme-dark.png` | The README screenshot: one namespace, the details panel and a live tail across its pods |
| `logs-dark.png`, `logs-light.png` | The pod table with a log tail |
| `pods-dark.png` | The pod table alone |
| `details-summary-dark.png`, `details-events-dark.png`, `details-yaml-dark.png` | Each tab of the details panel |
| `log-formats-dark.png`, `log-formats-light.png` | A tail of plain, logfmt, klog, nginx and JSON lines, with their severities |
| `log-gap-dark.png`, `log-gap-light.png` | A tail whose logs rotated while it reconnected, with the missing lines marked |
| `log-gap-filtered-dark.png` | The same tail at **ERROR only**, where the gaps still show |
| `log-timeline-dark.png`, `log-timeline-light.png` | Ten minutes of ingress lines with a burst of errors and a minute marked missing, on the timeline |
| `log-timeline-error-dark.png` | The same tail after `Shift+N`, with the last error marked |
| `log-timeline-filtered-dark.png` | The same tail filtered to `refused`, with the timeline counting only matches |
| `log-context-dark.png`, `log-context-light.png` | The same tail filtered to `refused` with **Context: 3 lines**, after `Shift+N` twice |
| `log-context-narrow-dark.png` | The same at the 760 px minimum width, with the compact dock header |
| `log-filter-dark.png`, `log-filter-light.png` | A tail filtered to `postgres` at **WARN and above**, with its count |
| `log-no-matches-dark.png` | A log filter with no matching lines |
| `log-filter-narrow-dark.png` | The log dock header and timeline with a filter at the 760 px minimum width |
| `follow-dark.png` | A tail following the selected pod's Deployment across two ReplicaSets |
| `palette-dark.png`, `palette-search-dark.png` | The command palette, empty and searching |
| `shortcuts-light.png` | The shortcut sheet |
| `about-dark.png`, `about-light.png` | The **About Taffrail** sheet |
| `collapsed-light.png` | The sidebar collapsed to its icon rail |
| `context-menu-dark.png` | The context picker, open |
| `namespace-dark.png`, `namespace-menu-dark.png` | The pod table filtered to one namespace, and the namespace picker open |
| `no-namespace-pods-light.png` | A chosen namespace with no pods |
| `columns-hidden-dark.png`, `columns-menu-dark.png` | The pod table with columns hidden, and the **Columns** menu open |
| `narrow-dark.png`, `narrow-columns-menu-dark.png` | The panel header at the 760 px minimum width, and its **Columns** menu open |
| `deployments-dark.png`, `deployments-light.png` | The Deployments table, with stalled, unavailable and progressing rollouts first |
| `owner-links-dark.png`, `owner-links-light.png` | A pod's Summary, linking to its Deployment via its ReplicaSet, with its Node as plain text until Nodes have a view |
| `owner-link-followed-dark.png`, `owner-link-followed-light.png` | The Deployment after following that link, with **Back** in the panel header |
| `deployment-details-dark.png`, `deployment-details-light.png` | A stalled Deployment's details |
| `statefulsets-dark.png`, `statefulsets-light.png` | The StatefulSets table, with an unavailable set, one rolling out, one not ready and one outdated first |
| `statefulset-details-dark.png`, `statefulset-details-light.png` | An unavailable StatefulSet's details |
| `daemonsets-dark.png`, `daemonsets-light.png` | The DaemonSets table, with a set that has no ready pod, a misscheduled one and one rolling out first |
| `daemonset-details-dark.png`, `daemonset-details-light.png` | That DaemonSet's details, with its node selector and tolerations |
| `jobs-dark.png`, `jobs-light.png` | The Jobs table, with failed Jobs and their reasons, then one retrying, first |
| `job-details-dark.png`, `job-details-light.png` | A failed Job's details, with its backoff limit |
| `no-permission-dark.png`, `no-permission-light.png` | The Deployments view when the API refuses to list Deployments, with the lock in the sidebar |
| `nodes-dark.png`, `nodes-light.png` | The Nodes table, with a node not ready, one under disk pressure and one cordoned first |
| `node-details-dark.png`, `node-details-light.png` | A node that is not ready, with its details |
| `no-permission-nodes-dark.png`, `no-permission-nodes-light.png` | The Nodes view when the API refuses to list nodes |
| `namespaces-dark.png`, `namespaces-light.png` | The Namespaces table, with a terminating namespace first and pod counts |
| `namespace-details-dark.png`, `namespace-details-light.png` | The terminating namespace's details, with the conditions that hold it |
| `failed-dark.png` | A cluster that can't be reached |
| `no-clusters-dark.png` | A kubeconfig with no contexts |

## Update the README screenshot

```sh
scripts/screenshot.sh
```

The script renders the previews in UTC, so log times match the sample timestamps on any machine. It resizes `readme-dark.png` to 1,600 px wide with `sips` on macOS or ImageMagick elsewhere, and writes it to `docs/images/taffrail.png`. Look at the image before you commit it.

## Limits

- Window controls come from the system, so the traffic lights are missing from the images. The gap for them is correct.
- Tooltips and hover states appear only when something drives them, so most images don't show them.
- The window always reports a light system, so `src/preview.rs` sets each theme itself.
