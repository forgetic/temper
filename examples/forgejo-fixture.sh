#!/bin/sh
# Shared Bench-owned Forgejo fixture resolution and startup verification for the
# delivery examples. This file is sourced by both launchers; keep versions and
# cache filenames in bench-forgejo rather than duplicating them here.

resolve_forgejo_fixture() {
    _fixture_workspace_root=$1
    _fixture_resolver="$_fixture_workspace_root/target/debug/temper-forgejo-fixture"

    log 'resolving checksum-verified Forgejo server and runner from the Bench fixture pins ...'
    ( cd "$_fixture_workspace_root" && cargo build -p temper-testing --bin temper-forgejo-fixture ) \
        || die 'cargo build -p temper-testing --bin temper-forgejo-fixture failed'
    [ -x "$_fixture_resolver" ] || die "Forgejo fixture resolver not found: $_fixture_resolver"

    _fixture_output=$(cd "$_fixture_workspace_root" && "$_fixture_resolver") \
        || die 'Bench Forgejo fixture resolution failed; inspect the preceding BENCH_FORGEJO_* diagnostic'
    FORGEJO_VERSION=$(printf '%s\n' "$_fixture_output" | sed -n 's/^forgejo_version=//p')
    FORGEJO_RUNNER_VERSION=$(printf '%s\n' "$_fixture_output" | sed -n 's/^forgejo_runner_version=//p')
    FORGEJO_BIN=$(printf '%s\n' "$_fixture_output" | sed -n 's/^forgejo=//p')
    RUNNER_BIN=$(printf '%s\n' "$_fixture_output" | sed -n 's/^forgejo_runner=//p')

    [ -n "$FORGEJO_VERSION" ] || die 'Bench fixture resolver returned no Forgejo version'
    [ -n "$FORGEJO_RUNNER_VERSION" ] || die 'Bench fixture resolver returned no runner version'
    [ -x "$FORGEJO_BIN" ] || die "resolved Forgejo binary is not executable: $FORGEJO_BIN"
    [ -x "$RUNNER_BIN" ] || die "resolved forgejo-runner binary is not executable: $RUNNER_BIN"
    log "resolved Bench fixture: Forgejo $FORGEJO_VERSION, forgejo-runner $FORGEJO_RUNNER_VERSION"
}

verify_forgejo_fixture_version() {
    _fixture_version_url="$BASE_URL/api/v1/version"
    _fixture_version_body=$(curl -fsS "$_fixture_version_url") \
        || die "Forgejo became reachable but $_fixture_version_url could not be read"
    _fixture_reported_version=$(printf '%s' "$_fixture_version_body" | python3 -c '
import json, sys
try:
    value = json.load(sys.stdin).get("version")
except (json.JSONDecodeError, AttributeError) as error:
    raise SystemExit(f"invalid JSON response: {error}")
if not isinstance(value, str) or not value:
    raise SystemExit("JSON response has no non-empty version field")
print(value)
') || die "Forgejo returned a malformed version response from $_fixture_version_url: $_fixture_version_body"

    case "$_fixture_reported_version" in
        "$FORGEJO_VERSION" | "$FORGEJO_VERSION"+*) ;;
        *)
            die "Forgejo fixture version mismatch: $_fixture_version_url reported $_fixture_reported_version, but Bench pins $FORGEJO_VERSION (resolved binary: $FORGEJO_BIN). Remove stale BENCH_FORGEJO_* overrides/cache entries or stage the pinned fixture again"
            ;;
    esac
    log "Forgejo API reports expected fixture release $_fixture_reported_version"
}
