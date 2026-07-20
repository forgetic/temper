#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
cd "$repo_root"

temp_root=${TMPDIR:-/tmp}
mkdir -p "$temp_root"
metadata_dir=$(mktemp -d "$temp_root/temper-nextest.XXXXXX")
trap 'rm -rf "$metadata_dir"' EXIT HUP INT TERM
cargo_metadata="$metadata_dir/cargo-metadata.json"
binaries_metadata="$metadata_dir/binaries-metadata.json"

# A binaries-only list asks Cargo for nextest's exact test build without
# executing custom harnesses to enumerate their tests. Capture that build so
# the eventual run can reuse it without a second Cargo/cache restoration.
printf '\n==> cargo metadata --format-version 1\n'
cargo metadata --format-version 1 > "$cargo_metadata"
printf '\n==> cargo nextest list --workspace --list-type binaries-only --message-format json\n'
cargo nextest list --workspace --color never \
    --list-type binaries-only --message-format json > "$binaries_metadata"

printf '\n==> repair cached custom harness permissions\n'
scripts/repair-custom-harness-permissions.sh

# Supplying both metadata files makes nextest enumerate and execute precisely
# the captured binaries rather than invoking Cargo again after the repair.
printf '\n==> cargo nextest run (captured build)\n'
cargo nextest run \
    --cargo-metadata "$cargo_metadata" \
    --binaries-metadata "$binaries_metadata" \
    --color never --show-progress none --status-level leak \
    "$@"
