# Taffrail

A read-first Kubernetes desktop console built around live logs across many pods. Written in Rust on GPUI. Free, with no account and no telemetry.

**Status:** early development. There are no releases yet. The app shows live pods for a kubeconfig context and merges a live log tail across up to 50 of them.

The name comes from the taffrail log, a ship's instrument trailed off the stern.

## Docs

- [Roadmap](docs/roadmap.md): the milestones, what is out of scope, and the performance budget
- [UI design](docs/design.md): layout, keyboard, states and colour
- [Contributing](CONTRIBUTING.md): set up, run and check your change
- [Simulate a large cluster](docs/how-to/simulate-a-large-cluster.md): run 5,000 pods locally
- [Measure performance](docs/how-to/measure-performance.md): record frame times and memory
- [Preview the UI](docs/how-to/preview-the-ui.md): render the app to images
- [Decisions](docs/decisions/): why the project is built this way
- [Security policy](SECURITY.md): report a vulnerability
- [AGENTS.md](AGENTS.md): how AI agents work in this repo

## License

[Apache-2.0](LICENSE)
