#!/usr/bin/env bash
set -euo pipefail

context="${1:?script assertion context path is required}"
repo_root="$(git rev-parse --show-toplevel)"
head="$(git -C "$repo_root" rev-parse HEAD)"
landing_base="$(git -C "$repo_root" rev-parse HEAD^)"
validator="$repo_root/target/debug/temper-scenario"
stale_sha="0000000000000000000000000000000000000000"
stale_dir="$TEMPER_SCENARIO_ARTIFACT_DIR/stale-attempt"

# The successful outer run must already be bound to this checkout and a
# canonical mapping digest before probing the rejection path.
grep -Fq "\"checkout_head_sha\": \"$head\"" "$context"
grep -Eq '"resolved_content_digest": "sha256:[0-9a-f]{64}"' "$context"
test -x "$validator"
rm -rf -- "$stale_dir"
mkdir -p "$stale_dir"

set +e
(
  cd "$repo_root"
  "$validator" validate-feature \
    --feature ai/temper#778 \
    --landing-base "$landing_base" \
    --source-branch feature/778-exact-head-validation \
    --pr 1 \
    --sha "$stale_sha" \
    --output-dir "$stale_dir"
) >"$stale_dir/stdout.log" 2>"$stale_dir/stderr.log"
status=$?
set -e

if [ "$status" -eq 0 ]; then
  echo "stale exact-head probe unexpectedly passed" >&2
  exit 1
fi
grep -Fq 'evidence would be stale' "$stale_dir/focused-validation-failure.txt"
echo "current mapping is exact-head bound and a stale supplied head was rejected"
