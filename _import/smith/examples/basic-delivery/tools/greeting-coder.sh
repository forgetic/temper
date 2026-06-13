#!/bin/sh
# Deterministic basic-delivery demo coding agent.
#
# smith-worker's coding executor invokes this command with the prepared checkout
# as the working directory and with file paths for the work-item context and
# WorkspaceResult supplied in its environment. The command must always write one
# WorkspaceResult JSON object to the supplied result path.
#
# When the context allows the architect triage verdict `ready_code`, this command
# acts as the read-only architect workspace and writes that routed verdict result.
# Otherwise it acts as the writable engineer workspace and leaves a meaningful,
# non-bookkeeping product diff in the working tree, then writes a summary result.
# smith-worker owns commit/push/PR result delivery, so this script never commits
# or pushes.
#
# This stand-in implements the seeded dead-simple intake (a configurable banner
# greeting) deterministically so the basic-delivery demo can run without a real
# coding agent for offline/CI smoke runs. Select it with BASIC_DELIVERY_CODER=greeting.
# The produced src/banner.sh also passes the bundled config/ci.yml (every *.sh in
# the tree must parse).
#
# POSIX sh only. It must be deterministic: re-running it on a fresh checkout of
# the same code issue must reproduce the same diff so CI re-runs are stable.
set -eu

json_escape_file() {
	awk '
		BEGIN { first = 1 }
		{
			line = $0
			gsub(/\\/, "\\\\", line)
			gsub(/"/, "\\\"", line)
			gsub(/\t/, "\\t", line)
			if (!first) {
				printf "\\n"
			}
			printf "%s", line
			first = 0
		}
	' "$1"
}

context=${TEMPER_CODING_WORKSPACE_CONTEXT:-}
if [ -n "$context" ] && [ -f "$context" ] && \
	grep '"allowed_verdicts"' "$context" >/dev/null 2>&1 && \
	grep '"ready_code"' "$context" >/dev/null 2>&1; then
	result=${TEMPER_CODING_WORKSPACE_RESULT:-}
	if [ -z "$result" ]; then
		printf '%s\n' 'error: architect mode requires TEMPER_CODING_WORKSPACE_RESULT' >&2
		exit 1
	fi

	body_file=${TMPDIR:-/tmp}/greeting-coder-body-$$.txt
	trap 'rm -f "$body_file"' EXIT HUP INT TERM
	cat >"$body_file" <<'EOF'
## Goal

Implement the seeded banner feature for the basic-delivery demo.

## Required behavior

Create an executable POSIX shell script at `src/banner.sh`.

The script must read the runtime configuration variable `BANNER_GREETING` and print its value followed by exactly one trailing newline. When `BANNER_GREETING` is unset or empty, it must default to:

`Hello from the basic-delivery demo`

## Implementation notes

- Create `src/` if it does not already exist.
- Keep `src/banner.sh` portable POSIX `sh`; do not use Bash-only syntax.
- The script should be deterministic and should not require network access or provider credentials.
- Ensure the file is executable.

## Validation

Run these checks from the repository root:

```sh
test -x src/banner.sh
sh -n src/banner.sh
src/banner.sh | grep -qx 'Hello from the basic-delivery demo'
BANNER_GREETING='Hello from prod' src/banner.sh | grep -qx 'Hello from prod'
```
EOF
	escaped_body=$(json_escape_file "$body_file")
	printf '{"verdict":"ready_code","body":"%s"}\n' "$escaped_body" >"$result"
	exit 0
fi

quote_single() {
	sed "s/'/'\\''/g; 1s/^/'/; \$s/\$/'/"
}

greeting=${BASIC_DELIVERY_GREETING:-Hello from the basic-delivery demo}
default_greeting=$(printf '%s' "$greeting" | quote_single)

mkdir -p src
cat >src/banner.sh <<EOF
#!/bin/sh
# Print the configurable service banner greeting on startup.
#
# Implements the basic-delivery intake: a BANNER_GREETING setting whose value is
# printed on startup, defaulting to the generated deterministic text when unset.
default_greeting=$default_greeting
: "\${BANNER_GREETING:=\$default_greeting}"
printf '%s\n' "\$BANNER_GREETING"
EOF
chmod +x src/banner.sh

result=${TEMPER_CODING_WORKSPACE_RESULT:-}
if [ -z "$result" ]; then
	printf '%s\n' 'error: engineer mode requires TEMPER_CODING_WORKSPACE_RESULT' >&2
	exit 1
fi
printf '{"summary":"Implement the configurable banner greeting"}\n' >"$result"
