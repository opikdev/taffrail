# Privacy policy

Taffrail has no account, analytics, crash reports or update checks. It never sends anything to the people who make it.

## What Taffrail contacts

- **The Kubernetes API servers in your kubeconfig.** Taffrail reads the kubeconfig named by `KUBECONFIG`, or `~/.kube/config`, and connects only to the server of each context you open. From it, Taffrail reads the server version, lists and watches the objects in the views you open, such as pods, workloads, nodes, namespaces and events, and reads an object's details, its events and a pod's logs. It never reads Secrets.
- **Any login plugin your kubeconfig runs.** If a context gets its token from an `exec` command, such as a cloud provider's CLI, Taffrail runs that command as your kubeconfig says. The plugin may contact its own sign-in service, and that provider's privacy policy covers it.

Taffrail contacts nothing else.

## What Taffrail runs

- **Your login shell, once at start-up**, on macOS and Linux when you open Taffrail from the Dock, Finder or a desktop launcher rather than a terminal. Apps opened that way miss your shell's `PATH` and `KUBECONFIG`, so login plugins aren't found. Taffrail keeps only `PATH`, `KUBECONFIG`, `HOME`, the proxy variables, and variables starting with `AWS_`, `AZURE_`, `AAD_`, `CLOUDSDK_`, `GOOGLE_`, `DIGITALOCEAN_` or `OCI_`. It uses `KUBECONFIG` to find your kubeconfig, passes the rest only to login plugins, and never saves or logs them. If the shell takes more than five seconds or fails, Taffrail uses its own environment instead.
- **Your kubeconfig's login plugins**, as described above.

## What Taffrail never does

- It never writes to a cluster. It doesn't create, change, scale, restart or delete anything, and it has no shell into containers.
- It never copies, saves or logs your credentials, or shows them in an error message. They stay in your kubeconfig, and Taffrail never changes that file. [How Taffrail handles credentials](../SECURITY.md#how-taffrail-handles-credentials) lists what it guarantees.
- It never sends what it reads from a cluster anywhere else.

## What stays on your computer

- **Window state**, in `taffrail/state.json` under your config folder, listed under [Remove everything](#remove-everything). It holds the window's position and size, the display it was on, whether the sidebar is collapsed, the last context and namespace names, the last resource view, and which columns you hid in each view. It holds nothing else from a cluster.
- **Log lines you tail**, in unnamed temporary files that only your user can read, at most 1 GiB for each tail. The system deletes them once Taffrail no longer needs them, or when it quits, even after a crash.
- **What you copy.** **Copy YAML** puts a pod's YAML on your clipboard, and `⌘C` or `Ctrl+C` copies the log lines you selected. Taffrail only does this when you ask.
- **Log lines you export.** **Export lines** writes the lines in view to a file you choose in a save dialog. On macOS and Linux, a new export file is readable by your user only. Taffrail never writes an export anywhere else, and deletes a partial file if the export stops.

## Remove everything

First uninstall the app as [Install Taffrail](install.md#uninstall) describes, or delete the folder you built it in. Uninstalling leaves the state file behind, so then delete the `taffrail` folder that holds it:

- macOS: `~/Library/Application Support/taffrail`
- Windows: `%APPDATA%\taffrail`
- Linux: `~/.config/taffrail`, or `$XDG_CONFIG_HOME/taffrail` if you set `XDG_CONFIG_HOME`

Nothing else is left behind.

## Questions

[Open an issue](https://github.com/opikdev/taffrail/issues) if something here is unclear, or if Taffrail does something this page doesn't describe.
