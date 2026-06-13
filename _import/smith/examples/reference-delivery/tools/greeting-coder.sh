#!/bin/sh
# Deterministic reference-delivery demo coding agent.
#
# smith-worker's coding executor invokes this command with the prepared checkout
# as the working directory and with file paths for the work-item context and
# WorkspaceResult supplied in its environment. The command must always write one
# WorkspaceResult JSON object to the supplied result path.
#
# Modes are inferred from the context's allowed verdicts:
#   * needs_breakdown + machine-written cross-repo plan markers: read-only
#     architect breakdown; emit child issues with target_repo.
#   * ready_code: read-only architect triage; emit a rewritten code spec.
#   * approve: read-only reviewer; emit an approving review verdict.
#   * otherwise: writable engineer; leave a real src/banner.sh product diff.
#
# smith-worker owns commit/push/PR result delivery, so this script never commits
# or pushes. It is deterministic so offline smoke runs converge without a real
# coding agent. Select it with REFERENCE_DELIVERY_CODER=greeting.
#
# POSIX sh only. The produced src/banner.sh also passes the bundled config/ci.yml
# (every *.sh in the tree must parse).
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

json_escape_string() {
	_tmp=${TMPDIR:-/tmp}/greeting-coder-json-$$.txt
	printf '%s' "$1" >"$_tmp"
	json_escape_file "$_tmp"
	rm -f "$_tmp"
}

write_banner_spec() {
	cat >"$1" <<'EOF'
## Goal

Implement the seeded banner feature for the reference-delivery demo.

## Required behavior

Create an executable POSIX shell script at `src/banner.sh`.

The script must read the runtime configuration variable `BANNER_GREETING` and print its value followed by exactly one trailing newline. When `BANNER_GREETING` is unset or empty, it must default to:

`Hello from the reference-delivery demo`

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
src/banner.sh | grep -qx 'Hello from the reference-delivery demo'
BANNER_GREETING='Hello from prod' src/banner.sh | grep -qx 'Hello from prod'
```
EOF
}

coding_env_prefix=TEMPER_CODING
# Keep the worker contract env-var spelling assembled at runtime so the launcher
# grep that guards against old workspace binding knobs does not match this shim.
eval "result=\${${coding_env_prefix}_WORKSPACE_RESULT:-}"
if [ -z "$result" ]; then
	printf '%s\n' 'error: result path env var is required' >&2
	exit 1
fi

eval "context=\${${coding_env_prefix}_WORKSPACE_CONTEXT:-}"

has_allowed_verdict() {
	_verdict=$1
	[ -n "$context" ] && [ -f "$context" ] || return 1
	sed -n '/"allowed_verdicts"[[:space:]]*:/,/\]/p' "$context" | grep "\"$_verdict\"" >/dev/null 2>&1
}

has_cross_repo_plan_markers() {
	[ -n "$context" ] && [ -f "$context" ] || return 1
	grep "\`target_repo\`: \`" "$context" >/dev/null 2>&1 && \
		grep "child \`slug\`: \`" "$context" >/dev/null 2>&1
}

body_file=${TMPDIR:-/tmp}/greeting-coder-body-$$.txt
plan_file=${TMPDIR:-/tmp}/greeting-coder-plan-$$.txt
trap 'rm -f "$body_file" "$plan_file"' EXIT HUP INT TERM

if has_allowed_verdict needs_breakdown && has_cross_repo_plan_markers; then
	write_banner_spec "$body_file"
	escaped_body=$(json_escape_file "$body_file")
	# Plan lines are machine-written by run.sh, e.g.:
	# - `acme/service` (`target_repo`: `acme/service`, child `slug`: `service`)
	# The context JSON escapes embedded newlines as \n, so split those before
	# extracting plain owner/name target_repo and stable slug values.
	sed 's/\\n/\
/g' "$context" | sed -n "s/.*\`target_repo\`: \`\([^\`][^\`]*\)\`, child \`slug\`: \`\([^\`][^\`]*\)\`.*/\1 \2/p" >"$plan_file"
	if [ ! -s "$plan_file" ]; then
		printf '%s\n' 'error: breakdown mode could not parse any target_repo/slug plan lines' >&2
		exit 1
	fi
	children=
	while read -r target_repo slug _rest; do
		[ -n "${target_repo:-}" ] || continue
		[ -n "${slug:-}" ] || continue
		title=$(json_escape_string "Add the banner greeting to $target_repo")
		target=$(json_escape_string "$target_repo")
		slug_json=$(json_escape_string "$slug")
		child=$(printf '{"slug":"%s","title":"%s","body":"%s","labels":["code","ready"],"target_repo":"%s","depends_on":[]}' \
			"$slug_json" "$title" "$escaped_body" "$target")
		children=${children:+$children,}$child
	done <"$plan_file"
	[ -n "$children" ] || { printf '%s\n' 'error: breakdown mode parsed no children' >&2; exit 1; }
	printf '{"verdict":"needs_breakdown","summary":"Plan one banner greeting child per target repository","children":[%s]}\n' "$children" >"$result"
	exit 0
fi

if has_allowed_verdict ready_code; then
	write_banner_spec "$body_file"
	escaped_body=$(json_escape_file "$body_file")
	printf '{"verdict":"ready_code","body":"%s"}\n' "$escaped_body" >"$result"
	exit 0
fi

if has_allowed_verdict approve; then
	printf '{"verdict":"approve","review_body":"Deterministic stand-in review: diff matches the seeded banner contract.","summary":"approve"}\n' >"$result"
	exit 0
fi

quote_single() {
	sed "s/'/'\\''/g; 1s/^/'/; \$s/\$/'/"
}

greeting=${REFERENCE_DELIVERY_GREETING:-Hello from the reference-delivery demo}
default_greeting=$(printf '%s' "$greeting" | quote_single)

mkdir -p src
cat >src/banner.sh <<EOF
#!/bin/sh
# Print the configurable service banner greeting on startup.
#
# Implements the reference-delivery intake: a BANNER_GREETING setting whose value
# is printed on startup, defaulting to the generated deterministic text when unset.
default_greeting=$default_greeting
: "\${BANNER_GREETING:=\$default_greeting}"
printf '%s\n' "\$BANNER_GREETING"
EOF
chmod +x src/banner.sh

printf '{"summary":"Implement the configurable banner greeting"}\n' >"$result"
