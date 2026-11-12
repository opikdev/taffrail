# Security policy

Taffrail handles cluster credentials, so treat any way to expose them as a vulnerability.

## How Taffrail handles credentials

Version 0.1 is checked against these guarantees. A way around any of them is a vulnerability.

- **Credentials stay in your kubeconfig.** Taffrail reads the files `KUBECONFIG` names, or `~/.kube/config`, and never writes to them. It never copies a token, password, client key or certificate anywhere: not to a file, a log, the clipboard or the window state file.
- **Error messages quote no credentials.** A broken kubeconfig is reported by line and column, without the text around the error. A login plugin that fails is reported by its exit code, without its command line, its environment or what it printed. Taffrail never stores a plugin's output; the token it prints goes only to the API server.
- **Login plugins get only what they need from your shell.** Outside a terminal, Taffrail passes them the variables [the privacy policy](docs/privacy.md#what-taffrail-runs) lists, and never logs or saves them. A plugin that may prompt you writes its messages to Taffrail's own standard error, as it would to a terminal under `kubectl`.
- **It never writes to a cluster.** It only gets, lists and watches objects and reads pod logs. It never creates, changes or deletes anything, never runs `exec`, `attach` or port forwarding, and never evicts a pod.
- **It never reads Secrets.** No view, YAML or link fetches a Secret object. A pod's YAML shows the names an `env` entry reads from a Secret, as the pod spec states them, never the values.
- **TLS is verified.** Taffrail checks every API server's certificate, and skips the check only for a cluster whose kubeconfig sets `insecure-skip-tls-verify: true`.
- **What it writes is yours alone.** On macOS and Linux, the window state file, the temporary files that hold tailed log lines, and a new export file are readable by your user only. The temporary files have no name and are deleted when Taffrail no longer needs them, even after a crash.

## Report a vulnerability

Use [private vulnerability reporting](https://github.com/opikdev/taffrail/security/advisories/new). Do not open a public issue.

## Supported versions

Only the latest release gets security fixes, currently 0.1.x. Reports about the `main` branch are welcome too.
