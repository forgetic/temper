#!/usr/bin/env sh
set -eu

# Cargo and remote build caches can restore harness = false test artifacts
# without their Unix execute bit. These binaries still need to be queried by
# nextest, even though they do not use libtest's generated main function.
target_deps_dir="${CARGO_TARGET_DIR:-target}/debug/deps"

if [ -d "$target_deps_dir" ]; then
    find "$target_deps_dir" -maxdepth 1 -type f \
        \( -name 'linux_supervisor-*' -o -name 'windows_job-*' -o -name 'benchmark_harness-*' \) \
        ! -name '*.d' -exec chmod +x {} +
fi
