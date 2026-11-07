# Agent guide

AI agents do the development here. Read this first, then [the roadmap](docs/roadmap.md) and [the UI design](docs/design.md).

## Hard rules

- Never copy code from Zed's `ui`, `theme`, `editor`, `terminal` or `workspace` crates. They are GPL-3.0-or-later and this project is Apache-2.0. The `gpui` crate itself is Apache-2.0.
- Never add a telemetry, analytics or account feature.
- Keep the app read-only. Any write to a cluster needs a new decision record first.
- Hide Secret values until the user reveals them. Never write them to logs.
- Depend on `gpui-kit` with an exact `=` version pin. Do not use GPUI CE.
- This repo is public. Keep market research, competitor comparisons and product strategy out of it.

## Architecture

- `crates/core` (`taffrail-core`): cluster state, watches, log storage. It never imports GPUI.
- `crates/ui` (`taffrail-ui`, binary `taffrail`): the only crate that imports GPUI or GPUI Kit.

This boundary limits the cost of leaving GPUI, so keep it strict. `deny.toml` enforces it, along with the license rules.

## Work rules

- Look at UI changes before calling them done. See [Preview the UI](docs/how-to/preview-the-ui.md).
- Claim performance only with a measured number and the command that produced it. See [Measure performance](docs/how-to/measure-performance.md).
- Bound log storage and spill to disk. Do not hold unbounded logs in memory.
- Keep one task per branch and per pull request. Run parallel tasks in separate worktrees.
- Use `scripts/sim-cluster.sh` for scale tests. Never test against a real cluster unless the user gives you one.
- Run the checks in [CONTRIBUTING.md](CONTRIBUTING.md) before calling work done.
- Update any doc your change makes wrong in the same change.

## Commits

- Write one-line conventional subjects, such as `feat(core): add pod watcher`.
- Add no body and no trailers.
- Commit, push or open a pull request only when asked.

## Gotchas

- `kwokctl create cluster` switches your current kubectl context. `scripts/sim-cluster.sh` restores it, so pass `--context=kwok-taffrail` explicitly.
- `kubectl logs -f` on a simulated pod streams only if the log file lives inside the Docker VM. See [the how-to](docs/how-to/simulate-a-large-cluster.md).
- Do not write `use super::*` in a test module if the parent has `use gpui_kit::*`. It shadows the built-in `test` attribute, and `#[gpui_kit::test]` fails with a recursion limit error.
- `gpui-fast` is resolved even though it is unused, because the `test-support` feature names it. It pins exact versions of `taffy` and `unicode-properties`, so `cargo update` can fail on those crates.
