#!/usr/bin/env sh
# Enforce one hard cap on Rust file size.
#
# Source files may have at most RUST_FILE_SIZE_MAX_LOC nonblank lines and test
# files at most RUST_TEST_FILE_SIZE_MAX_LOC. A file is a test file when it lives
# under a `tests/` directory or is named `tests.rs` or `*_tests.rs`.
#
# Files that legitimately exceed their cap (generated code, large fixtures,
# snapshot tests, bindings, data tables) are listed one path per line in the
# allowlist. An allowlist entry that no longer exists or no longer exceeds its
# cap fails the check, so the list prunes itself.
set -eu

max_loc="${RUST_FILE_SIZE_MAX_LOC:-800}"
test_max_loc="${RUST_TEST_FILE_SIZE_MAX_LOC:-1200}"
allowlist="${RUST_FILE_SIZE_ALLOWLIST:-scripts/rust-file-size-allowlist.txt}"

for pair in "RUST_FILE_SIZE_MAX_LOC=$max_loc" "RUST_TEST_FILE_SIZE_MAX_LOC=$test_max_loc"; do
    value="${pair#*=}"
    if ! expr "$value" : '[0-9][0-9]*$' >/dev/null || [ "$value" -eq 0 ]; then
        echo "${pair%%=*} must be a positive integer, got: $value" >&2
        exit 2
    fi
done

allowlist_entries=""
if [ -f "$allowlist" ]; then
    allowlist_entries="$(sed -e 's/[[:space:]]*#.*$//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$allowlist" | grep -v '^$' || true)"
fi

is_allowlisted() {
    printf '%s\n' "$allowlist_entries" | grep -qxF -- "$1"
}

cap_for() {
    case "$1" in
        tests/*|*/tests/*|tests.rs|*/tests.rs|*_tests.rs) echo "$test_max_loc" ;;
        *) echo "$max_loc" ;;
    esac
}

nonblank_loc() {
    awk 'NF { count++ } END { print count + 0 }' "$1"
}

violations=""
while IFS= read -r path; do
    if [ -z "$path" ] || [ ! -f "$path" ]; then
        continue
    fi
    cap="$(cap_for "$path")"
    loc="$(nonblank_loc "$path")"
    if [ "$loc" -gt "$cap" ] && ! is_allowlisted "$path"; then
        violations="${violations}${loc} > ${cap} ${path}
"
    fi
done <<PATHS
$(git ls-files --cached --others --exclude-standard '*.rs' | sort -u)
PATHS

stale=""
if [ -n "$allowlist_entries" ]; then
    while IFS= read -r path; do
        [ -z "$path" ] && continue
        if [ ! -f "$path" ]; then
            stale="${stale}missing ${path}
"
        elif [ "$(nonblank_loc "$path")" -le "$(cap_for "$path")" ]; then
            stale="${stale}under cap ${path}
"
        fi
    done <<ENTRIES
$allowlist_entries
ENTRIES
fi

status=0
if [ -n "$violations" ]; then
    echo "Rust files over their nonblank LOC cap (source ${max_loc}, tests ${test_max_loc}):" >&2
    printf '%s' "$violations" | sort -nr >&2
    echo >&2
    echo "Split the file along a domain boundary, or add its path to $allowlist if it is generated code, a fixture, a snapshot, a binding, or a data table." >&2
    status=1
fi

if [ -n "$stale" ]; then
    echo "Stale entries in $allowlist (remove them):" >&2
    printf '%s' "$stale" >&2
    status=1
fi

exit "$status"
