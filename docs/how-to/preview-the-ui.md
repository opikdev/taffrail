# Preview the UI

Render the real app to PNG files, without a display or screen recording permission. This works on macOS and Linux. GPUI has no offscreen renderer on Windows.

## Render the previews

```sh
TAFFRAIL_PREVIEW_DIR=target/preview cargo run -p taffrail-ui --features preview
```

The app connects to a fake API server with sample pods and logs, runs through its main states, and writes one image for each.

| File | Shows |
|---|---|
| `readme-dark.png` | The README screenshot: one namespace, the details panel and a live tail across its pods |
| `logs-dark.png`, `logs-light.png` | The pod table with a log tail |
| `pods-dark.png` | The pod table alone |
| `details-summary-dark.png`, `details-events-dark.png`, `details-yaml-dark.png` | Each tab of the details panel |
| `log-formats-dark.png`, `log-formats-light.png` | A tail of plain, logfmt, klog, nginx and JSON lines, with their severities |
| `log-gap-dark.png`, `log-gap-light.png` | A tail whose logs rotated while it reconnected, with the missing lines marked |
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
