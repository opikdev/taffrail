#!/usr/bin/env bash
# Write LICENSE and THIRD-PARTY-LICENSES.html, the licenses of every crate compiled into the
# taffrail binary, into a directory:
# scripts/third-party-licenses.sh [<out-dir>] [<target>...]
# The directory defaults to target/licenses and the target to this machine's. Pass both Mac
# targets for the universal binary. Needs cargo-about, at the version the release workflow pins.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/licenses}"
shift || true
targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  targets=("$(rustc -vV | sed -n 's/^host: //p')")
fi

target_args=()
for target in "${targets[@]}"; do
  target_args+=(--target "$target")
done

mkdir -p "$out"
cargo fetch --locked --quiet
cargo about generate --frozen --fail \
  --manifest-path crates/ui/Cargo.toml \
  --config crates/ui/packaging/about.toml \
  "${target_args[@]}" \
  --output-file "$out/THIRD-PARTY-LICENSES.html" \
  crates/ui/packaging/third-party-licenses.hbs
cp LICENSE "$out/LICENSE"
echo "Wrote $out/LICENSE and $out/THIRD-PARTY-LICENSES.html for ${targets[*]}."
