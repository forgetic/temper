#!/usr/bin/env sh
set -eu

# check-no-ambient-env.sh
#
# Hermeticity lint: ambient process-environment reads must live only at
# explicit composition boundaries, never scattered through library code.
#
# FAILS (exit 1) if any of the four ambient-env reads
#
#     std::env::var      std::env::var_os      std::env::vars      std::env::args
#
# appears in a tracked `.rs` file OUTSIDE the allowlist below. Exits 0 on a
# clean tree. On failure it prints `file:line: text` for every violation.
#
# Detection: the single pattern `std::env::(var|args)` matches all four needles,
# because `var` is a prefix of `var_os` and `vars`, and `args` is literal.
# Lines whose first non-whitespace is `//` (line/doc comments) are skipped, so
# allowlisted-style mentions in doc comments do not trip the lint.
#
# ---------------------------------------------------------------------------
# ALLOWLIST -- a path is allowed if ANY rule matches.
#
# Pattern rules (matched via `case` globs):
#
#   */src/bin/*        Binary composition roots. A `src/bin/*.rs` file IS the
#                      process entry point, so reading argv/env there is the
#                      whole point of the boundary.
#
#   test-support       Test code legitimately reads env to gate live/network
#                      tests and locate fixtures. Covered globs:
#                        */tests/*        (integration test trees)
#                        *_tests.rs       (inline unit-test modules)
#                        tests.rs         (a crate's tests module file)
#                        */tests/*.rs     (any .rs under a dir named `tests`)
#                        crates/temper-testing/*  (the reusable test-support
#                          crate: non-production machinery kept out of the
#                          default production dependency graph -- it builds the
#                          fixture worker/provision binaries that read env, the
#                          same spirit as `tests/`.)
#                        examples/*       (example programs read argv/env as
#                          their own entry points; not production code.)
#                      This mirrors how entry.rs's scan_dir skips `tests/`
#                      subtrees and `*_tests.rs` files.
#
# Named exact-path rules (matched against the literal path) -- keep this list
# easy to extend; add one path per line to AMBIENT_ENV_ALLOWLIST below:
#
#   crates/temper-config/src/env.rs
#       #198: the `#[doc(hidden)]` binary-only env shim
#       (EnvMap::from_system / SystemEnv / PathResolver::from_system). This is
#       the one sanctioned snapshot point for the real process environment.
#
#   crates/temper-agent-session/src/entry.rs
#       #201: the standalone agent's single real-env reader (process entry).
#
#   crates/temper-log/src/lib.rs
#       #208/#209: the UNRELATED logging workstream's crate. It reads
#       RUST_LOG / JOURNAL_STREAM at its own logging boundary -- not part of
#       this config epic, allowlisted at the logging crate's boundary.
#
#   Operator-tool binary-adjacent entry points -- genuine operator tools
#   reachable as `temper <subcmd>`, whose parse/run ARE their process entry
#   points and which expose a `parse_with_env` seam for tests:
#       crates/temper-interaction-service/src/interaction_repl.rs
#       crates/temper-interaction-service/src/interaction_serve.rs
#       crates/temper-provision-forgejo-cli/src/provision_args/parse.rs
#       crates/temper-reference-delivery-validator/src/reference_delivery_validator.rs
#
#   Responder process entry point that reads provider env (the analogue of the
#   agent `entry`):
#       crates/temper-cli/src/responders.rs
#
# NOTE: crates/temper-cli/src/lib.rs is deliberately NOT allowlisted -- its
# `std::env::args` read is being removed by refactor (a `boot()` env reader in
# src/bin/temper.rs), and it must become clean that way, not by allowlisting.
# ---------------------------------------------------------------------------

# Named exact-path allowlist. One repo-relative path per line; trailing
# comments (after `#`) and blank lines are ignored. Extend by adding a line.
AMBIENT_ENV_ALLOWLIST='
crates/temper-config/src/env.rs                                              # #198 env shim
crates/temper-agent-session/src/entry.rs                                     # #201 agent entry
crates/temper-log/src/lib.rs                                                 # logging crate boundary (RUST_LOG / TEMPER_LOG_FORMAT / JOURNAL_STREAM / NO_COLOR)
crates/temper-interaction-service/src/interaction_repl.rs                    # operator tool entry
crates/temper-interaction-service/src/interaction_serve.rs                   # operator tool entry
crates/temper-provision-forgejo-cli/src/provision_args/parse.rs             # operator tool entry
crates/temper-reference-delivery-validator/src/reference_delivery_validator.rs  # operator tool entry
crates/temper-cli/src/responders.rs                                          # responder provider-env entry
'

tmp_allowed="$(mktemp)"
tmp_violations="$(mktemp)"
trap 'rm -f "$tmp_allowed" "$tmp_violations"' EXIT

# Normalise the named allowlist into one bare path per line.
printf '%s\n' "$AMBIENT_ENV_ALLOWLIST" |
    sed -e 's/[[:space:]]*#.*$//' -e 's/[[:space:]]*$//' -e '/^[[:space:]]*$/d' >"$tmp_allowed"

# Pattern allowlist: returns 0 (allowed) if the path matches a glob rule.
is_pattern_allowed() {
    case "$1" in
        */src/bin/*) return 0 ;;   # crate binary composition roots
        src/bin/*)   return 0 ;;   # the root package's binary (src/bin/temper.rs)
        */tests/*)   return 0 ;;   # any .rs under a nested dir named tests
        tests/*)     return 0 ;;   # any .rs under a top-level dir named tests
        *_tests.rs)  return 0 ;;   # inline unit-test module files
        tests.rs|*/tests.rs) return 0 ;;  # a crate's tests module file
        crates/temper-testing/*) return 0 ;;  # reusable test-support crate
        examples/*)  return 0 ;;   # example programs (own entry points)
    esac
    return 1
}

git ls-files --cached --others --exclude-standard '*.rs' | sort -u |
while IFS= read -r path; do
    if [ ! -f "$path" ]; then
        continue
    fi
    if is_pattern_allowed "$path"; then
        continue
    fi
    if grep -Fxq "$path" "$tmp_allowed"; then
        continue
    fi

    # Match the four needles, skipping lines that are pure comments
    # (first non-whitespace is `//`).
    grep -nE 'std::env::(var|args)' "$path" |
        grep -vE '^[0-9]+:[[:space:]]*//' |
        while IFS= read -r hit; do
            printf '%s:%s\n' "$path" "$hit"
        done
done >"$tmp_violations"

if [ -s "$tmp_violations" ]; then
    echo "Ambient std::env reads outside the allowlist:" >&2
    cat "$tmp_violations" >&2
    echo >&2
    echo "Read process env only at an explicit boundary, or extend the" >&2
    echo "allowlist in scripts/check-no-ambient-env.sh with a justification." >&2
    exit 1
fi
