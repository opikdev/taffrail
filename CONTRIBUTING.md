# Contributing

Read [AGENTS.md](AGENTS.md) first. Its rules apply to people and agents alike.

## Set up

1. Install [rustup](https://rustup.rs/). `rust-toolchain.toml` selects the compiler.
2. Install the platform toolchain: Xcode on macOS, or Visual Studio with the C++ workload on Windows.
3. Run `cargo install cargo-deny --locked`.

To test against a large cluster, see [Simulate a large cluster](docs/how-to/simulate-a-large-cluster.md).

## Run the app

```sh
cargo run
```

## Check your change

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo deny --locked --workspace check
```

CI runs these on macOS and Windows, except `cargo deny`, which runs once on Linux.

## Open a pull request

Keep one task per branch and per pull request, with a one-line conventional commit subject such as `feat(core): add pod watcher`.

Contributions are licensed under [Apache-2.0](LICENSE).
