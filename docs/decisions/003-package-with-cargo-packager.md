# 003. Package with cargo-packager

- **Status**: accepted
- **Context**: Version 0.1 needs installers for macOS, Windows and Linux, built from a version tag. Taffrail is a plain Rust binary, so app-framework bundlers don't apply. Writing a `.app`, `.dmg`, NSIS script, WiX project, `.deb` and AppImage by hand would mean six formats to maintain.
- **Decision**: Use [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), pinned to an exact version in the [release workflow](../../.github/workflows/release.yml), with its settings under `[package.metadata.packager]` in `crates/ui/Cargo.toml`. It builds the `.app` and `.dmg`, the NSIS `-setup.exe`, the WiX `.msi`, the `.deb` and the AppImage from one config. The workflow builds the binaries itself, joins the two Mac architectures with `lipo`, and packs the Linux `.tar.gz` with `tar`.
- **Consequences**:
  - It is Apache-2.0 or MIT, and it runs only in CI, so it adds nothing to the app's dependencies.
  - It ad-hoc signs the Mac app, and can sign with a Developer ID later through the same config.
  - It downloads NSIS, WiX and linuxdeploy at build time, so a release build needs the network.
  - It names the outputs and lays out the bundles. If that stops fitting, each format can move to its own tool without changing the app.
