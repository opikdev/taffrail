# Privacy policy

Taffrail has no account, analytics, crash reports or update checks. It never sends anything to the people who make it.

## What Taffrail contacts

- **The Kubernetes API servers in your kubeconfig.** Taffrail reads the kubeconfig named by `KUBECONFIG`, or `~/.kube/config`, and connects only to the server of each context you open. From it, Taffrail reads the server version, lists and watches pods, and reads a pod's details, its events and its logs.
- **Any login plugin your kubeconfig runs.** If a context gets its token from an `exec` command, such as a cloud provider's CLI, Taffrail runs that command as your kubeconfig says. The plugin may contact its own sign-in service, and that provider's privacy policy covers it.

Taffrail contacts nothing else.

## What Taffrail runs

- **Your login shell, once at start-up**, on macOS and Linux when you open Taffrail from the Dock, Finder or a desktop launcher rather than a terminal. Apps opened that way miss your shell's `PATH` and `KUBECONFIG`, so login plugins aren't found. Taffrail keeps only `PATH`, `KUBECONFIG`, `HOME`, the proxy variables, and variables starting with `AWS_`, `AZURE_`, `AAD_`, `CLOUDSDK_`, `GOOGLE_`, `DIGITALOCEAN_` or `OCI_`. It uses `KUBECONFIG` to find your kubeconfig, passes the rest only to login plugins, and never saves or logs them. If the shell takes more than five seconds or fails, Taffrail uses its own environment instead.
- **Your kubeconfig's login plugins**, as described above.

## What Taffrail never does

- It never writes to a cluster. It doesn't create, change, scale, restart or delete anything, and it has no shell into containers.
- It never copies, saves or logs your credentials. They stay in your kubeconfig, and Taffrail never changes that file.
- It never sends what it reads from a cluster anywhere else.

## What stays on your computer

- **Window state**, in `taffrail/state.json` under your config folder, listed under [Remove everything](#remove-everything). It holds the window's position and size, the display it was on, whether the sidebar is collapsed, the last context and namespace names, and which columns you hid. It holds nothing else from a cluster.
- **Log lines you tail**, in unnamed temporary files, at most 1 GiB for each tail. The system deletes them once Taffrail no longer needs them, or when it quits, even after a crash.
- **YAML you copy.** **Copy YAML** puts a pod's YAML on your clipboard, only when you press it.

## Remove everything

First uninstall the app as [Install Taffrail](install.md#uninstall) describes, or delete the folder you built it in. Uninstalling leaves the state file behind, so then delete the `taffrail` folder that holds it:

- macOS: `~/Library/Application Support/taffrail`
- Windows: `%APPDATA%\taffrail`
- Linux: `~/.config/taffrail`, or `$XDG_CONFIG_HOME/taffrail` if you set `XDG_CONFIG_HOME`

Nothing else is left behind.

## Questions

[Open an issue](https://github.com/opikdev/taffrail/issues) if something here is unclear, or if Taffrail does something this page doesn't describe.
