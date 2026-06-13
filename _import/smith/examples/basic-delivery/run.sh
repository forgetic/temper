#!/bin/sh
# basic-delivery example — POSIX launcher / teardown.
#
# The minimal, no-human-in-the-loop counterpart to reference-delivery: ONE repo,
# TWO human-capable workflow roles (architect + engineer) plus a mechanical bot,
# CI, webhooks on, and landing gated on CI alone. It boots every process of the
# two-tier production topology from development-profile binaries:
#   1. a throwaway Forgejo server (SQLite, Actions enabled),
#   2. a host-mode forgejo-runner producing real CI,
#   3. admin bootstrap + the production provision binary against the bundled
#      3-role workflow (--workflow), creating the org/users/repo/labels/CI and
#      registering the daemon webhook — but deliberately NOT yet filing the
#      intake issue,
#   4. one temper-daemon: webhook route, long poll backstop, short mechanical
#      CI/landing backstop, leases, per-role apply tokens, and result appliers,
#   5. one smith-worker with coding executor capability for architect + engineer,
#      persistent workspaces, and per-role git credentials,
#   6. only once the daemon and worker are ready, a second seed-only provision
#      pass (--seed-only) files ONE unlabeled intake issue authored by the SITE
#      ADMIN (the workflow's intake_author = site_admin), so the issue-created
#      webhook is the demonstrated wake path.
# The Smith coding agent lets the architect triage the intake to a ready code
# issue and the engineer open a real implementation PR; CI runs, goes green, and
# the daemon's mechanical backstop auto-merges — no reviewer, owner, or human. It
# tears everything down cleanly on Ctrl-C / signal / `./run.sh stop`.
#
# This script targets the operator-facing entry points from Temper's root
# package and Smith's worker package. By default it builds/uses development
# binaries under target/debug; override TEMPER_*_BIN / SMITH_WORKER_BIN for
# prebuilt or release artifacts.
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
#       pkill -f temper-daemon
#       pkill -f smith-worker
#       rm -rf examples/basic-delivery/run
#
# POSIX sh only (no bashisms). Validate with `sh -n run.sh` (and shellcheck).
# Secrets travel by env or the sourced secrets files, NEVER on a command line.

set -eu

