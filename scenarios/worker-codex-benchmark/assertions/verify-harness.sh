#!/usr/bin/env bash
set -euo pipefail
exec python3 -I -B "$(dirname "$0")/verify_harness.py" "${1:?assertion context is required}"
