# UI design

**Brief:** help an on-call engineer find what is wrong in a running cluster, on a desktop, in a calm and dense interface.

The signature element is the log timeline, a strip above the log lines that shows volume and severity over time. It is how you find where things went wrong before you read a single line.

## Layout

```text
+------------------------------------------------------------------+
| ● ● ●  ◧  [D] dev v                                              |  Title bar
+-----------+------------------------------------------------------+
| Workloads | Workloads / Pods          Filter pods   Tail 12 pods |
|  Pods 412 +------------------------------------+-----------------+
|  Deploys  | Name         Status       Ready  Age | api-7f9c-x2k  |  Sidebar,
|           | api-7f9c     ● Running    1/1    2h  | Summary       |  panel,
| Network   | web-58d1     ● CrashLoop  0/1    4m  | Events        |  details
| Config    |                                      | YAML          |
| Cluster   +------------------------------------+-----------------+
|           | Logs  Tailing 12 containers           Filter   Pause |
|           | ▁▁▂▁▁▁▂▁▁▁▁▂▃█▇▂▁▁▁▂▁  timeline                      |  Log dock
| ● v1.35.0 | 10:42:07  api-7f9c  ERROR  connection refused        |
+-----------+------------------------------------------------------+
```

- The title bar and sidebar sit on the window canvas. Everything you work in sits in one raised panel with a hairline border, inset from the window edge.
- The title bar holds the context picker, with a brass badge showing the context's first letter.
- The sidebar lists resources by group, with a neutral pill on the current one, and collapses to an icon rail. Resources that don't exist yet stay disabled.
- The sidebar footer shows the connection. Anything that needs explaining appears in the panel, where there is room for a next step.
- The details panel opens when you select a row, and closes with `Esc`.
- The log dock stays open while you move between views. It can be resized or maximized.
- Each window shows one context. Use **New window** to compare clusters.

## Resource tables

- Rows have no dividers or stripes. Hover and selection separate them.
- Rows are virtual, sort by any column, and update in place without moving the selection.
- A row keeps its position under the pointer while live updates arrive, and new rows appear in sort order.
- Status shows a dot and a word in a colour, never colour alone.
- Numbers are right-aligned with tabular figures. Long names truncate in the middle, with the full name on hover.

## Details panel

| Tab | Shows |
|---|---|
| Summary | Status, owner, node, containers, labels, and links to related objects |
| Events | Recent events for this object, newest first |
| YAML | Read-only YAML with syntax highlighting |

## Log explorer

- Each line shows time, source, severity and text. Long lines clip by default, and **Wrap lines** wraps them.
- The source column is hidden when you follow one pod.
- Sources get a stable colour from a fixed set, and always keep their name.
- The view follows the newest lines until you scroll up or press **Pause**. Lines keep being stored while paused, and the button then reads **Follow (1,240 new)**.
- Filters narrow the lines. Search highlights matches and keeps the surrounding lines.
- The timeline bars are stacked by severity. Click a bar to jump there, or press `n` for the next error.
- A lost connection shows a marked gap in the lines, with the time range that may be missing.

## Keyboard

Every action works from the keyboard. Single-letter keys work when no text field has focus.

| Key | Action |
|---|---|
| `⌘K` / `Ctrl+K` | Open the command palette |
| `/` | Filter the current view |
| `j` `k` or arrows | Move through rows |
| `Enter` | Open details |
| `l` | Tail logs for the selection |
| `y` | Show YAML |
| `e` | Show events |
| `Space` | Pause or follow logs |
| `n` / `Shift+N` | Next or previous error |
| `Esc` | Clear the filter, then close the panel |
| `?` | Show all shortcuts |

## States

| State | What the user sees |
|---|---|
| No kubeconfig | **No clusters found.** Taffrail reads `~/.kube/config` and `KUBECONFIG`. Add a cluster with your provider's CLI, then press **Reload contexts**. |
| Connecting | **Connecting to staging.** The sidebar footer shows **Connecting**. |
| Can't connect | **Can't reach staging**, with the error and **Try again**. |
| Login needed | **Waiting for login to staging.** Finish signing in in your browser or terminal. |
| Connection lost | **Reconnecting to staging.** Tables show their last data, marked with the time it was last updated. |
| No permission | **You can't list Secrets in this namespace.** The rest of the app keeps working. |
| No rows | **No pods in `payments`.** Offer **Show all namespaces**. |
| No matches | **No pods match "web".** Offer **Clear filter**. |
| Logs not started | The log dock stays closed, and **Tail 12 pods** sits beside the filter. It reads **Tail first 50 pods** when the limit applies. |
| Log storage full | **Oldest lines removed to stay under 1 GiB.** Shown once, in the log dock. |
| Secret hidden | `••••••••` with **Reveal value**. Values hide again when you leave the object. |

## Words

- Use one term per concept: **context**, not cluster profile; **tail**, not stream; **filter** hides lines, and **search** only highlights them.
- Buttons name the action: **Tail 12 pods**, **Reveal value**, **Reload contexts**.
- Errors say what happened, why if known, and what to do next.

## Colour

The chrome is monochrome, so colour always means something. Brass marks focus and the context badge, and status colours mark state.

Light and dark themes follow the system. Both live in `crates/ui/themes/taffrail.json`, which maps these tokens onto the GPUI Kit theme. Spacing uses a 4 px scale, and text sizes come from the theme.

| Token | Dark | Light | Use |
|---|---|---|---|
| `canvas` | `#101113` | `#EFEEEA` | Window, title bar, sidebar |
| `panel` | `#17181A` | `#FBFAF8` | The content panel |
| `raised` | `#1E1F22` | `#FFFFFF` | Menus and popovers |
| `hover` | `#232427` | `#F1EFEA` | Hovered rows |
| `selected` | `#2A2B2F` | `#E7E4DC` | Selected rows and the current sidebar item |
| `line` | `#26272A` | `#E2DFD8` | Hairline borders |
| `text` | `#E9E9E7` | `#1B1C1E` | Primary text |
| `muted` | `#9C9DA1` | `#5C5D61` | Secondary text |
| `accent` | `#E0AE4B` | `#7A5200` | Focus, context badge |
| `error` | `#F4837D` | `#B3261E` | Failed states |
| `warning` | `#F0975A` | `#964A00` | Degraded states |
| `success` | `#6CC07A` | `#1E7038` | Healthy states |
| `info` | `#7DB7E8` | `#1F5F99` | Neutral notices |

A test checks that every text colour measures at least 4.5:1 on every surface, in both themes.

## Accessibility

- Every control has a label, and focus is always visible in `accent`.
- Tab reaches every panel, and focus never gets trapped in a table.
- Respect the system's reduced motion and contrast settings.
- Test keyboard reach and screen reader labels at each milestone.
