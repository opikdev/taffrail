# UI design

**Brief:** help an on-call engineer find what is wrong in a running cluster, on a desktop, in a calm and dense interface.

The signature element is the log timeline, a strip above the log lines that shows volume and severity over time. It is how you find where things went wrong before you read a single line.

## Layout

```text
+------------------------------------------------------------------+
| Context v   Namespace v              Search or run a command  ⌘K |  Title bar
+----------+--------------------------------------+----------------+
| Workloads| Filter pods                  Columns | api-7f9c-x2k   |
|  Pods    +--------------------------------------+ Summary        |
|  Deploys | Name       Status     Ready  Age     | Events         |  Sidebar,
| Network  | api-7f9c   Running    1/1    2h      | YAML           |  content,
| Config   | web-58d1   CrashLoop  0/1    4m      |                |  details
| Cluster  |                                      |                |
+----------+--------------------------------------+----------------+
| Logs  Following 12 pods   Filter lines      Errors only    Pause |
| ▁▁▂▁▁▁▂▁▁▁▁▂▃█▇▂▁▁▁▂▁  timeline                                  |  Log dock
| 10:42:07  api-7f9c  ERROR  connection refused                    |
+------------------------------------------------------------------+
| Connected · 5,500 pods · 10,214 lines/s · 412 MB of 1 GiB kept   |  Status bar
+------------------------------------------------------------------+
```

- The sidebar lists resources by group, and collapses to icons with labels on hover.
- The details panel opens when you select a row, and closes with `Esc`.
- The log dock stays open while you move between views. It can be resized or maximized.
- Each window shows one context. Use **New window** to compare clusters.

## Resource tables

- Rows are virtual, sort by any column, and update in place without moving the selection.
- A row keeps its position under the pointer while live updates arrive, and new rows appear in sort order.
- Status shows a word and a colour, never colour alone.
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
| Connecting | The context name with progress in the status bar. The last view stays visible. |
| Login needed | **Waiting for login to staging.** Finish signing in in your browser or terminal. |
| Connection lost | **Reconnecting to staging.** Tables show their last data, marked with the time it was last updated. |
| No permission | **You can't list Secrets in this namespace.** The rest of the app keeps working. |
| No rows | **No pods in `payments`.** Offer **Show all namespaces**. |
| No matches | **No pods match "web".** Offer **Clear filter**. |
| Logs not started | **Choose pods to follow.** Offer **Tail selected pods**. |
| Log storage full | **Oldest lines removed to stay under 1 GiB.** Shown once, in the status bar. |
| Secret hidden | `••••••••` with **Reveal value**. Values hide again when you leave the object. |

## Words

- Use one term per concept: **context**, not cluster profile; **tail**, not stream; **filter** hides lines, and **search** only highlights them.
- Buttons name the action: **Tail selected pods**, **Reveal value**, **Reload contexts**.
- Errors say what happened, why if known, and what to do next.

## Colour

Light and dark themes follow the system, and both are set through the GPUI Kit theme. Text sizes come from the theme, and spacing uses a 4 px scale.

| Token | Dark | Light | Use |
|---|---|---|---|
| `surface` | `#14181D` | `#F7F6F3` | Window background |
| `raised` | `#1C2229` | `#FFFFFF` | Panels and menus |
| `selected` | `#2A3038` | `#EBE7DE` | Selected rows |
| `text` | `#E6E9ED` | `#1C2127` | Primary text |
| `muted` | `#9AA5B1` | `#56606B` | Secondary text |
| `accent` | `#E0AE4B` | `#7A5200` | Focus, active items |
| `error` | `#F4837D` | `#B3261E` | Failed states |
| `warning` | `#F0975A` | `#964A00` | Degraded states |
| `success` | `#6CC07A` | `#1E7038` | Healthy states |
| `info` | `#7DB7E8` | `#1F5F99` | Neutral notices |

Every foreground colour measures at least 4.5:1 against `surface`, `raised` and `selected`.

## Accessibility

- Every control has a label, and focus is always visible in `accent`.
- Tab reaches every panel, and focus never gets trapped in a table.
- Respect the system's reduced motion and contrast settings.
- Test keyboard reach and screen reader labels at each milestone.
