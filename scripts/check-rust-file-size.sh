#!/usr/bin/env sh
set -eu

hard_max_loc="${RUST_FILE_SIZE_HARD_MAX_LOC:-800}"
justification_loc="${RUST_FILE_SIZE_JUSTIFICATION_LOC:-600}"
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

path_list_contains() {
    list_file="$1"
    needle="$2"

    if [ ! -f "$list_file" ]; then
        return 1
    fi

    awk -v needle="$needle" '
        {
            sub(/[[:space:]]*#.*$/, "")
            sub(/^[[:space:]]+/, "")
            sub(/[[:space:]]+$/, "")
            if ($0 == needle) {
                found = 1
            }
        }
        END { exit found ? 0 : 1 }
    ' "$list_file"
}

violations=""
missing_justification=""
paths="$(git ls-files --cached --others --exclude-standard '*.rs' | sort -u)"

while IFS= read -r path; do
    if [ -z "$path" ] || [ ! -f "$path" ]; then
        continue
    fi

    loc="$(awk 'NF { count++ } END { print count + 0 }' "$path")"
    if [ "$loc" -gt "$hard_max_loc" ] && ! path_list_contains "$hard_allowlist" "$path"; then
        violations="${violations}${loc} ${path}
"
    fi
    if [ "$loc" -gt "$justification_loc" ] && ! path_list_contains "$justifications" "$path"; then
        missing_justification="${missing_justification}${loc} ${path}
"
    fi
done <<EOF
$paths
EOF

if [ -n "$violations" ]; then
    echo "Rust files over ${hard_max_loc} nonblank LOC:" >&2
    printf '%s' "$violations" | sort -nr >&2
    echo >&2
    echo "Split the file or add an explicit hard-rule exception to $hard_allowlist." >&2
    exit 1
fi

if [ -n "$missing_justification" ]; then
    echo "Rust files over ${justification_loc} nonblank LOC without justification:" >&2
    printf '%s' "$missing_justification" | sort -nr >&2
    echo >&2
    echo "Split the file or add a short justification near its path in $justifications." >&2
    exit 1
fi
