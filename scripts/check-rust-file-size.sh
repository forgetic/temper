#!/usr/bin/env sh
set -eu

hard_max_loc="${RUST_FILE_SIZE_HARD_MAX_LOC:-600}"
justification_loc="${RUST_FILE_SIZE_JUSTIFICATION_LOC:-500}"
hard_allowlist="${RUST_FILE_SIZE_ALLOWLIST:-scripts/rust-file-size-allowlist.txt}"
justifications="${RUST_FILE_SIZE_JUSTIFICATIONS:-scripts/rust-file-size-justifications.txt}"

if ! expr "$hard_max_loc" : '[0-9][0-9]*$' >/dev/null; then
    echo "RUST_FILE_SIZE_HARD_MAX_LOC must be a positive integer, got: $hard_max_loc" >&2
    exit 2
fi

if ! expr "$justification_loc" : '[0-9][0-9]*$' >/dev/null; then
    echo "RUST_FILE_SIZE_JUSTIFICATION_LOC must be a positive integer, got: $justification_loc" >&2
    exit 2
fi

tmp_allowed="$(mktemp)"
tmp_justified="$(mktemp)"
tmp_missing_justification="$(mktemp)"
tmp_violations="$(mktemp)"
trap 'rm -f "$tmp_allowed" "$tmp_justified" "$tmp_missing_justification" "$tmp_violations"' EXIT

if [ -f "$hard_allowlist" ]; then
    sed -e 's/[[:space:]]*#.*$//' -e '/^[[:space:]]*$/d' "$hard_allowlist" >"$tmp_allowed"
else
    : >"$tmp_allowed"
fi

if [ -f "$justifications" ]; then
    sed -e 's/[[:space:]]*#.*$//' -e '/^[[:space:]]*$/d' "$justifications" >"$tmp_justified"
else
    : >"$tmp_justified"
fi

git ls-files --cached --others --exclude-standard '*.rs' | sort -u |
while IFS= read -r path; do
    if [ ! -f "$path" ]; then
        continue
    fi

    loc="$(awk 'NF { count++ } END { print count + 0 }' "$path")"
    if [ "$loc" -gt "$hard_max_loc" ] && ! grep -Fxq "$path" "$tmp_allowed"; then
        printf '%s %s\n' "$loc" "$path"
    fi
    if [ "$loc" -gt "$justification_loc" ] && ! grep -Fxq "$path" "$tmp_justified"; then
        printf '%s %s\n' "$loc" "$path" >>"$tmp_missing_justification"
    fi
done >"$tmp_violations"

if [ -s "$tmp_violations" ]; then
    echo "Rust files over ${hard_max_loc} nonblank LOC:" >&2
    sort -nr "$tmp_violations" >&2
    echo >&2
    echo "Split the file or add an explicit hard-rule exception to $hard_allowlist." >&2
    exit 1
fi

if [ -s "$tmp_missing_justification" ]; then
    echo "Rust files over ${justification_loc} nonblank LOC without justification:" >&2
    sort -nr "$tmp_missing_justification" >&2
    echo >&2
    echo "Split the file or add a short justification near its path in $justifications." >&2
    exit 1
fi
