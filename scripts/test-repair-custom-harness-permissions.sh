#!/usr/bin/env sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
temp_root=${TMPDIR:-/tmp}
mkdir -p "$temp_root"
fixture_root=$(mktemp -d "$temp_root/temper-harness-modes.XXXXXX")
trap 'rm -rf "$fixture_root"' EXIT HUP INT TERM
fixture_deps="$fixture_root/target with spaces/debug/deps"
mkdir -p "$fixture_deps"

cached_harnesses='benchmark_harness-cached
linux_supervisor-cached
windows_job-cached'
for artifact in $cached_harnesses; do
    : > "$fixture_deps/$artifact"
    chmod 0644 "$fixture_deps/$artifact"
done

# Matching dependency metadata and unrelated cached artifacts must not be made
# executable by the narrowly-scoped repair.
: > "$fixture_deps/linux_supervisor-cached.d"
: > "$fixture_deps/ordinary_test-cached"
chmod 0644 "$fixture_deps/linux_supervisor-cached.d" "$fixture_deps/ordinary_test-cached"

(
    cd "$repo_root"
    CARGO_TARGET_DIR="$fixture_root/target with spaces" \
        scripts/repair-custom-harness-permissions.sh
)

for artifact in $cached_harnesses; do
    if [ ! -x "$fixture_deps/$artifact" ]; then
        echo "custom harness was not made executable: $artifact" >&2
        exit 1
    fi
done

for artifact in linux_supervisor-cached.d ordinary_test-cached; do
    if [ -x "$fixture_deps/$artifact" ]; then
        echo "non-harness artifact was unexpectedly made executable: $artifact" >&2
        exit 1
    fi
done

printf 'cached 0644 custom harness permission repair passed\n'
