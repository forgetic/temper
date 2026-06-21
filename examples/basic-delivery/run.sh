#!/bin/sh
# basic-delivery example — POSIX launcher / teardown.
#
# The minimal, no-human-in-the-loop counterpart to reference-delivery: ONE repo,
# TWO human-capable workflow roles (architect + engineer) plus a mechanical bot,
# CI, webhooks on, and landing gated on CI alone. It boots a local development
# topology as a SINGLE process:
#   1. a throwaway Forgejo server (SQLite, Actions enabled),
#   2. a host-mode forgejo-runner producing real CI,
#   3. a local jig fake LLM provider loaded with fixtures/basic-delivery.json,
#   4. admin bootstrap, then `temper init --non-interactive` to create the empty
#      managed repo, labels, webhook, config.toml, workflow.json,
#      credentials.toml, and webhook-secret,
#   5. an explicit initial repo commit (README + .forgejo/workflows/ci.yml),
#   6. `temper serve standalone`: the unified daemon + worker + coding agent on
#      ONE event loop, using the init-emitted config and credentials,
#   7. only once serve-standalone is ready, a direct Forgejo REST API call files
#      ONE unlabeled intake issue authored by the SITE ADMIN, so the
#      issue-created webhook is the demonstrated wake path.
# The jig-backed coding agent lets the architect triage the intake to a ready
# code issue and the engineer open a real implementation PR; CI runs, goes green,
# and the mechanical backstop auto-merges — no reviewer, owner, or human. It
# tears everything down cleanly on Ctrl-C / signal / `./run.sh stop`.
#
# This script targets the operator-facing `temper` entry point. By default it
# builds/uses the development binary under target/debug; override TEMPER_RUN_BIN
# for a prebuilt or release artifact.
#
# Usage:
#   ./run.sh [start]          boot everything and block until Ctrl-C / stop-file
#   ./run.sh validate-webhooks inspect logs from a running/completed run
#   ./run.sh stop             tear down a previous run via the saved PIDs
#   ./run.sh help             show this usage
#
# Orphan cleanup (lesson 0009) — if a run is force-killed (SIGKILL) the Drop/
# trap guards do not fire; clean up survivors by hand with:
#       pkill -f forgejo
#       pkill -f forgejo-runner
#       pkill -f 'target/debug/temper'
#       pkill -f 'target/debug/jig'
#       rm -rf examples/basic-delivery/run
#
# POSIX sh only (no bashisms). Validate with `sh -n run.sh` (and shellcheck).
# Secrets travel by env or generated files, NEVER on a command line.

set -eu

# --- Locations ----------------------------------------------------------------
if [ -n "${TEMPER_BASIC_DELIVERY_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$TEMPER_BASIC_DELIVERY_SCRIPT_DIR
else
    SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fi
WORKSPACE_ROOT=${TEMPER_WORKSPACE_ROOT:-$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)}
CONFIG_DIR="$SCRIPT_DIR/config"
SECRETS_DIR="$SCRIPT_DIR/secrets"
RUN_DIR="$SCRIPT_DIR/run"
LOG_DIR="$SCRIPT_DIR/logs"

FORGEJO_DATA="$RUN_DIR/forgejo"
APP_INI="$FORGEJO_DATA/custom/conf/app.ini"
RUNNER_DIR="$RUN_DIR/runner"
STOP_FILE="$RUN_DIR/stop"
SERVER_PID_FILE="$RUN_DIR/server.pid"
RUNNER_PID_FILE="$RUN_DIR/runner.pid"
RUN_PID_FILE="$RUN_DIR/run.pid"
JIG_PID_FILE="$RUN_DIR/jig.pid"
JIG_STDIN="$RUN_DIR/jig.stdin"

# `temper init` emits the live deployment artifacts into the run directory. The
# launcher consumes these exact files for `temper serve standalone`; it does not
# synthesize an equivalent runtime config by hand.
CONFIG_FILE="$RUN_DIR/config.toml"
CREDENTIALS_FILE="$RUN_DIR/credentials.toml"
INIT_WORKFLOW_PATH="$RUN_DIR/workflow.json"
WEBHOOK_SECRET_FILE="$RUN_DIR/webhook-secret"

# Pinned versions for the bundled throwaway server/runner used by this example.
FORGEJO_VERSION=7.0.12
FORGEJO_RUNNER_VERSION=3.5.1

# Throwaway admin identity. This is also the workflow's intake_author
# (site_admin): the bundled workflow.json declares intake_author = site_admin, so
# run.sh files the intake issue as THIS admin (the "external filer").
# The server is killed + wiped on teardown; this is never a credential that
# reaches anything real, and never echoed.
ADMIN_USER=basicadmin
ADMIN_EMAIL=basicadmin@example.invalid
ADMIN_PASSWORD='Basic-Delivery-Admin-1!'

# Diagnostic strings emitted when Forgejo 7.0.x Actions status cannot be read by
# the ADR-0019 web-UI fallback. `temper serve standalone` hosts the mechanical CI-read path.
CI_FALLBACK_MISSING_CREDENTIALS='no web-UI credentials configured for the CI read fallback'
CI_FALLBACK_LOGIN_FAILED='forgejo web-ui login failed'

log() { printf '[run.sh] %s\n' "$*"; }
die() { printf '[run.sh] error: %s\n' "$*" >&2; exit 1; }

sleep_short() {
    sleep 0.2 2>/dev/null || sleep 1
}

DISPLAY_SCRIPT=${TEMPER_BASIC_DELIVERY_ORIGINAL:-$SCRIPT_DIR/run.sh}

# Dash reads long-running scripts lazily. If this file is edited while the demo
# is sleeping in monitor(), the running shell may parse a half-new tail and fail
# during teardown. Run starts from a private snapshot so source edits/rebuilds do
# not affect the already-running launcher.
if [ "${TEMPER_BASIC_DELIVERY_SNAPSHOT:-0}" != "1" ]; then
    case "${1:-start}" in
        start | "")
            mkdir -p "$RUN_DIR"
            _snapshot="$RUN_DIR/run.sh.snapshot.$$"
            cp "$SCRIPT_DIR/run.sh" "$_snapshot"
            chmod 700 "$_snapshot"
            TEMPER_BASIC_DELIVERY_SNAPSHOT=1 \
            TEMPER_BASIC_DELIVERY_SCRIPT_DIR="$SCRIPT_DIR" \
            TEMPER_BASIC_DELIVERY_ORIGINAL="$DISPLAY_SCRIPT" \
                exec /bin/sh "$_snapshot" "$@"
            ;;
    esac
fi