# --- Locations ----------------------------------------------------------------
if [ -n "${TEMPER_BASIC_DELIVERY_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$TEMPER_BASIC_DELIVERY_SCRIPT_DIR
else
    SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fi
SMITH_WORKSPACE_ROOT_DEFAULT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
ANVIL_WORKSPACE_ROOT_DEFAULT=$(CDPATH= cd -- "$SMITH_WORKSPACE_ROOT_DEFAULT/../anvil" && pwd)
WORKSPACE_ROOT=${TEMPER_WORKSPACE_ROOT:-$(CDPATH= cd -- "$SMITH_WORKSPACE_ROOT_DEFAULT/../temper" && pwd)}
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
DAEMON_PID_FILE="$RUN_DIR/daemon.pid"
WORKER_PID_FILE="$RUN_DIR/worker.pid"
ROLES_ENV="$SECRETS_DIR/roles.env"
WEBHOOK_SECRET_FILE="$SECRETS_DIR/webhook-secret"

# Pinned versions for the bundled throwaway server/runner used by this example.
FORGEJO_VERSION=7.0.12
FORGEJO_RUNNER_VERSION=3.5.1

# Throwaway admin identity. This is also the workflow's intake_author
# (site_admin): the bundled workflow.json declares intake_author = site_admin, so
# the provisioner seeds the intake issue as THIS admin (the "external filer").
# The server is killed + wiped on teardown; this is never a credential that
# reaches anything real, and never echoed.
ADMIN_USER=basicadmin
ADMIN_EMAIL=basicadmin@example.invalid
ADMIN_PASSWORD='Basic-Delivery-Admin-1!'

# Diagnostic strings emitted when Forgejo 7.0.x Actions status cannot be read by
# the ADR-0019 web-UI fallback. The daemon hosts the mechanical CI-read path.
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

  start (default)      boot Forgejo + runner, provision the single repo against
                       the bundled 3-role workflow, launch one temper-daemon and
                       one smith-worker (architect + engineer coding executor),
                       then file one site-admin intake issue so its webhook wakes
                       the daemon/worker path, and block until Ctrl-C or the
                       stop-file.
  validate-webhooks    inspect logs/ and report whether the daemon webhook was
                       registered, accepted, scanned, assigned, and completed.
  stop                 tear down a previous run via run/*.pid.
  help                 show this message.

Configuration is read from config/temper.env (no secrets). Concrete LLM
provider/auth options are passed to Smith as opaque coding-agent arguments; see
Smith's README.
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

# Tears down worker, daemon, runner, and server (in that order) and clears run
# state. Idempotent: safe to call from the EXIT trap and from `./run.sh stop`.
cleanup() {
    trap - EXIT INT TERM
    log 'tearing down...'
    [ -d "$RUN_DIR" ] && : >"$STOP_FILE" 2>/dev/null || true
    sleep 1
    stop_pid_file "$WORKER_PID_FILE"
    stop_pid_file "$DAEMON_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    stop_pid_file "$SERVER_PID_FILE"
    # Drop throwaway server/runner data + runtime checkouts + sentinel so a
    # re-run starts fresh; keep logs/ for inspection.
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$STOP_FILE" \
        "$RUN_DIR/ci-seed" "$RUN_DIR/workspaces" "$RUN_DIR"/run.sh.snapshot.* \
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
CONFIG_KNOBS="OWNER NAME DEFAULT_BRANCH WORKFLOW_FILE INTAKE_TITLE INTAKE_BODY_FILE BASE_URL DAEMON_BIND WEBHOOK_URL \
DAEMON_POLL_CADENCE_SECS DAEMON_MECHANICAL_CADENCE_SECS DAEMON_LEASE_TTL_SECS RUN_SECS \
TEMPER_FORGEJO_GOMAXPROCS TEMPER_FORGEJO_BINARY TEMPER_FORGEJO_RUNNER_BINARY \
TEMPER_DAEMON_BIN TEMPER_PROVISION_BIN TEMPER_BUILD_PACKAGE \
SMITH_WORKSPACE_ROOT SMITH_WORKER_BIN ANVIL_WORKSPACE_ROOT WORKER_MAX_CONCURRENT \
BASIC_DELIVERY_CODER ANVIL_AGENT_BIN ANVIL_AGENT_ARGS"

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
    # Optional operator secret overrides (gitignored).
    if [ -f "$SECRETS_DIR/.env" ]; then
        # shellcheck disable=SC1090
        . "$SECRETS_DIR/.env"
    fi
    # Re-apply any non-empty pre-existing env value over the file's setting.
    for _k in $CONFIG_KNOBS; do
        eval "_p=\${_pre_$_k}"
        [ -n "$_p" ] && eval "$_k=\$_p"
    done

    OWNER=${OWNER:-acme}
    NAME=${NAME:-service}
    DEFAULT_BRANCH=${DEFAULT_BRANCH:-main}
    WORKFLOW_FILE=${WORKFLOW_FILE:-workflow.json}
    # The seeded intake issue is deliberately THIN: the site admin (external
    # filer) states only the overall intent, with no acceptance criteria, no
    # setting name, and no implementation detail. Turning that into an
    # implementable spec is the architect's job (the triage_intake_to_code
    # set_body rewrite) — that is what this example proves the architect can do.
    INTAKE_TITLE=${INTAKE_TITLE:-Service banner should identify the environment}
    INTAKE_BODY_FILE=${INTAKE_BODY_FILE:-intake-issue.md}
    BASE_URL=${BASE_URL:-http://127.0.0.1:4100}
    DAEMON_BIND=${DAEMON_BIND:-127.0.0.1:38100}
    WEBHOOK_URL=${WEBHOOK_URL:-http://$DAEMON_BIND/forgejo/webhook}
    DAEMON_POLL_CADENCE_SECS=${DAEMON_POLL_CADENCE_SECS:-120}
    DAEMON_MECHANICAL_CADENCE_SECS=${DAEMON_MECHANICAL_CADENCE_SECS:-2}
    DAEMON_LEASE_TTL_SECS=${DAEMON_LEASE_TTL_SECS:-300}
    RUN_SECS=${RUN_SECS:-600}
    TEMPER_FORGEJO_GOMAXPROCS=${TEMPER_FORGEJO_GOMAXPROCS:-2}
    TEMPER_FORGEJO_BINARY=${TEMPER_FORGEJO_BINARY:-}
    TEMPER_FORGEJO_RUNNER_BINARY=${TEMPER_FORGEJO_RUNNER_BINARY:-}
    TEMPER_DAEMON_BIN=${TEMPER_DAEMON_BIN:-}
    TEMPER_PROVISION_BIN=${TEMPER_PROVISION_BIN:-}
    TEMPER_BUILD_PACKAGE=${TEMPER_BUILD_PACKAGE:-temper}
    SMITH_WORKSPACE_ROOT=${SMITH_WORKSPACE_ROOT:-$SMITH_WORKSPACE_ROOT_DEFAULT}
    ANVIL_WORKSPACE_ROOT=${ANVIL_WORKSPACE_ROOT:-$ANVIL_WORKSPACE_ROOT_DEFAULT}
    SMITH_WORKER_BIN=${SMITH_WORKER_BIN:-}
    WORKER_MAX_CONCURRENT=${WORKER_MAX_CONCURRENT:-1}
    BASIC_DELIVERY_CODER=${BASIC_DELIVERY_CODER:-anvil}
    case "$BASIC_DELIVERY_CODER" in
        anvil | greeting) ;;
        *) die "BASIC_DELIVERY_CODER must be anvil or greeting, got '$BASIC_DELIVERY_CODER'" ;;
    esac
    ANVIL_AGENT_BIN=${ANVIL_AGENT_BIN:-}
    ANVIL_AGENT_ARGS=${ANVIL_AGENT_ARGS:-}
    if [ -z "$ANVIL_AGENT_ARGS" ]; then
        ANVIL_AGENT_ARGS='--auth chatgpt-oauth'
    fi

    require_positive_int DAEMON_POLL_CADENCE_SECS "$DAEMON_POLL_CADENCE_SECS"
    require_positive_int DAEMON_MECHANICAL_CADENCE_SECS "$DAEMON_MECHANICAL_CADENCE_SECS"
    require_positive_int DAEMON_LEASE_TTL_SECS "$DAEMON_LEASE_TTL_SECS"
    require_positive_int RUN_SECS "$RUN_SECS"
    require_positive_int WORKER_MAX_CONCURRENT "$WORKER_MAX_CONCURRENT"

    # Single repo only: this example is deliberately one converging happy path.
    REPO="$OWNER/$NAME"
    validate_repo_path "$REPO"

    # Resolve the workflow file. A relative WORKFLOW_FILE is taken relative to
    # config/; an absolute path is used verbatim.
    case "$WORKFLOW_FILE" in
        /*) WORKFLOW_PATH="$WORKFLOW_FILE" ;;
        *)  WORKFLOW_PATH="$CONFIG_DIR/$WORKFLOW_FILE" ;;
    esac
    [ -f "$WORKFLOW_PATH" ] || die "workflow file not found: $WORKFLOW_PATH (set WORKFLOW_FILE in config/temper.env)"

    # Resolve the thin intake body the same way: a relative path is taken
    # relative to config/, an absolute path is used verbatim.
    case "$INTAKE_BODY_FILE" in
        /*) INTAKE_BODY_PATH="$INTAKE_BODY_FILE" ;;
        *)  INTAKE_BODY_PATH="$CONFIG_DIR/$INTAKE_BODY_FILE" ;;
    esac
    [ -f "$INTAKE_BODY_PATH" ] || die "intake body file not found: $INTAKE_BODY_PATH (set INTAKE_BODY_FILE in config/temper.env)"

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
    DAEMON_BIN=${TEMPER_DAEMON_BIN:-$WORKSPACE_ROOT/target/debug/temper-daemon}
    PROVISION_BIN=${TEMPER_PROVISION_BIN:-$WORKSPACE_ROOT/target/debug/temper-provision-forgejo}
    WORKER_BIN=${SMITH_WORKER_BIN:-$SMITH_WORKSPACE_ROOT/target/debug/smith-worker}

    # Keep the demo entry point self-healing after source changes. Cargo is a
    # cheap no-op when the development binaries are already current; skipping
    # this is an explicit operator choice for prebuilt/current binaries.
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring Temper development binaries are current (cargo build -p $TEMPER_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$TEMPER_BUILD_PACKAGE" ) \
            || die 'Temper cargo build failed'
        log 'ensuring Smith worker is current (cargo build -p smith-worker)...'
        ( cd "$SMITH_WORKSPACE_ROOT" && cargo build -p smith-worker ) \
            || die 'Smith worker cargo build failed'
        if [ "$BASIC_DELIVERY_CODER" = "anvil" ]; then
            log 'ensuring anvil-agent is current (cargo build --bin anvil-agent)...'
            ( cd "$ANVIL_WORKSPACE_ROOT" && cargo build --bin anvil-agent ) \
                || die 'anvil-agent cargo build failed'
        fi
    fi

    [ -x "$DAEMON_BIN" ] || die "daemon binary not found: $DAEMON_BIN"
    [ -x "$PROVISION_BIN" ] || die "provision binary not found: $PROVISION_BIN"
    [ -x "$WORKER_BIN" ] || die "smith-worker binary not found: $WORKER_BIN"

    # This example requires the runtime workflow provisioner, the daemon's
    # mechanical cadence flag, and the worker's coding executor surface. Refuse
    # to run against stale development binaries.
    _provision_help=$("$PROVISION_BIN" --help 2>&1 || true)
    case "$_provision_help" in
        *--workflow*--seed-intake*--seed-only*) ;;
        *) die "provision binary is stale or incompatible: $PROVISION_BIN does not advertise --workflow/--seed-intake/--seed-only. Re-run without TEMPER_SKIP_BUILD=1 or rebuild the Temper entry-point package with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac
    _daemon_help=$("$DAEMON_BIN" --help 2>&1 || true)
    case "$_daemon_help" in
        *--mechanical-cadence-secs*) ;;
        *) die "daemon binary is stale or incompatible: $DAEMON_BIN does not advertise --mechanical-cadence-secs. Re-run without TEMPER_SKIP_BUILD=1 or rebuild the Temper entry-point package with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac
    _worker_help=$("$WORKER_BIN" --help 2>&1 || true)
    case "$_worker_help" in
        *--executor*) ;;
        *) die "smith-worker binary is stale or incompatible: $WORKER_BIN does not advertise --executor. Re-run without TEMPER_SKIP_BUILD=1 or rebuild Smith's smith-worker package." ;;
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

    case "$BASIC_DELIVERY_CODER" in
        greeting)
            GREETING_CODER_BIN=$SCRIPT_DIR/tools/greeting-coder.sh
            [ -f "$GREETING_CODER_BIN" ] || die "greeting coder script not found: $GREETING_CODER_BIN"
            [ -x "$GREETING_CODER_BIN" ] || die "greeting coder script is not executable: $GREETING_CODER_BIN"
            log "coding agent: deterministic greeting stand-in ($GREETING_CODER_BIN)"
            ;;
        anvil)
            # The coding agent is anvil-agent, spawned out-of-process by
            # smith-worker (`--agent-command anvil-native`). The built binary is
            # passed via --agent-program; ANVIL_AGENT_ARGS carries the agent's
            # auth flags, passed through as --agent-arg.
            AGENT_BIN=${ANVIL_AGENT_BIN:-$ANVIL_WORKSPACE_ROOT/target/debug/anvil-agent}
            [ -x "$AGENT_BIN" ] || die "anvil-agent binary not found: $AGENT_BIN
       Set ANVIL_AGENT_BIN, or build it with: cargo build --bin anvil-agent (in $ANVIL_WORKSPACE_ROOT)"
            log "coding agent: anvil-agent ($AGENT_BIN; args: $ANVIL_AGENT_ARGS)"
            ;;
    esac
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

# --- Provision + seed ---------------------------------------------------------

repo_slug() {
    repo_name "$1" | tr -c '[:alnum:]' '-' | tr '[:upper:]' '[:lower:]' | sed 's/^-*//;s/-*$//'
}

