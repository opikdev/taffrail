# UI design

**Brief:** help an on-call engineer find what is wrong in a running cluster, on a desktop, in a calm and dense interface.

The signature element is the log timeline, a strip above the log lines that shows volume and severity over time. It is how you find where things went wrong before you read a single line.

## Layout

```text
+------------------------------------------------------------------+
| ● ● ●  ◧  [D] dev v                                              |  Title bar
+-----------+------------------------------------------------------+
| Workloads | Pods  All namespaces  Filter  Columns  Tail 12 pods |
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
- **New window** opens on the same context, offset from the current window. Each window has its own session, which ends when the window closes. The app quits when the last window closes.
- The app reopens where the last changed window was left, with its sidebar state, context, namespace and columns. A context that no longer exists falls back to the kubeconfig's current context, and a window that no longer fits moves back onto the display.
- This state lives in `taffrail/state.json` under the platform config directory. Apart from the context and namespace names, it holds nothing from a cluster.
- **About Taffrail**, in the app menu and the palette, shows the version, the license and a link to the repository. `Esc` closes it.

## Resource tables

- Rows have no dividers or stripes. Hover and selection separate them.
- Rows that need attention lead by default: failed, then degraded, then healthy, then finished.
- The namespace picker sits left of the filter and lists the namespaces in the cluster, with a search field. Both narrow the rows on the client, over one watch of all pods, so they narrow what a tail follows too.
- A chosen namespace stays chosen if it disappears, and the table shows the **No rows** state until it returns.
- Filters hide rows but keep the selection, so the details panel stays open on a pod the filter hides.
- **Columns** opens a menu with a check for each column. **Name** always shows. Hiding the sorted column returns to the default order.
- Below 920 px of window width, the panel header drops **Workloads /**, **Columns** shows only its icon, and the filter narrows, so the header fits the 760 px minimum.
- Clicking a header sorts by that column, then reverses it, then returns to the default order. Only the sorted column shows an arrow.
- Rows are virtual, and update in place without moving the selection, because a selection belongs to the pod and not to a position.
- Once you have scrolled, the row at the top of the view stays there while others move around it. At the very top of the list, new rows appear.
- **Tail first 50 pods** follows the rows as shown, so sorting decides which pods are tailed.
- Status shows a dot and a word in a colour, never colour alone.
- Numbers are right-aligned with tabular figures. Long names truncate in the middle, with the full name on hover.

## Details panel

| Tab | Shows |
|---|---|
| Summary | Status, owner, node, containers, labels, and links to related objects |
| Events | Recent events for this object, newest first |
| YAML | Read-only YAML with coloured keys and **Copy YAML**, without `managedFields` |

- The panel reads the pod when it opens, when its row changes, and every 10 seconds.
- Moving the selection opens that pod's details, on the Summary tab.
- If the pod is deleted, what was read stays visible with **This pod no longer exists. Showing what was last read.**
- A role that can read pods but not events still sees the Summary and YAML, and the Events tab says why it is empty.

## Log explorer

- Each line shows time, source, severity and text. Long lines clip by default, and **Wrap lines** wraps them.
- The time is the Kubernetes timestamp of the line, as `HH:MM:SS` in local time. Lines Taffrail writes itself, such as **tail ended**, have no time.
- The severity shows as a word in a status colour: **ERROR** in `error`, **WARN** in `warning`, **INFO** in `info`, and **DEBUG** in `muted`. Lines with no severity leave the column blank.
- Severity comes from a JSON level field, such as `level`, `severity`, `lvl`, `levelname`, `log.level`, or a bunyan or pino number. Plain lines are read from their first 256 bytes: a whole uppercase word such as `ERROR`, a logfmt `level=error`, a bracketed `[error]` or `[core:error]`, a klog `E1107` prefix, or a message that starts `error:`. The first match wins. Error includes fatal, critical and panic, and debug includes trace.
- A JSON line with a `msg`, `message` or `@message` field shows that message, then its other fields as `key=value` in `muted`. Level and time fields are left out, because their columns show them. A JSON line without a message, or that is not valid JSON, shows as written.
- The stored line stays as the container wrote it, with its Kubernetes timestamp, and the screen reader label reads it whole.
- The source column is hidden when you follow one pod.
- Sources get a stable colour from a fixed set, and always keep their name.
- **Tail its workload**, or `w`, follows the selected pod's controller: a Deployment, StatefulSet, DaemonSet, Job or ReplicaSet. A pod owned by a ReplicaSet follows the ReplicaSet's Deployment, so a rollout's new ReplicaSet is included.
- Typing a label selector such as `app=checkout-api` in the palette offers **Tail pods matching app=checkout-api**, in the chosen namespace.
- A followed tail adds pods that match as they appear and drops pods once they are deleted, over the same pod watch as the table. The header names what it follows: **Following deployment/checkout-api · 6 pods · 1,240 lines kept**.
- A follow tails at most 50 pods. It keeps the pods it has, fills free places with the newest, and says when pods are left out: **50 of 120 pods, the most a tail follows**.
- A pod that appears after a follow starts is read from its first line, up to 10,000 lines. Other pods start from their last 100 lines.
- Lines from a pod that has left keep its name.
- The view follows the newest lines until you scroll up or press **Pause**. Lines keep being stored while paused, and the button then reads **Follow (1,240 new)**.
- Filters narrow the lines. Search highlights matches and keeps the surrounding lines.
- The timeline bars are stacked by severity. Click a bar to jump there, or press `n` for the next error.
- A lost connection shows a marked gap in the lines, with the time range that may be missing.

## Keyboard

Every action works from the keyboard. Single-letter keys work when no text field has focus.

| Key | Action |
|---|---|
| `⌘K` / `Ctrl+K` | Open the command palette |
| `⌘B` / `Ctrl+B` | Show or hide the sidebar |
| `⌘N` / `Ctrl+N` | Open a new window |
| `⌘W` / `Ctrl+W` | Close the window |
| `⌘Q` / `Ctrl+Q` | Quit |
| `/` | Filter the current view |
| `j` `k` or arrows | Move through rows |
| `Enter` | Open details |
| `l` | Tail logs for the selection |
| `w` | Tail the selected pod's workload |
| `s` | Show the summary |
| `y` | Show YAML |
| `e` | Show events |
| `Space` | Pause or follow logs |
| `n` / `Shift+N` | Next or previous error, from milestone 3 |
| `Esc` | Clear the filter, then close the panel |
| `?` | Show all shortcuts |

## Command palette

- `⌘K`, or the search field in the title bar, opens the palette over the panel.
- Typing searches pods by name and namespace across the whole context, ignoring the filter and namespace picker. Text with a selector operator, such as `=`, `!=`, `in` or `!`, also offers to tail the pods it matches. Pods that need attention lead, and at most 20 show. Each row shows the pod's namespace on the right.
- Choosing a pod selects it and opens its details. If the filter or namespace hid it, both clear first.
- Below the pods, the palette lists the commands that apply now, then contexts and namespaces, with a check on the current ones.
- Every command dispatches the same action as its key or button.
- `Esc` clears the search, then closes the palette.
- `?` opens the list of shortcuts, which `Esc` closes.

## States

| State | What the user sees |
|---|---|
| No kubeconfig | **No clusters found.** Taffrail reads `KUBECONFIG` from your shell, or `~/.kube/config`. Add a cluster with your provider's CLI, then press **Reload contexts**. |
| Connecting | **Connecting to staging.** The sidebar footer shows **Connecting**. |
| Can't connect | **Can't reach staging**, with the error and **Try again**. |
| Login needed | **Waiting for login to staging.** Finish signing in in your browser or terminal. |
| Connection lost | **Reconnecting to staging.** Tables show their last data, marked with the time it was last updated. |
| No permission | **You can't list Secrets in this namespace.** The rest of the app keeps working. |
| No rows | **No pods in `payments`.** Offer **Show all namespaces**. |
| No matches | **No pods match "web".** Offer **Clear filter**. |
| Logs not started | The log dock stays closed, and **Tail 12 pods** sits beside the filter. It reads **Tail first 50 pods** when the limit applies. |
| No workload | **debug-shell has no workload to follow.** Shown in the log dock, which keeps any running tail. |
| Log storage full | **Oldest lines removed to stay under 1 GiB.** Shown once, in the log dock. |
| Secret hidden | `••••••••` with **Reveal value**. Values hide again when you leave the object. |

On macOS and Linux, an app opened from the Dock, Finder or a desktop launcher misses the shell's `PATH` and `KUBECONFIG`. When Taffrail does not start in a terminal, it reads them from your login shell once at start-up, along with the proxy and cloud CLI variables that exec auth plugins such as `aws`, `gke-gcloud-auth-plugin` and `kubelogin` read. It waits five seconds at most, and keeps its own environment if the shell fails. On Windows, the Start menu already passes your user environment.

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

## App icon

The icon shows the register dial of a taffrail log, with its line trailed off to a brass rotator. It sits on a dark tile on the macOS icon grid: an 824 px rounded square inside a 1024 px canvas.

The source is `crates/ui/assets/icon/icon.svg`. After changing it, run `scripts/render-icons.sh` on macOS to render the PNG, `.icns` and `.ico` files beside it, and commit them. The script needs `resvg`, from `cargo install resvg --locked`. The app's bundle identifier is `dev.opik.taffrail`.

## Accessibility

- Every control has a label, and focus is always visible in `accent`.
- Tab reaches every panel, and focus never gets trapped in a table.
- Respect the system's reduced motion and contrast settings.
- Test keyboard reach and screen reader labels at each milestone.