usage() {
    cat <<EOF
usage: $DISPLAY_SCRIPT [start|validate-webhooks|stop|help]

  start (default)      boot Forgejo + runner + local jig, run
                       \`temper init --non-interactive\` against the empty repo,
                       commit the demo README + CI workflow explicitly, launch
                       \`temper serve standalone\` (daemon + worker + jig-backed
                       coding agent), then file one site-admin intake issue so
                       its webhook wakes the run, and block until Ctrl-C or the
                       stop-file.
  validate-webhooks    inspect logs/ and report whether the webhook was
                       registered, accepted, scanned, assigned, and completed.
  stop                 tear down a previous run via run/*.pid.
  help                 show this message.

Configuration is read from config/temper.env (no secrets). The LLM provider is a
local jig server by default; jig ignores the dummy provider key stored by init.
EOF
}

# --- Teardown -----------------------------------------------------------------

# Sends TERM, waits briefly, then KILL. Tolerates a dead/absent pid.
stop_pid() {
    _pid=$1
    [ -n "$_pid" ] || return 0
    kill -0 "$_pid" 2>/dev/null || return 0
    kill -TERM "$_pid" 2>/dev/null || true
    _i=0
    while kill -0 "$_pid" 2>/dev/null && [ "$_i" -lt 20 ]; do
        sleep 0.2 2>/dev/null || sleep 1
        _i=$((_i + 1))
    done
    kill -KILL "$_pid" 2>/dev/null || true
}

# Stops every pid listed (one per line) in a pid file, then removes it.
stop_pid_file() {
    _file=$1
    [ -f "$_file" ] || return 0
    while IFS= read -r _p; do
        [ -n "$_p" ] && stop_pid "$_p"
    done <"$_file"
    rm -f "$_file"
}

# Tears down the run process, runner, and server (in that order) and clears run
# state. Idempotent: safe to call from the EXIT trap and from `./run.sh stop`.
cleanup() {
    trap - EXIT INT TERM
    log 'tearing down...'
    [ -d "$RUN_DIR" ] && : >"$STOP_FILE" 2>/dev/null || true
    sleep 1
    stop_pid_file "$RUN_PID_FILE"
    stop_pid_file "$JIG_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    stop_pid_file "$SERVER_PID_FILE"
    # Drop throwaway server/runner data + runtime checkouts + init-emitted
    # secrets/config + sentinel so a re-run starts fresh; keep logs/ for
    # inspection.
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$STOP_FILE" \
        "$RUN_DIR/repo-seed" "$RUN_DIR/workspaces" \
        "$CONFIG_FILE" "$CREDENTIALS_FILE" "$INIT_WORKFLOW_PATH" \
        "$WEBHOOK_SECRET_FILE" "$JIG_STDIN" "$RUN_DIR"/run.sh.snapshot.* \
        2>/dev/null || true
    rmdir "$RUN_DIR" 2>/dev/null || true
    log 'teardown complete'
}

cmd_stop() {
    [ -d "$RUN_DIR" ] || { log 'nothing to stop (no run/ dir)'; return 0; }
    cleanup
}

# --- Config + secrets ---------------------------------------------------------

# Config knobs whose pre-existing environment value should win over the file
# (precedence: CLI/env > config/temper.env > built-in default). The file is the
# operator's edited config; a `VAR=x ./run.sh` still overrides it.
CONFIG_KNOBS="OWNER NAME DEFAULT_BRANCH INTAKE_TITLE INTAKE_BODY_FILE BASE_URL DAEMON_BIND \
DAEMON_POLL_CADENCE_SECS DAEMON_MECHANICAL_CADENCE_SECS DAEMON_LEASE_TTL_SECS RUN_SECS \
TEMPER_FORGEJO_GOMAXPROCS TEMPER_FORGEJO_BINARY TEMPER_FORGEJO_RUNNER_BINARY \
TEMPER_RUN_BIN TEMPER_BUILD_PACKAGE \
JIG_REPO JIG_BIN JIG_FIXTURE TEMPER_SKIP_JIG_BUILD INIT_PROVIDER_KEY RUN_MAX_ITERATIONS"

repo_owner() { printf '%s\n' "${1%%/*}"; }
repo_name() { printf '%s\n' "${1#*/}"; }

validate_repo_path() {
    _repo=$1
    case "$_repo" in
        */*) ;;
        *) die "repository must be owner/name, got '$_repo'" ;;
    esac
    _owner=$(repo_owner "$_repo")
    _name=$(repo_name "$_repo")
    [ -n "$_owner" ] && [ -n "$_name" ] && [ "$_owner/$_name" = "$_repo" ] \
        || die "repository must be owner/name with non-empty parts, got '$_repo'"
}

require_positive_int() {
    _name=$1
    _value=$2
    case "$_value" in
        '' | *[!0-9]* | 0) die "$_name must be a positive integer, got '$_value'" ;;
    esac
}

load_config() {
    [ -f "$CONFIG_DIR/temper.env" ] || die "missing $CONFIG_DIR/temper.env"
    # Snapshot any pre-existing env values so they survive the file sourcing.
    for _k in $CONFIG_KNOBS; do
        eval "_pre_$_k=\${$_k:-}"
    done
    # shellcheck disable=SC1090
    . "$CONFIG_DIR/temper.env"
    # Re-apply any non-empty pre-existing env value over the file's setting.
    for _k in $CONFIG_KNOBS; do
        eval "_p=\${_pre_$_k}"
        [ -n "$_p" ] && eval "$_k=\$_p"
    done

    OWNER=${OWNER:-acme}
    NAME=${NAME:-service}
    DEFAULT_BRANCH=${DEFAULT_BRANCH:-main}
    # The seeded intake issue is deliberately THIN: the site admin (external
    # filer) states only the overall intent, with no acceptance criteria, no
    # setting name, and no implementation detail. Turning that into an
    # implementable spec is the architect's job (the triage_intake_to_code
    # set_body rewrite) — that is what this example proves the architect can do.
    INTAKE_TITLE=${INTAKE_TITLE:-Service banner should identify the environment}
    INTAKE_BODY_FILE=${INTAKE_BODY_FILE:-intake-issue.md}
    BASE_URL=${BASE_URL:-http://127.0.0.1:4100}
    DAEMON_BIND=${DAEMON_BIND:-127.0.0.1:38100}
    WEBHOOK_URL=http://$DAEMON_BIND/forgejo/webhook
    DAEMON_POLL_CADENCE_SECS=${DAEMON_POLL_CADENCE_SECS:-120}
    DAEMON_MECHANICAL_CADENCE_SECS=${DAEMON_MECHANICAL_CADENCE_SECS:-2}
    DAEMON_LEASE_TTL_SECS=${DAEMON_LEASE_TTL_SECS:-300}
    RUN_SECS=${RUN_SECS:-600}
    TEMPER_FORGEJO_GOMAXPROCS=${TEMPER_FORGEJO_GOMAXPROCS:-2}
    TEMPER_FORGEJO_BINARY=${TEMPER_FORGEJO_BINARY:-}
    TEMPER_FORGEJO_RUNNER_BINARY=${TEMPER_FORGEJO_RUNNER_BINARY:-}
    TEMPER_RUN_BIN=${TEMPER_RUN_BIN:-}
    TEMPER_BUILD_PACKAGE=${TEMPER_BUILD_PACKAGE:-temper}
    JIG_REPO=${JIG_REPO:-$HOME/src/rust/jig}
    JIG_BIN=${JIG_BIN:-}
    JIG_FIXTURE=${JIG_FIXTURE:-fixtures/basic-delivery.json}
    TEMPER_SKIP_JIG_BUILD=${TEMPER_SKIP_JIG_BUILD:-0}
    INIT_PROVIDER_KEY=${INIT_PROVIDER_KEY:-basic-delivery-jig-dummy-key}
    RUN_MAX_ITERATIONS=${RUN_MAX_ITERATIONS:-250}

    require_positive_int DAEMON_POLL_CADENCE_SECS "$DAEMON_POLL_CADENCE_SECS"
    require_positive_int DAEMON_MECHANICAL_CADENCE_SECS "$DAEMON_MECHANICAL_CADENCE_SECS"
    require_positive_int DAEMON_LEASE_TTL_SECS "$DAEMON_LEASE_TTL_SECS"
    require_positive_int RUN_SECS "$RUN_SECS"
    require_positive_int RUN_MAX_ITERATIONS "$RUN_MAX_ITERATIONS"

    # Single repo only: this example is deliberately one converging happy path.
    REPO="$OWNER/$NAME"
    validate_repo_path "$REPO"

    # Resolve the thin intake body: a relative path is taken relative to config/,
    # an absolute path is used verbatim.
    case "$INTAKE_BODY_FILE" in
        /*) INTAKE_BODY_PATH="$INTAKE_BODY_FILE" ;;
        *)  INTAKE_BODY_PATH="$CONFIG_DIR/$INTAKE_BODY_FILE" ;;
    esac
    [ -f "$INTAKE_BODY_PATH" ] || die "intake body file not found: $INTAKE_BODY_PATH (set INTAKE_BODY_FILE in config/temper.env)"

    # Resolve the jig fixture the same way: relative paths are anchored at the
    # jig checkout so the default is portable across cwd changes.
    case "$JIG_FIXTURE" in
        /*) JIG_FIXTURE_PATH="$JIG_FIXTURE" ;;
        *)  JIG_FIXTURE_PATH="$JIG_REPO/$JIG_FIXTURE" ;;
    esac

    # Cap the Go runtime of the spawned forgejo + forgejo-runner (lesson 0009).
    # Exported so both Go processes inherit it; harmless for Rust processes.
    if [ -n "$TEMPER_FORGEJO_GOMAXPROCS" ]; then
        export GOMAXPROCS="$TEMPER_FORGEJO_GOMAXPROCS"
    fi

    # Derive host/port from BASE_URL (http://host:port).
    _hostport=${BASE_URL#*://}
    _hostport=${_hostport%%/*}
    HOST=${_hostport%%:*}
    case "$_hostport" in
        *:*) PORT=${_hostport##*:} ;;
        *)   PORT=3000 ;;
    esac
}

# --- Binaries -----------------------------------------------------------------

resolve_binaries() {
    # One unified binary provides everything this example needs: `temper init`
    # and `temper serve standalone`.
    RUN_BIN=${TEMPER_RUN_BIN:-$WORKSPACE_ROOT/target/debug/temper}

    command -v curl >/dev/null 2>&1 \
        || die 'curl is required to probe Forgejo readiness'
    command -v python3 >/dev/null 2>&1 \
        || die 'python3 is required for Forgejo issue API JSON construction, git credential URL encoding, and config patching'
    command -v git >/dev/null 2>&1 \
        || die 'git is required to create the explicit initial repository commit'
    command -v mkfifo >/dev/null 2>&1 \
        || die 'mkfifo is required to keep the local jig process stdin open'

    # Keep the demo entry point self-healing after source changes. Cargo is a
    # cheap no-op when the development binaries are already current; skipping
    # this is an explicit operator choice for prebuilt/current binaries.
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring the Temper development binary is current (cargo build -p $TEMPER_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$TEMPER_BUILD_PACKAGE" ) \
            || die 'Temper cargo build failed'
    fi

    [ -x "$RUN_BIN" ] || die "temper binary not found: $RUN_BIN"

    # This example requires the local-dev init path and the serve-standalone UX.
    # Refuse to run against a stale development binary.
    _init_help=$("$RUN_BIN" init --help 2>&1 || true)
    case "$_init_help" in
        *--non-interactive*) ;;
        *) die "temper binary is stale or incompatible: $RUN_BIN 'init' does not advertise --non-interactive. Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac
    case "$_init_help" in
        *--provider-url*) ;;
        *) die "temper binary is stale or incompatible: $RUN_BIN 'init' does not advertise --provider-url. Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac
    _serve_help=$("$RUN_BIN" serve standalone --help 2>&1 || true)
    case "$_serve_help" in
        *--config*) ;;
        *) die "temper binary is stale or incompatible: $RUN_BIN 'serve standalone' does not advertise --config. Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac
    case "$_serve_help" in
        *--credentials*) ;;
        *) die "temper binary is stale or incompatible: $RUN_BIN 'serve standalone' does not advertise --credentials. Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac

    # Pinned Forgejo + runner: env override, else the cached pinned path.
    FORGEJO_BIN=${TEMPER_FORGEJO_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-$FORGEJO_VERSION-linux-amd64}
    RUNNER_BIN=${TEMPER_FORGEJO_RUNNER_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-runner-$FORGEJO_RUNNER_VERSION-linux-amd64}
    [ -x "$FORGEJO_BIN" ] || die "forgejo binary not found: $FORGEJO_BIN
       Set TEMPER_FORGEJO_BINARY, or pre-stage the pinned binary in .cache/forgejo/
       with: cargo test -p temper-forgejo-fixture --test cache -- --ignored"
    [ -x "$RUNNER_BIN" ] || die "forgejo-runner binary not found: $RUNNER_BIN
       Set TEMPER_FORGEJO_RUNNER_BINARY, or pre-stage the pinned binary in .cache/forgejo/
       with: cargo test -p temper-forgejo-fixture --test cache -- --ignored"

    # Local jig fake LLM provider.
    [ -d "$JIG_REPO" ] || die "jig checkout not found: $JIG_REPO (set JIG_REPO in config/temper.env or the environment)"
    [ -f "$JIG_REPO/Cargo.toml" ] || die "jig checkout lacks Cargo.toml: $JIG_REPO"
    if [ -z "$JIG_BIN" ]; then
        JIG_BIN="$JIG_REPO/target/debug/jig"
        if [ "$TEMPER_SKIP_JIG_BUILD" != "1" ]; then
            log "ensuring the jig development binary is current (cargo build -p jig in $JIG_REPO)..."
            ( cd "$JIG_REPO" && cargo build -p jig ) || die 'jig cargo build failed'
        fi
    fi
    [ -x "$JIG_BIN" ] || die "jig binary not found: $JIG_BIN (set JIG_BIN or build $JIG_REPO)"
    [ -f "$JIG_FIXTURE_PATH" ] || die "jig fixture not found: $JIG_FIXTURE_PATH (set JIG_FIXTURE)"

    log "coding agent: in-process (temper serve standalone; provider=deepseek via jig fixture $JIG_FIXTURE_PATH)"
}

# --- Forgejo server -----------------------------------------------------------

write_app_ini() {
    mkdir -p "$FORGEJO_DATA/custom/conf" "$FORGEJO_DATA/data" \
        "$FORGEJO_DATA/log" "$FORGEJO_DATA/repos"
    cat >"$APP_INI" <<EOF
APP_NAME = Basic Delivery Example
RUN_MODE = prod
WORK_PATH = $FORGEJO_DATA

[server]
PROTOCOL = http
HTTP_ADDR = $HOST
HTTP_PORT = $PORT
ROOT_URL = $BASE_URL/
DISABLE_SSH = true
START_SSH_SERVER = false
OFFLINE_MODE = true
APP_DATA_PATH = $FORGEJO_DATA/data

[database]
DB_TYPE = sqlite3
PATH = $FORGEJO_DATA/data/forgejo.db
LOG_SQL = false

[repository]
ROOT = $FORGEJO_DATA/repos

[log]
ROOT_PATH = $FORGEJO_DATA/log
MODE = console
LEVEL = error

[security]
INSTALL_LOCK = true
SECRET_KEY = basic-delivery-example-not-for-production
INTERNAL_TOKEN = basic-delivery-example-internal-not-for-production

[service]
DISABLE_REGISTRATION = true
REQUIRE_SIGNIN_VIEW = false

[mailer]
ENABLED = false

[webhook]
ALLOWED_HOST_LIST = 127.0.0.1,localhost

[actions]
ENABLED = true
EOF
}

# Runs a `forgejo` admin/CLI subcommand against the instance config.
forgejo_cli() {
    GITEA_WORK_DIR="$FORGEJO_DATA" "$FORGEJO_BIN" --config "$APP_INI" "$@"
}

boot_server() {
    log "booting Forgejo at $BASE_URL ..."
    if curl -fsS "$BASE_URL/api/v1/version" >/dev/null 2>&1; then
        die "Forgejo already responds at $BASE_URL before this run started. Stop the existing run first, or clean up orphaned forgejo processes."
    fi
    write_app_ini
    forgejo_cli migrate >"$LOG_DIR/forgejo-migrate.log" 2>&1 \
        || die "forgejo migrate failed (see logs/forgejo-migrate.log)"

    GITEA_WORK_DIR="$FORGEJO_DATA" "$FORGEJO_BIN" --config "$APP_INI" web \
        >"$LOG_DIR/forgejo.log" 2>&1 &
    SERVER_PID=$!
    echo "$SERVER_PID" >"$SERVER_PID_FILE"

    _i=0
    until curl -fsS "$BASE_URL/api/v1/version" >/dev/null 2>&1; do
        kill -0 "$SERVER_PID" 2>/dev/null \
            || die "forgejo exited during startup (see logs/forgejo.log)"
        _i=$((_i + 1))
        [ "$_i" -gt 300 ] && die "forgejo did not become ready (see logs/forgejo.log)"
        sleep 0.2 2>/dev/null || sleep 1
    done
    log "Forgejo ready (pid $SERVER_PID)"
}

ensure_secret_file() {
    _file=$1
    [ -f "$_file" ] && return 0
    umask 077
    mkdir -p "$(dirname -- "$_file")"
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 32 >"$_file"
    else
        dd if=/dev/urandom bs=32 count=1 2>/dev/null | od -An -tx1 | tr -d ' \n' >"$_file"
        printf '\n' >>"$_file"
    fi
}

boot_runner() {
    log 'registering host-mode forgejo-runner ...'
    mkdir -p "$RUNNER_DIR"
    _reg_token=$(forgejo_cli actions generate-runner-token | tr -d '[:space:]')
    [ -n "$_reg_token" ] || die 'failed to mint a runner registration token'
    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" register --no-interactive \
        --instance "$BASE_URL" --token "$_reg_token" \
        --name "basic-delivery-$$" --labels host:host ) \
        >"$LOG_DIR/runner-register.log" 2>&1 \
        || die 'forgejo-runner register failed (see logs/runner-register.log)'

    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" daemon ) >"$LOG_DIR/runner.log" 2>&1 &
    RUNNER_PID=$!
    echo "$RUNNER_PID" >"$RUNNER_PID_FILE"
    log "runner daemon running (pid $RUNNER_PID)"
}

wait_for_log_line() {
    _file=$1
    _needle=$2
    _pid=$3
    _label=$4
    _i=0
    while ! grep -q "$_needle" "$_file" 2>/dev/null; do
        kill -0 "$_pid" 2>/dev/null || die "$_label exited before readiness (see $_file)"
        _i=$((_i + 1))
        [ "$_i" -gt 100 ] && die "$_label did not become ready (see $_file)"
        sleep_short
    done
}

# --- Jig + init + repo population + seed --------------------------------------

repo_slug() {
    repo_name "$1" | tr -c '[:alnum:]' '-' | tr '[:upper:]' '[:lower:]' | sed 's/^-*//;s/-*$//'
}

# Reads one `[forge.users.<key>]` field from the credentials.toml init wrote.
# `$1` is the user/role key, `$2` the field name (`user`/`token`/`password`/
# `email`). Prints the unquoted value, or nothing if the section/field is absent
# (e.g. `user` is omitted when it equals the key). POSIX awk only; values never
# contain embedded quotes in practice.
toml_forge_user_field() {
    [ -f "$CREDENTIALS_FILE" ] || die "missing $CREDENTIALS_FILE"
    awk -v section="[forge.users.$1]" -v field="$2" '
        $0 == section { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section {
            line = $0
            sub(/^[ \t]*/, "", line)
            key = line
            sub(/[ \t]*=.*/, "", key)
            if (key == field) {
                val = line
                sub(/^[^=]*=[ \t]*/, "", val)
                gsub(/^"|"$/, "", val)
                print val
                exit
            }
        }
    ' "$CREDENTIALS_FILE"
}

# URL-encodes one value for a git-credentials store entry. python3 is already a
# demo dependency (the intake helper also uses it for JSON construction).
percent_encode() {
    python3 -c 'import sys, urllib.parse; sys.stdout.write(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

boot_jig() {
    log "starting local jig fake LLM provider from $JIG_REPO ..."
    : >"$LOG_DIR/jig.log"
    # The jig binary prints its bound base URL on stdout, then blocks until stdin
    # closes. Give it a FIFO opened read/write so stdin remains open without an
    # extra feeder process; cleanup kills jig and removes the FIFO.
    rm -f "$JIG_STDIN"
    mkfifo "$JIG_STDIN"
    "$JIG_BIN" "$JIG_FIXTURE_PATH" <>"$JIG_STDIN" >"$LOG_DIR/jig.log" 2>&1 &
    JIG_PID=$!
    echo "$JIG_PID" >"$JIG_PID_FILE"

    _i=0
    JIG_URL=
    while [ -z "$JIG_URL" ]; do
        kill -0 "$JIG_PID" 2>/dev/null || die "jig exited during startup (see logs/jig.log)"
        JIG_URL=$(sed -n 's#^\(http://[^[:space:]]*\).*#\1#p' "$LOG_DIR/jig.log" 2>/dev/null | sed -n '1p')
        [ -n "$JIG_URL" ] && break
        _i=$((_i + 1))
        [ "$_i" -gt 100 ] && die "jig did not print a base URL (see logs/jig.log)"
        sleep_short
    done
    # DeepSeek uses the SDK's OpenAI-compatible completions path, which appends
    # /chat/completions to the configured base URL. Jig serves that route at the
    # printed base itself, so do not add /v1 here.
    JIG_PROVIDER_URL=$JIG_URL
    log "jig ready at $JIG_URL (fixture $JIG_FIXTURE_PATH)"
}

bootstrap_admin() {
    log 'bootstrapping the throwaway Forgejo site admin ...'
    # Create the admin (tolerate a pre-existing one on a re-run), then mint an
    # all-scoped token. The token stays in a shell variable; it is never echoed,
    # reaches `temper init` only through the admin password env, and is reused by
    # populate_repo / seed_intake as the setup-only REST/API credential. The
    # workflow's intake_author = site_admin means the intake issue is authored by
    # THIS admin (the "external filer").
    forgejo_cli admin user create --username "$ADMIN_USER" --password "$ADMIN_PASSWORD" \
        --email "$ADMIN_EMAIL" --admin --must-change-password=false \
        >"$LOG_DIR/admin-create.log" 2>&1 || true
    ADMIN_TOKEN=$(forgejo_cli admin user generate-access-token --username "$ADMIN_USER" \
        --scopes all --raw | tr -d '[:space:]')
    [ -n "$ADMIN_TOKEN" ] || die 'failed to mint an admin access token'
}

patch_init_config_runtime_knobs() {
    # `temper init` owns the deployment config. The example only adds
    # demo-specific runtime knobs that init intentionally leaves at defaults:
    # backstop cadences, stable IDs, workspace/git base, and max iterations.
    TEMPER_CONFIG_FILE="$CONFIG_FILE" \
    TEMPER_DEMO_POLL="$DAEMON_POLL_CADENCE_SECS" \
    TEMPER_DEMO_MECHANICAL="$DAEMON_MECHANICAL_CADENCE_SECS" \
    TEMPER_DEMO_LEASE="$DAEMON_LEASE_TTL_SECS" \
    TEMPER_DEMO_WORKSPACE="$RUN_DIR/workspaces" \
    TEMPER_DEMO_GIT_BASE="$BASE_URL" \
    TEMPER_DEMO_MAX_ITERATIONS="$RUN_MAX_ITERATIONS" \
        python3 <<'PY'
import json
import os
from pathlib import Path

path = Path(os.environ["TEMPER_CONFIG_FILE"])
text = path.read_text(encoding="utf-8")
lines = text.splitlines()

updates = {
    "engine": [
        ("poll_cadence_secs", os.environ["TEMPER_DEMO_POLL"]),
        ("mechanical_cadence_secs", os.environ["TEMPER_DEMO_MECHANICAL"]),
        ("lease_ttl_secs", os.environ["TEMPER_DEMO_LEASE"]),
        ("daemon_id", json.dumps("basic-delivery-daemon")),
    ],
    "worker": [
        ("worker_id", json.dumps("basic-delivery-1")),
        ("workspace", json.dumps(os.environ["TEMPER_DEMO_WORKSPACE"])),
        ("git_base_url", json.dumps(os.environ["TEMPER_DEMO_GIT_BASE"])),
    ],
    "agent": [
        ("max_iterations", os.environ["TEMPER_DEMO_MAX_ITERATIONS"]),
    ],
}


def section_of(line: str):
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]") and not stripped.startswith("[["):
        return stripped.strip("[]")
    return None


def existing_keys(section: str):
    keys = set()
    in_section = False
    for line in lines:
        current = section_of(line)
        if current is not None:
            in_section = current == section
            continue
        if in_section and "=" in line and not line.lstrip().startswith("#"):
            keys.add(line.split("=", 1)[0].strip())
    return keys


def ensure_section(section: str, pairs):
    global lines
    keys = existing_keys(section)
    missing = [(k, v) for k, v in pairs if k not in keys]
    if not missing:
        return
    header = f"[{section}]"
    for idx, line in enumerate(lines):
        if line.strip() == header:
            insert_at = idx + 1
            while insert_at < len(lines):
                current = section_of(lines[insert_at])
                if current is not None:
                    break
                insert_at += 1
            lines[insert_at:insert_at] = [f"{k} = {v}" for k, v in missing]
            return
    if lines and lines[-1].strip():
        lines.append("")
    lines.append(header)
    lines.extend(f"{k} = {v}" for k, v in missing)

for section, pairs in updates.items():
    ensure_section(section, pairs)

path.write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
}

run_temper_init() {
    [ -n "${JIG_PROVIDER_URL:-}" ] || die 'run_temper_init: jig provider URL is not set'
    log "running temper init --non-interactive for $REPO ..."
    : >"$LOG_DIR/init.log"
    : >"$LOG_DIR/provision.log"
    (
        TEMPER_INIT_ADMIN_PASSWORD="$ADMIN_PASSWORD" \
        TEMPER_INIT_PROVIDER_KEY="$INIT_PROVIDER_KEY" \
            "$RUN_BIN" init --non-interactive --force \
                --forge "$BASE_URL" \
                --repo "$REPO" \
                --bind "$DAEMON_BIND" \
                --admin-user "$ADMIN_USER" \
                --provider deepseek \
                --provider-url "$JIG_PROVIDER_URL" \
                --config "$CONFIG_FILE" \
                --secrets "$CREDENTIALS_FILE"
    ) >"$LOG_DIR/init.log" 2>&1 || die "temper init failed (see logs/init.log)"

    [ -f "$CONFIG_FILE" ] || die "temper init did not write $CONFIG_FILE"
    [ -f "$CREDENTIALS_FILE" ] || die "temper init did not write $CREDENTIALS_FILE"
    [ -f "$INIT_WORKFLOW_PATH" ] || die "temper init did not write $INIT_WORKFLOW_PATH"
    [ -f "$WEBHOOK_SECRET_FILE" ] || die "temper init did not write $WEBHOOK_SECRET_FILE"
    patch_init_config_runtime_knobs

    {
        printf 'repo=%s initialized_by=temper_init config=%s credentials=%s workflow=%s webhook_secret=%s\n' \
            "$REPO" "$CONFIG_FILE" "$CREDENTIALS_FILE" "$INIT_WORKFLOW_PATH" "$WEBHOOK_SECRET_FILE"
        printf 'repo=%s webhook registered url=%s\n' "$REPO" "$WEBHOOK_URL"
        printf 'repo=%s provider=deepseek provider_url=%s fixture=%s\n' "$REPO" "$JIG_PROVIDER_URL" "$JIG_FIXTURE_PATH"
    } >>"$LOG_DIR/provision.log"
    log "temper init wrote config/credentials and registered the webhook for $REPO ($WEBHOOK_URL)"
}

populate_repo() {
    [ -n "${ADMIN_TOKEN:-}" ] || die 'populate_repo: no admin token (bootstrap_admin must run first)'
    _seed_dir="$RUN_DIR/repo-seed"
    _checkout="$_seed_dir/$(repo_slug "$REPO")"
    _creds="$_seed_dir/git-credentials"
    _remote="$BASE_URL/$REPO.git"
    _without_scheme=${BASE_URL#*://}

    log "creating the initial $DEFAULT_BRANCH commit for $REPO (README + demo CI) ..."
    rm -rf "$_seed_dir"
    mkdir -p "$_checkout"
    ( umask 077; printf 'http://%s:%s@%s\n' "$(percent_encode "$ADMIN_USER")" "$(percent_encode "$ADMIN_TOKEN")" "$_without_scheme" >"$_creds" )

    : >"$LOG_DIR/repo-populate.log"
    if ! git -C "$_checkout" init -b "$DEFAULT_BRANCH" >>"$LOG_DIR/repo-populate.log" 2>&1; then
        git -C "$_checkout" init >>"$LOG_DIR/repo-populate.log" 2>&1 \
            || die "repo population failed: git init (see logs/repo-populate.log)"
        git -C "$_checkout" checkout -b "$DEFAULT_BRANCH" >>"$LOG_DIR/repo-populate.log" 2>&1 \
            || die "repo population failed: git checkout -b $DEFAULT_BRANCH (see logs/repo-populate.log)"
    fi
    git -C "$_checkout" config user.email "$ADMIN_EMAIL" \
        && git -C "$_checkout" config user.name 'Basic Delivery Admin' \
        && git -C "$_checkout" config credential.helper "store --file=$_creds" \
        && git -C "$_checkout" remote add origin "$_remote" \
        || die "repo population failed: git config/remote setup (see logs/repo-populate.log)"

    mkdir -p "$_checkout/.forgejo/workflows"
    cp "$CONFIG_DIR/ci.yml" "$_checkout/.forgejo/workflows/ci.yml"
    cat >"$_checkout/README.md" <<EOF
# $REPO

Minimal project baseline for the Temper basic-delivery demo.

Temper initialized the Forgejo integration, and run.sh created this explicit
first commit so the demo starts from a normal repository instead of provisioned
content.
EOF

    git -C "$_checkout" add README.md .forgejo/workflows/ci.yml \
        && git -C "$_checkout" commit --quiet -m 'chore: initialize basic-delivery demo repository' >>"$LOG_DIR/repo-populate.log" 2>&1 \
        && git -C "$_checkout" push --quiet --set-upstream origin "HEAD:$DEFAULT_BRANCH" >>"$LOG_DIR/repo-populate.log" 2>&1 \
        || die "repo population failed: commit/push (see logs/repo-populate.log)"

    printf 'repo=%s initial_commit_branch=%s files=README.md,.forgejo/workflows/ci.yml\n' \
        "$REPO" "$DEFAULT_BRANCH" >>"$LOG_DIR/provision.log"
    log "created initial commit on $REPO@$DEFAULT_BRANCH"
}

# Files the single site-admin intake issue AFTER `temper serve standalone` is up
# by POSTing directly to Forgejo's REST API. The org/users/repo/labels, initial
# README+CI commit, and webhook already exist from init + populate_repo; this
# creates one unlabeled issue so the issue-created webhook demonstrates the wake
# path while the poll backstop is deliberately long.
seed_intake() {
    [ -n "${ADMIN_TOKEN:-}" ] || die 'seed_intake: no admin token (bootstrap_admin must run first)'
    _owner=$(repo_owner "$REPO")
    _name=$(repo_name "$REPO")
    log 'filing the site-admin intake issue now that temper serve standalone is ready ...'
    _issue_info=$(
        TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" \
        TEMPER_FORGEJO_BASE_URL="$BASE_URL" \
        TEMPER_FORGEJO_OWNER="$_owner" \
        TEMPER_FORGEJO_REPO="$_name" \
        TEMPER_INTAKE_TITLE="$INTAKE_TITLE" \
        TEMPER_INTAKE_BODY_PATH="$INTAKE_BODY_PATH" \
            python3 <<'PY'
import json
import os
import pathlib
import sys
import urllib.error
import urllib.parse
import urllib.request

base_url = os.environ["TEMPER_FORGEJO_BASE_URL"].rstrip("/")
owner = os.environ["TEMPER_FORGEJO_OWNER"]
repo = os.environ["TEMPER_FORGEJO_REPO"]
owner_path = urllib.parse.quote(owner, safe="")
repo_path = urllib.parse.quote(repo, safe="")
body_path = pathlib.Path(os.environ["TEMPER_INTAKE_BODY_PATH"])
try:
    body = body_path.read_text(encoding="utf-8")
except OSError as exc:
    print(f"failed to read intake body {body_path}: {exc}", file=sys.stderr)
    sys.exit(1)

payload = json.dumps({
    "title": os.environ["TEMPER_INTAKE_TITLE"],
    "body": body,
}).encode("utf-8")
request = urllib.request.Request(
    f"{base_url}/api/v1/repos/{owner_path}/{repo_path}/issues",
    data=payload,
    headers={
        "Accept": "application/json",
        "Authorization": f"token {os.environ['TEMPER_FORGEJO_ADMIN_TOKEN']}",
        "Content-Type": "application/json",
    },
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        raw = response.read()
except urllib.error.HTTPError as exc:
    detail = exc.read().decode("utf-8", "replace")
    print(f"Forgejo issue create failed: HTTP {exc.code} {exc.reason}: {detail}", file=sys.stderr)
    sys.exit(1)
except urllib.error.URLError as exc:
    print(f"Forgejo issue create failed: {exc.reason}", file=sys.stderr)
    sys.exit(1)

try:
    issue = json.loads(raw.decode("utf-8"))
except json.JSONDecodeError as exc:
    print(f"Forgejo issue create returned invalid JSON: {exc}", file=sys.stderr)
    sys.exit(1)

number = issue.get("number")
if number is None:
    print("Forgejo issue create response did not include an issue number", file=sys.stderr)
    sys.exit(1)
html_url = issue.get("html_url") or f"{base_url}/{owner_path}/{repo_path}/issues/{number}"
print(number)
print(html_url)
PY
    ) || die "filing intake issue for $REPO failed"

    _issue=$(printf '%s\n' "$_issue_info" | sed -n '1p')
    _issue_url=$(printf '%s\n' "$_issue_info" | sed -n '2p')
    [ -n "$_issue" ] || die "filing intake issue for $REPO did not return an issue number"
    [ -n "$_issue_url" ] || _issue_url="$BASE_URL/$REPO/issues/$_issue"
    _status="created intake issue #$_issue at $_issue_url"
    {
        printf 'repo=%s %s\n' "$REPO" "$_status"
        printf 'repo=%s intake_issue_number=%s intake_issue_url=%s\n' "$REPO" "$_issue" "$_issue_url"
    } >>"$LOG_DIR/provision.log"
    log "$_status"
    log "  intake issue: $_issue_url (filing it should drive the webhook path)"
}

# --- temper serve standalone --------------------------------------------------

# Boots one `temper serve standalone`: the daemon, one worker, and the
# in-process coding agent on a single event loop. The deployment is loaded from
# the config.toml and credentials.toml emitted by `temper init`.
boot_run() {
    [ -f "$CONFIG_FILE" ] || die "missing $CONFIG_FILE (run_temper_init must run first)"
    [ -f "$CREDENTIALS_FILE" ] || die "missing $CREDENTIALS_FILE (run_temper_init must run first)"
    mkdir -p "$RUN_DIR/workspaces"

    log "starting temper serve standalone at $DAEMON_BIND (poll=${DAEMON_POLL_CADENCE_SECS}s mechanical=${DAEMON_MECHANICAL_CADENCE_SECS}s provider=deepseek/jig) ..."
    : >"$LOG_DIR/run.log"
    "$RUN_BIN" serve standalone --config "$CONFIG_FILE" --credentials "$CREDENTIALS_FILE" \
        >"$LOG_DIR/run.log" 2>&1 &
    RUN_PID=$!
    echo "$RUN_PID" >"$RUN_PID_FILE"
    # Readiness: the webhook listener must be up before the seed-last webhook can
    # be delivered, the in-process worker must have announced capacity before any
    # job can be assigned, and the standalone boot banner must report ready/idle.
    wait_for_log_line "$LOG_DIR/run.log" 'webhook listener up' "$RUN_PID" 'temper serve standalone'
    wait_for_log_line "$LOG_DIR/run.log" 'worker:  capacity:' "$RUN_PID" 'temper serve standalone'
    wait_for_log_line "$LOG_DIR/run.log" 'ready -- watching' "$RUN_PID" 'temper serve standalone'
    log "temper serve standalone up (pid $RUN_PID; logs/run.log)"
}

# --- Webhook validation -------------------------------------------------------

count_matches() {
    _pattern=$1
    _file=$2
    _count=$(grep -c "$_pattern" "$_file" 2>/dev/null || true)
    [ -n "$_count" ] || _count=0
    printf '%s\n' "$_count"
}

validate_contains() {
    _file=$1
    _pattern=$2
    _description=$3
    if grep -F -q "$_pattern" "$_file" 2>/dev/null; then
        log "ok: $_description"
        return 0
    fi
    log "missing: $_description (looked in $_file)"
    return 1
}

# Confirms `temper serve standalone` has the bot automation credentials it needs to merge
# CI-green PRs and read Forgejo 7.0.x Actions status (ADR 0019).
validate_mechanical_bot_config() {
    _ok=0
    if [ ! -f "$CREDENTIALS_FILE" ]; then
        log "missing: $CREDENTIALS_FILE not found; cannot confirm bot automation credentials"
        log 'diagnosis: Forgejo 7.0.x CI reads need web-UI credentials for the mechanical backstop (ADR 0019)'
        return 1
    fi
    _bot_user=$(toml_forge_user_field bot user)
    [ -n "$_bot_user" ] || _bot_user=bot
    if [ "$_bot_user" = "bot" ] && [ -n "$(toml_forge_user_field bot token)" ] \
        && [ -n "$(toml_forge_user_field bot password)" ]; then
        log 'ok: bot automation token + web-UI credentials present for the mechanical backstop'
    else
        log "missing: bot automation user token/username/password in $CREDENTIALS_FILE"
        log 'diagnosis: rerun temper init so credentials.toml contains the bot token/password used for landing and ADR-0019 CI reads'
        _ok=1
    fi
    return "$_ok"
}

# Checks that no CI read fallback error (missing/unusable web-UI credentials) was
# reported for the mechanical landing gate.
validate_mechanical_ci_log() {
    _ok=0
    _run_log="$LOG_DIR/run.log"
    if [ ! -f "$_run_log" ]; then
        log 'missing: logs/run.log exists for mechanical CI-read diagnostics'
        return 1
    fi
    if grep -F -q "$CI_FALLBACK_MISSING_CREDENTIALS" "$_run_log" 2>/dev/null; then
        log 'missing: temper serve standalone reported missing Forgejo web-UI credentials for CI reads'
        log 'diagnosis: the landing queue needs native CI; verify the bot password in the init-emitted credentials.toml (ADR 0019)'
        _ok=1
    fi
    if grep -F -q "$CI_FALLBACK_LOGIN_FAILED" "$_run_log" 2>/dev/null; then
        log 'missing: temper serve standalone could not log in to Forgejo web UI for CI reads'
        log 'diagnosis: verify the bot automation credentials in secrets/credentials.toml'
        _ok=1
    fi
    if [ "$_ok" -eq 0 ]; then
        log 'ok: mechanical CI read fallback reported no missing/unusable web-UI credentials'
    fi
    return "$_ok"
}

cmd_validate_webhooks() {
    load_config
    _ok=0
    _run_log="$LOG_DIR/run.log"
    _provision_log="$LOG_DIR/provision.log"

    [ -d "$LOG_DIR" ] || die "no logs/ directory yet; start a run first"
    log "validating webhook logs under $LOG_DIR"
    log "configured repo: $REPO"
    log "configured DAEMON_POLL_CADENCE_SECS=$DAEMON_POLL_CADENCE_SECS DAEMON_MECHANICAL_CADENCE_SECS=$DAEMON_MECHANICAL_CADENCE_SECS; long-poll smoke expects DAEMON_POLL_CADENCE_SECS=120"

    validate_mechanical_bot_config || _ok=1
    validate_mechanical_ci_log || _ok=1

    validate_contains "$_provision_log" 'webhook registered url=' \
        'repo webhook registration recorded' || _ok=1
    validate_contains "$_run_log" 'ready -- watching' \
        'temper serve standalone reached serving readiness' || _ok=1
    validate_contains "$_run_log" 'webhook accepted' \
        'Forgejo delivered at least one accepted webhook' || _ok=1
    validate_contains "$_run_log" 'webhook wake scan' \
        'temper serve standalone ran at least one webhook wake scan' || _ok=1
    if grep -E -q 'webhook wake scan.*enqueued=[1-9][0-9]*' "$_run_log" 2>/dev/null; then
        log 'ok: webhook wake scan enqueued work'
    else
        log 'missing: no webhook wake scan reported enqueued>0'
        _ok=1
    fi
    validate_contains "$_run_log" 'assigned job_id=' \
        'engine assigned at least one job' || _ok=1
    validate_contains "$_run_log" 'result received' \
        'daemon received at least one job result' || _ok=1

    validate_contains "$_run_log" 'worker:  capacity:' \
        'in-process worker announced standalone capacity' || _ok=1
    validate_contains "$_run_log" 'worker: assigned job_id=' \
        'in-process worker accepted at least one assignment' || _ok=1
    validate_contains "$_run_log" 'worker: result sent' \
        'in-process worker sent at least one result' || _ok=1

    _accepted=$(count_matches 'webhook accepted' "$_run_log")
    _wake_scans=$(count_matches 'webhook wake scan' "$_run_log")
    _wake_enqueued=$(grep -E -c 'webhook wake scan.*enqueued=[1-9][0-9]*' "$_run_log" 2>/dev/null || true)
    _assigned=$(count_matches 'assigned job_id=' "$_run_log")
    _results=$(count_matches 'result received' "$_run_log")
    log "daemon summary: accepted=$_accepted wake_scans=$_wake_scans wake_enqueued=$_wake_enqueued assigned=$_assigned result_received=$_results"

    _capacity=$(count_matches 'worker:  capacity:' "$_run_log")
    _worker_assigned=$(count_matches 'worker: assigned job_id=' "$_run_log")
    _worker_results=$(count_matches 'worker: result sent' "$_run_log")
    log "worker summary: capacity=$_capacity assigned=$_worker_assigned result_sent=$_worker_results"

    if [ "$_ok" -eq 0 ]; then
        log 'webhook validation passed'
    else
        log 'webhook validation failed; inspect logs/provision.log and logs/run.log'
    fi
    return "$_ok"
}

# --- Monitor ------------------------------------------------------------------

# Blocks until the stop-file appears, the server dies, or RUN_SECS elapses, so
# the EXIT/INT/TERM trap can tear everything down on Ctrl-C.
monitor() {
    log ''
    log "Forgejo UI:   $BASE_URL  (log in as any provisioned role)"
    log "temper serve: http://$DAEMON_BIND  (webhook + poll/mechanical backstops + in-process coding agent for $REPO)"
    log "Jig LLM:      ${JIG_URL:-unknown}  (fixture $JIG_FIXTURE_PATH)"
    log "Intake issue: $BASE_URL/$REPO/issues"
    log "Logs:         $LOG_DIR/run.log"
    log 'The intake issue is filed once temper serve standalone is ready, so its'
    log 'webhook drives the wake scan; the architect triages it to a ready code'
    log 'issue, the engineer opens an implementation PR, CI runs and goes green,'
    log 'and the mechanical backstop auto-merges it — no human.'
    log ''
    log "Press Ctrl-C (or run '$DISPLAY_SCRIPT stop') to tear everything down."

    _waited=0
    while [ ! -f "$STOP_FILE" ]; do
        sleep 2
        _waited=$((_waited + 2))
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            log 'forgejo server exited; shutting down.'
            break
        fi
        if [ "$_waited" -ge "$RUN_SECS" ]; then
            log "run-secs backstop ($RUN_SECS s) reached; shutting down."
            break
        fi
    done
}

# --- Start --------------------------------------------------------------------

cmd_start() {
    load_config
    resolve_binaries

    if [ -f "$SERVER_PID_FILE" ] && kill -0 "$(cat "$SERVER_PID_FILE" 2>/dev/null)" 2>/dev/null; then
        die "a run appears active (run/server.pid). Stop it first: $DISPLAY_SCRIPT stop"
    fi

    mkdir -p "$RUN_DIR" "$LOG_DIR" "$SECRETS_DIR"
    rm -f "$STOP_FILE"

    # From here on, tear everything down on any exit/interrupt.
    trap cleanup EXIT INT TERM

    boot_server
    boot_runner
    boot_jig
    bootstrap_admin
    run_temper_init
    populate_repo
    boot_run
    seed_intake
    monitor
    # cleanup runs via the EXIT trap.
}

# --- Dispatch -----------------------------------------------------------------

case "${1:-start}" in
    start | "") cmd_start ;;
    validate-webhooks | smoke-webhooks) cmd_validate_webhooks ;;
    stop) cmd_stop ;;
    help | -h | --help) usage ;;
    *)
        usage >&2
        exit 2
        ;;
esac