# Uppercases a role id and replaces non-alphanumerics with `_` (matching the
# provision binary's env_role_key), yielding the secrets-file variable suffix.
role_env_key() {
    printf '%s' "$1" | tr '[:lower:]' '[:upper:]' | tr -c 'A-Z0-9' '_'
}

roles_from_roles_env() {
    [ -f "$ROLES_ENV" ] || die "missing $ROLES_ENV"
    sed -n \
        -e "s/^TEMPER_FORGEJO_USER_[A-Z0-9_]*='\(.*\)'\$/\1/p" \
        -e 's/^TEMPER_FORGEJO_USER_[A-Z0-9_]*=\([^'"'"'"].*\)$/\1/p' \
        "$ROLES_ENV"
}

bootstrap_and_provision() {
    log 'bootstrapping admin + provisioning the single repo against the bundled 3-role workflow ...'
    # Create the admin (tolerate a pre-existing one on a re-run), then mint an
    # all-scoped token. The token stays in a shell variable; it is never echoed
    # and reaches the provision steps only via the environment. It is also kept
    # for the later seed_intake pass: the workflow's intake_author = site_admin
    # means the intake issue is authored by THIS admin (the "external filer").
    # This pass deliberately runs with --seed-intake no: it sets up the
    # org/users/repo/labels/CI and registers the daemon webhook but does NOT file
    # the intake issue, so daemon + worker can come up first and the issue's
    # creation webhook is what drives the first demonstrated progress.
    forgejo_cli admin user create --username "$ADMIN_USER" --password "$ADMIN_PASSWORD" \
        --email "$ADMIN_EMAIL" --admin --must-change-password=false \
        >"$LOG_DIR/admin-create.log" 2>&1 || true
    ADMIN_TOKEN=$(forgejo_cli admin user generate-access-token --username "$ADMIN_USER" \
        --scopes all --raw | tr -d '[:space:]')
    [ -n "$ADMIN_TOKEN" ] || die 'failed to mint an admin access token'

    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    _webhook_args="--webhook-url $WEBHOOK_URL --webhook-secret-file $WEBHOOK_SECRET_FILE"
    : >"$LOG_DIR/provision.log"

    _owner=$(repo_owner "$REPO")
    _name=$(repo_name "$REPO")
    log "provisioning $REPO (labels + CI + webhook; intake filed after daemon + worker readiness) ..."
    # _webhook_args intentionally word-split: POSIX sh has no arrays and the
    # values above are controlled by this script/config. --seed-intake no holds
    # the intake issue back for the post-launch seed_intake pass.
    # shellcheck disable=SC2086
    _status=$(TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
        --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$ROLES_ENV" \
        --workflow "$WORKFLOW_PATH" --seed-intake no \
        $_webhook_args) \
        || die "provisioning $REPO failed"

    {
        printf 'repo=%s %s\n' "$REPO" "$_status"
        printf 'repo=%s webhook registered url=%s\n' "$REPO" "$WEBHOOK_URL"
    } >>"$LOG_DIR/provision.log"
    log "$_status"
    log "  webhook registered for $REPO ($WEBHOOK_URL)"

    [ -f "$ROLES_ENV" ] || die "provision did not write $ROLES_ENV"
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
}

# Files the single site-admin intake issue AFTER the daemon and worker are up.
# This is a second, seed-only provision pass (--seed-only): the org/users/repo/
# labels/CI and the webhook already exist from bootstrap_and_provision, so this
# only creates the issue. The daemon's poll backstop is deliberately long; filing
# now lets the issue-created webhook demonstrate the wake path.
seed_intake() {
    [ -n "${ADMIN_TOKEN:-}" ] || die 'seed_intake: no admin token (bootstrap_and_provision must run first)'
    _owner=$(repo_owner "$REPO")
    _name=$(repo_name "$REPO")
    log 'filing the site-admin intake issue now that daemon + worker are ready ...'
    _status=$(TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
        --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$ROLES_ENV" \
        --workflow "$WORKFLOW_PATH" --seed-only \
        --intake-title "$INTAKE_TITLE" --intake-body-file "$INTAKE_BODY_PATH") \
        || die "seeding intake issue for $REPO failed"

    _issue=$(printf '%s\n' "$_status" | sed -n 's/.*intake issue #\([0-9][0-9]*\).*/\1/p')
    {
        printf 'repo=%s %s\n' "$REPO" "$_status"
        [ -n "$_issue" ] && printf 'repo=%s intake_issue_url=%s/%s/issues/%s\n' "$REPO" "$BASE_URL" "$REPO" "$_issue"
    } >>"$LOG_DIR/provision.log"
    log "$_status"
    if [ -n "$_issue" ]; then
        log "  intake issue: $BASE_URL/$REPO/issues/$_issue (filing it should drive the webhook path)"
    fi
}

# --- Demo CI seed -------------------------------------------------------------

# URL-encodes one value for a git-credentials store entry. python3 is already a
# demo dependency (the bundled CI workflow runs it via actions/checkout).
percent_encode() {
    python3 -c 'import sys, urllib.parse; sys.stdout.write(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

# Replaces the provisioned commit-message-marker CI with the bundled pass-through
# workflow so a real coder PR head (which carries an ordinary commit message, not
# the demo marker) clears the landing CI gate. Non-fatal: if this setup fails the
# rest of the topology still boots, but landing CI may not pass.
apply_demo_ci() {
    _key=$(role_env_key engineer)
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    eval "_eng_user=\${TEMPER_FORGEJO_USER_${_key}:-}"
    eval "_eng_password=\${TEMPER_FORGEJO_PASSWORD_${_key}:-}"
    if [ -z "$_eng_user" ] || [ -z "$_eng_password" ]; then
        log "demo CI seed: no engineer username/password in $ROLES_ENV; landing CI may not pass"
        return 0
    fi

    _seed_dir="$RUN_DIR/ci-seed"
    _checkout="$_seed_dir/$(repo_slug "$REPO")"
    _creds="$_seed_dir/git-credentials"
    _remote="$BASE_URL/$REPO.git"
    _without_scheme=${BASE_URL#*://}

    log "demo CI seed: cloning $REPO to apply bundled CI ..."
    rm -rf "$_seed_dir"
    mkdir -p "$_seed_dir"
    ( umask 077; printf 'http://%s:%s@%s\n' "$(percent_encode "$_eng_user")" "$(percent_encode "$_eng_password")" "$_without_scheme" >"$_creds" )
    if ! git -c credential.helper="store --file=$_creds" clone --quiet "$_remote" "$_checkout" >>"$LOG_DIR/ci-seed.log" 2>&1; then
        log "demo CI seed: clone of $REPO failed (see logs/ci-seed.log); landing CI may not pass"
        return 0
    fi
    if ! { git -C "$_checkout" config user.email "$_eng_user@example.invalid" \
        && git -C "$_checkout" config user.name 'Temper Engineer' \
        && git -C "$_checkout" config credential.helper "store --file=$_creds"; }; then
        log "demo CI seed: could not configure the checkout; landing CI may not pass"
        return 0
    fi

    _base=$(git -C "$_checkout" rev-parse --abbrev-ref HEAD 2>/dev/null || printf '%s' "$DEFAULT_BRANCH")
    mkdir -p "$_checkout/.forgejo/workflows"
    if cp "$CONFIG_DIR/ci.yml" "$_checkout/.forgejo/workflows/ci.yml" \
        && ! git -C "$_checkout" diff --quiet -- .forgejo/workflows/ci.yml; then
        if git -C "$_checkout" add .forgejo/workflows/ci.yml \
            && git -C "$_checkout" commit --quiet -m 'ci: use basic-delivery demo CI workflow' >>"$LOG_DIR/ci-seed.log" 2>&1 \
            && git -C "$_checkout" push --quiet origin "HEAD:$_base" >>"$LOG_DIR/ci-seed.log" 2>&1; then
            log "demo CI seed: applied bundled CI to $REPO@$_base"
        else
            log "demo CI seed: could not apply bundled CI to $REPO (see logs/ci-seed.log); landing CI may not pass"
        fi
    else
        log "demo CI seed: bundled CI already present for $REPO"
    fi
}

# --- Daemon + worker ----------------------------------------------------------

# Resolves the provisioned `bot` automation identity from the secrets file. The
# daemon uses it for the mechanical backstop: landing CI-green PRs and the
# ADR-0019 web-UI CI read fallback. The setup-only site admin never participates
# in the workflow.
resolve_bot_identity() {
    [ -f "$ROLES_ENV" ] || die "missing $ROLES_ENV"
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    BOT_USER=${TEMPER_FORGEJO_BOT_USER:-}
    BOT_TOKEN=${TEMPER_FORGEJO_BOT_TOKEN:-}
    BOT_PASSWORD=${TEMPER_FORGEJO_BOT_PASSWORD:-}
    [ -n "$BOT_USER" ] || die "automation user 'bot' has no username in $ROLES_ENV"
    [ "$BOT_USER" = "bot" ] || die "automation user must be 'bot' in $ROLES_ENV, got '$BOT_USER'"
    [ -n "$BOT_TOKEN" ] || die "automation user 'bot' has no token in $ROLES_ENV"
    [ -n "$BOT_PASSWORD" ] || die "automation user 'bot' has no password in $ROLES_ENV"
}

export_daemon_role_tokens() {
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    _roles=$(roles_from_roles_env)
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"
    for _role in $_roles; do
        _key=$(role_env_key "$_role")
        eval "_token=\${TEMPER_FORGEJO_TOKEN_${_key}:-}"
        [ -n "$_token" ] || die "no token for role '$_role' in $ROLES_ENV"
        export "TEMPER_FORGEJO_TOKEN_${_key}"
    done
}

export_worker_role_identities() {
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    _roles=$(roles_from_roles_env)
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"
    for _role in $_roles; do
        _key=$(role_env_key "$_role")
        eval "_user=\${TEMPER_FORGEJO_USER_${_key}:-}"
        eval "_token=\${TEMPER_FORGEJO_TOKEN_${_key}:-}"
        [ -n "$_user" ] || die "no username for role '$_role' in $ROLES_ENV"
        [ -n "$_token" ] || die "no token for role '$_role' in $ROLES_ENV"
        export "TEMPER_FORGEJO_USER_${_key}" "TEMPER_FORGEJO_TOKEN_${_key}"
        eval "_email=\${TEMPER_FORGEJO_EMAIL_${_key}:-}"
        if [ -n "$_email" ]; then
            export "TEMPER_FORGEJO_EMAIL_${_key}"
        fi
    done
}

boot_daemon() {
    resolve_bot_identity
    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    _roles=$(roles_from_roles_env)
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"

    set -- "$DAEMON_BIN" --bind "$DAEMON_BIND" --repo "$REPO"
    for _role in $_roles; do
        set -- "$@" --role "$_role"
    done
    set -- "$@" --workflow "$WORKFLOW_PATH" \
        --poll-cadence-secs "$DAEMON_POLL_CADENCE_SECS" \
        --mechanical-cadence-secs "$DAEMON_MECHANICAL_CADENCE_SECS" \
        --lease-ttl-secs "$DAEMON_LEASE_TTL_SECS" \
        --webhook-secret-file "$WEBHOOK_SECRET_FILE" \
        --daemon-id basic-delivery-daemon

    log "starting temper-daemon at $DAEMON_BIND (poll=${DAEMON_POLL_CADENCE_SECS}s mechanical=${DAEMON_MECHANICAL_CADENCE_SECS}s) ..."
    : >"$LOG_DIR/daemon.log"
    (
        export_daemon_role_tokens
        FORGEJO_URL="$BASE_URL" \
        FORGEJO_ACCESS_TOKEN="$BOT_TOKEN" \
        FORGEJO_USERNAME="$BOT_USER" \
        FORGEJO_PASSWORD="$BOT_PASSWORD" \
            "$@"
    ) >"$LOG_DIR/daemon.log" 2>&1 &
    DAEMON_PID=$!
    echo "$DAEMON_PID" >"$DAEMON_PID_FILE"
    wait_for_log_line "$LOG_DIR/daemon.log" 'temper-daemon: serving on' "$DAEMON_PID" 'temper-daemon'
    log "temper-daemon running (pid $DAEMON_PID; logs/daemon.log)"
}

boot_worker() {
    _roles=$(roles_from_roles_env)
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"
    mkdir -p "$RUN_DIR/workspaces"

    case "$BASIC_DELIVERY_CODER" in
        greeting) _agent_command=$GREETING_CODER_BIN ;;
        # `anvil-native` selects the native anvil agent surface: the worker
        # spawns anvil-agent out-of-process; --agent-program points it at the
        # binary built from the sibling anvil checkout.
        anvil) _agent_command=anvil-native ;;
        *) die "unknown BASIC_DELIVERY_CODER '$BASIC_DELIVERY_CODER'" ;;
    esac

    set -- "$WORKER_BIN" \
        --daemon-url "http://$DAEMON_BIND" \
        --worker-id basic-delivery-1 \
        --executor coding \
        --workspace-root "$RUN_DIR/workspaces" \
        --git-base-url "$BASE_URL" \
        --agent-command "$_agent_command"
    for _role in $_roles; do
        set -- "$@" --capability "$REPO:$_role"
    done
    if [ "$BASIC_DELIVERY_CODER" = "anvil" ]; then
        set -- "$@" --agent-arg --agent-program --agent-arg "$AGENT_BIN"
        for _agent_arg in $ANVIL_AGENT_ARGS; do
            set -- "$@" --agent-arg "$_agent_arg"
        done
    fi
    [ -n "$WORKER_MAX_CONCURRENT" ] && set -- "$@" --max-concurrent "$WORKER_MAX_CONCURRENT"

    log "starting smith-worker with capabilities for $REPO roles: $_roles ..."
    : >"$LOG_DIR/worker.log"
    (
        export_worker_role_identities
        "$@"
    ) >"$LOG_DIR/worker.log" 2>&1 &
    WORKER_PID=$!
    echo "$WORKER_PID" >"$WORKER_PID_FILE"
    wait_for_log_line "$LOG_DIR/worker.log" 'smith-worker: registered' "$WORKER_PID" 'smith-worker'
    log "smith-worker running (pid $WORKER_PID; logs/worker.log)"
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

# Confirms the daemon has the bot automation credentials it needs to merge
# CI-green PRs and read Forgejo 7.0.x Actions status (ADR 0019).
validate_mechanical_bot_config() {
    _ok=0
    if [ ! -f "$ROLES_ENV" ]; then
        log "missing: $ROLES_ENV not found; cannot confirm bot automation credentials"
        log 'diagnosis: Forgejo 7.0.x CI reads need web-UI credentials for the daemon mechanical backstop (ADR 0019)'
        return 1
    fi
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    if [ "${TEMPER_FORGEJO_BOT_USER:-}" = "bot" ] && [ -n "${TEMPER_FORGEJO_BOT_TOKEN:-}" ] \
        && [ -n "${TEMPER_FORGEJO_BOT_PASSWORD:-}" ]; then
        log 'ok: bot automation token + web-UI credentials present for daemon mechanical backstop'
    else
        log "missing: bot automation user token/username/password in $ROLES_ENV"
        log 'diagnosis: provision the bot user and launch the daemon with its REST token plus FORGEJO_USERNAME/FORGEJO_PASSWORD for landing and the ADR-0019 CI read fallback'
        _ok=1
    fi
    return "$_ok"
}

# Checks that no CI read fallback error (missing/unusable web-UI credentials) was
# reported for the daemon-hosted mechanical landing gate.
validate_mechanical_ci_log() {
    _ok=0
    _daemon_log="$LOG_DIR/daemon.log"
    if [ ! -f "$_daemon_log" ]; then
        log 'missing: logs/daemon.log exists for mechanical CI-read diagnostics'
        return 1
    fi
    if grep -F -q "$CI_FALLBACK_MISSING_CREDENTIALS" "$_daemon_log" 2>/dev/null; then
        log 'missing: daemon reported missing Forgejo web-UI credentials for CI reads'
        log 'diagnosis: the landing queue needs native CI; pass the bot FORGEJO_USERNAME/FORGEJO_PASSWORD to the daemon (ADR 0019)'
        _ok=1
    fi
    if grep -F -q "$CI_FALLBACK_LOGIN_FAILED" "$_daemon_log" 2>/dev/null; then
        log 'missing: daemon could not log in to Forgejo web UI for CI reads'
        log 'diagnosis: verify the bot automation credentials in secrets/roles.env'
        _ok=1
    fi
    if [ "$_ok" -eq 0 ]; then
        log 'ok: daemon mechanical CI read fallback reported no missing/unusable web-UI credentials'
    fi
    return "$_ok"
}

cmd_validate_webhooks() {
    load_config
    _ok=0
    _daemon_log="$LOG_DIR/daemon.log"
    _worker_log="$LOG_DIR/worker.log"
    _provision_log="$LOG_DIR/provision.log"

    [ -d "$LOG_DIR" ] || die "no logs/ directory yet; start a run first"
    log "validating daemon webhook logs under $LOG_DIR"
    log "configured repo: $REPO"
    log "configured DAEMON_POLL_CADENCE_SECS=$DAEMON_POLL_CADENCE_SECS DAEMON_MECHANICAL_CADENCE_SECS=$DAEMON_MECHANICAL_CADENCE_SECS; long-poll smoke expects DAEMON_POLL_CADENCE_SECS=120"

    validate_mechanical_bot_config || _ok=1
    validate_mechanical_ci_log || _ok=1

    validate_contains "$_provision_log" 'webhook registered url=' \
        'repo webhook registration recorded' || _ok=1
    validate_contains "$_daemon_log" 'temper-daemon: serving on' \
        'daemon reached serving readiness' || _ok=1
    validate_contains "$_daemon_log" 'webhook accepted' \
        'Forgejo delivered at least one accepted webhook' || _ok=1
    validate_contains "$_daemon_log" 'webhook wake scan' \
        'daemon ran at least one webhook wake scan' || _ok=1
    if grep -E -q 'webhook wake scan.*enqueued=[1-9][0-9]*' "$_daemon_log" 2>/dev/null; then
        log 'ok: daemon webhook wake scan enqueued work'
    else
        log 'missing: no daemon webhook wake scan reported enqueued>0'
        _ok=1
    fi
    validate_contains "$_daemon_log" 'assigned job_id=' \
        'daemon assigned at least one job' || _ok=1
    validate_contains "$_daemon_log" 'result received' \
        'daemon received at least one job result' || _ok=1

    validate_contains "$_worker_log" 'smith-worker: registered' \
        'smith-worker registered with daemon' || _ok=1
    validate_contains "$_worker_log" 'smith-worker: assigned job_id=' \
        'smith-worker accepted at least one assignment' || _ok=1
    validate_contains "$_worker_log" 'smith-worker: result sent' \
        'smith-worker sent at least one result' || _ok=1

    _accepted=$(count_matches 'webhook accepted' "$_daemon_log")
    _wake_scans=$(count_matches 'webhook wake scan' "$_daemon_log")
    _wake_enqueued=$(grep -E -c 'webhook wake scan.*enqueued=[1-9][0-9]*' "$_daemon_log" 2>/dev/null || true)
    _assigned=$(count_matches 'assigned job_id=' "$_daemon_log")
    _results=$(count_matches 'result received' "$_daemon_log")
    log "daemon summary: accepted=$_accepted wake_scans=$_wake_scans wake_enqueued=$_wake_enqueued assigned=$_assigned result_received=$_results"

    _registered=$(count_matches 'smith-worker: registered' "$_worker_log")
    _worker_assigned=$(count_matches 'smith-worker: assigned job_id=' "$_worker_log")
    _worker_results=$(count_matches 'smith-worker: result sent' "$_worker_log")
    log "worker summary: registered=$_registered assigned=$_worker_assigned result_sent=$_worker_results"

    if [ "$_ok" -eq 0 ]; then
        log 'daemon webhook validation passed'
    else
        log 'daemon webhook validation failed; inspect logs/provision.log, logs/daemon.log, and logs/worker.log'
    fi
    return "$_ok"
}

# --- Monitor ------------------------------------------------------------------

# Blocks until the stop-file appears, the server dies, or RUN_SECS elapses, so
# the EXIT/INT/TERM trap can tear everything down on Ctrl-C.
monitor() {
    log ''
    log "Forgejo UI:    $BASE_URL  (log in as any provisioned role)"
    log "Daemon:       http://$DAEMON_BIND  (webhook + poll/mechanical backstops for $REPO)"
    log "Smith worker: architect + engineer capabilities for $REPO"
    log "Intake issue:  $BASE_URL/$REPO/issues"
    log "Logs:          $LOG_DIR/daemon.log and $LOG_DIR/worker.log"
    log 'The intake issue is filed once the daemon and worker are ready, so its'
    log 'webhook drives the daemon scan; the architect triages it to a ready code'
    log 'issue, the engineer opens an implementation PR, CI runs and goes green,'
    log 'and the daemon mechanical backstop auto-merges it — no human.'
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
    bootstrap_and_provision
    apply_demo_ci
    boot_daemon
    boot_worker
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
