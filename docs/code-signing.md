# Code signing policy

Taffrail's releases aren't code signed yet. This page says what will be signed, by whom, and what the warnings mean until then.

## Now

| Platform | Signature | What you see |
|---|---|---|
| macOS | Ad-hoc. It seals the app bundle, so macOS can tell whether it was changed, but it doesn't say who made it. | macOS asks you to allow the app once in **Privacy & Security**. See [Install Taffrail](install.md#macos). |
| Windows | None | SmartScreen warns you until the installer is signed and has a reputation. See [Install Taffrail](install.md#windows). |
| Linux | None needed | Nothing |

## Plans

- **Windows.** After the first release, Taffrail will apply for free code signing from the [SignPath Foundation](https://signpath.org/), which signs open source projects. Once approved, Windows releases will carry this notice: Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).
- **macOS.** Notarization needs a paid Apple Developer Program membership, so Mac builds stay ad-hoc signed for now. Until they are notarized, Taffrail can't be offered through Homebrew.

## What gets signed

Only the files the [Release workflow](../.github/workflows/release.yml) builds from this repository for a version tag: the Windows `-setup.exe` and `.msi` and the `taffrail.exe` inside them, and, once notarized, the Mac app and `.dmg`. Nothing built anywhere else is signed.

## Team roles

| Role | Members |
|---|---|
| Committers and reviewers | [opikdev](https://github.com/opikdev) |
| Approvers | [opikdev](https://github.com/opikdev) |

A change from anyone outside this list is reviewed before it's merged. Every release needs an approver to sign off before it's signed.

## Privacy

Taffrail talks only to your clusters, and never to the people who make it. [The privacy policy](privacy.md) lists exactly what it contacts and stores.
