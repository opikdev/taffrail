# Release Taffrail

The [Release workflow](../.github/workflows/release.yml) builds every installer. [ADR 003](decisions/003-package-with-cargo-packager.md) explains the tooling.

| Platform | Files |
|---|---|
| macOS | `Taffrail_<VERSION>_universal.dmg`, for Apple silicon and Intel |
| Windows | `taffrail_<VERSION>_x64-setup.exe`, `taffrail_<VERSION>_x64_en-US.msi` |
| Linux | `taffrail_<VERSION>_amd64.deb`, `taffrail_<VERSION>_x86_64.AppImage`, `taffrail_<VERSION>_x86_64.tar.gz` |

Each build job also checks its own output. The Mac app must be sealed by its ad-hoc signature, in the bundle and in the `.dmg`. Both Windows installers must install. The `.deb` must install on a clean Debian with every library it links. Each app must still be running 10 seconds after it starts.

## Publish a version

1. Set the new version in `[workspace.package]` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and merge that change to `main`.
2. Tag the merged commit and push the tag: `git tag v<VERSION> && git push origin v<VERSION>`.
3. Wait for the workflow to finish. It stops if the tag doesn't match the version in `Cargo.toml`, and otherwise creates a draft release with every installer attached.
4. Download each installer and check that it installs and opens, following [Install Taffrail](install.md).
5. Write the release notes in the draft: what changed, and anything people must do differently.
6. Publish the draft.
7. Check that the published release's address ends in `/tag/v<VERSION>`.

Editing a draft through the API without `tag_name` resets its tag to `untagged-...`, and publishing it then creates that tag. Always send both:

```sh
gh api -X PATCH repos/opikdev/taffrail/releases/<RELEASE_ID> -f tag_name=v<VERSION> -F body=@notes.md
```

If a release went out on an `untagged-...` tag, run the same command without `body`, then delete the stray tag with `git push origin :refs/tags/untagged-<ID>`.

## Build installers without a release

Run the workflow from the Actions tab, or with `gh workflow run release.yml --ref <BRANCH>`. The installers are attached to the run as artifacts, and no release is created.

## Signing

Releases aren't code signed yet. See the [code signing policy](code-signing.md) for what is signed now and what comes next.
