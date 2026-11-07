# Preview the UI

Render the real app to PNG files, without a display or screen recording permission. This works on macOS and Linux. GPUI has no offscreen renderer on Windows.

## Render the previews

```sh
TAFFRAIL_PREVIEW_DIR=target/preview cargo run -p taffrail-ui --features preview
```

The app connects to a fake API server with sample pods and logs, runs through its main states, and writes one image for each.

| File | Shows |
|---|---|
| `logs-dark.png`, `logs-light.png` | The pod table with a log tail |
| `pods-dark.png` | The pod table alone |
| `details-summary-dark.png`, `details-events-dark.png`, `details-yaml-dark.png` | Each tab of the details panel |
| `collapsed-light.png` | The sidebar collapsed to its icon rail |
| `context-menu-dark.png` | The context picker, open |
| `namespace-dark.png`, `namespace-menu-dark.png` | The pod table filtered to one namespace, and the namespace picker open |
| `no-namespace-pods-light.png` | A chosen namespace with no pods |
| `failed-dark.png` | A cluster that can't be reached |
| `no-clusters-dark.png` | A kubeconfig with no contexts |

## Limits

- Window controls come from the system, so the traffic lights are missing from the images. The gap for them is correct.
- Tooltips and hover states appear only when something drives them, so most images don't show them.
- The window always reports a light system, so `src/preview.rs` sets each theme itself.
