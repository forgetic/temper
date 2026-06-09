#!/bin/sh
# basic-delivery example — POSIX launcher / teardown.
#
# The minimal, no-human-in-the-loop counterpart to reference-delivery: ONE repo,
# the three basic-delivery roles (architect, engineer, and the bot mechanical
# authority) + CI, webhooks on, and landing gated on CI alone. Like
# reference-delivery it drives deterministic fake agents (`temper-testing-worker`)
# against a real throwaway Forgejo and a real host-mode forgejo-runner; it has no
# Smith/LLM dependency. Unlike reference-delivery it loads its own 3-role spec at
# runtime (--workflow) and seeds the intake issue as the SITE ADMIN (the
# workflow's intake_author = site_admin), with no `human` role and no cross-repo
# fan-out — a single, converging happy path.
#
# Usage:
#   ./run.sh [start]          boot everything and block until Ctrl-C / stop-file
#   ./run.sh validate-webhooks inspect logs from a running/completed long-poll run
#   ./run.sh stop             tear down a previous run via the saved PIDs
#   ./run.sh help             show this usage
#
# Orphan cleanup — if a run is force-killed (SIGKILL) the trap guards do not
# fire; clean up survivors by hand with:
#       pkill -f forgejo
#       pkill -f forgejo-runner
#       pkill -f temper-testing-worker
#       pkill -f temper-trigger-forgejo
#       rm -rf examples/basic-delivery/run
#
# POSIX sh only. Secrets travel by env or sourced secrets files, never argv.

set -eu

# --- Locations ----------------------------------------------------------------
if [ -n "${TEMPER_BASIC_DELIVERY_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$TEMPER_BASIC_DELIVERY_SCRIPT_DIR
else
    SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fi
WORKSPACE_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
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
WORKERS_PID_FILE="$RUN_DIR/workers.pids"
TRIGGER_PID_FILE="$RUN_DIR/trigger.pid"
WAKE_DIR="$RUN_DIR/wake"
ROLES_ENV="$SECRETS_DIR/roles.env"
WEBHOOK_SECRET_FILE="$SECRETS_DIR/webhook-secret"
WAKE_SECRET_FILE="$SECRETS_DIR/wake-secret"

FORGEJO_VERSION=7.0.12
FORGEJO_RUNNER_VERSION=3.5.1

# Global Forgejo site admin. Used for initial setup (bootstrapping and
# provisioning) AND, because the bundled workflow declares intake_author =
# site_admin, it is the author of record for the seeded intake issue. It never
# otherwise participates in the workflow. Forgejo reserves the literal username
# `admin`, so this throwaway admin uses a valid siteadmin handle. Workflow
# automation (landing, CI reads) runs as the provisioned `bot` user instead.
ADMIN_USER=basicadmin
ADMIN_EMAIL=admin@example.invalid
ADMIN_PASSWORD='Basic-Delivery-Admin-1!'

CI_FALLBACK_MISSING_CREDENTIALS='no web-UI credentials configured for the CI read fallback'
CI_FALLBACK_LOGIN_FAILED='forgejo web-ui login failed'

log() { printf '[run.sh] %s\n' "$*"; }
die() { printf '[run.sh] error: %s\n' "$*" >&2; exit 1; }

sleep_short() { sleep 0.2 2>/dev/null || sleep 1; }

DISPLAY_SCRIPT=${TEMPER_BASIC_DELIVERY_ORIGINAL:-$SCRIPT_DIR/run.sh}

# Dash reads long-running scripts lazily. Run starts from a private snapshot so
# source edits/rebuilds do not corrupt a sleeping launcher during teardown.
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
                       the bundled 3-role workflow, seed one site-admin intake
                       issue, launch the architect + engineer + mechanical(bot)
                       fake workers, then block until Ctrl-C or the stop-file.
  validate-webhooks    inspect logs/ for webhook registration, delivery, wakes,
                       and fake worker progress.
  stop                 tear down a previous run via run/*.pid.
  help                 show this message.

Configuration is read from config/temper.env (no secrets). This Temper example
uses deterministic fake agents and has no Smith/provider-auth dependency.
EOF
}

# --- Teardown -----------------------------------------------------------------

stop_pid() {
    _pid=$1
    [ -n "$_pid" ] || return 0
    kill -0 "$_pid" 2>/dev/null || return 0
    kill -TERM "$_pid" 2>/dev/null || true
    _i=0
    while kill -0 "$_pid" 2>/dev/null && [ "$_i" -lt 20 ]; do
        sleep_short
        _i=$((_i + 1))
    done
    kill -KILL "$_pid" 2>/dev/null || true
}

stop_pid_file() {
    _file=$1
    [ -f "$_file" ] || return 0
    while IFS= read -r _p; do
        [ -n "$_p" ] && stop_pid "$_p"
    done <"$_file"
    rm -f "$_file"
}

cleanup() {
    trap - EXIT INT TERM
    log 'tearing down...'
    [ -d "$RUN_DIR" ] && : >"$STOP_FILE" 2>/dev/null || true
    sleep 1
    stop_pid_file "$WORKERS_PID_FILE"
    stop_pid_file "$TRIGGER_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    stop_pid_file "$SERVER_PID_FILE"
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$WAKE_DIR" "$STOP_FILE" \
        "$RUN_DIR"/run.sh.snapshot.* \
        2>/dev/null || true
    rmdir "$RUN_DIR" 2>/dev/null || true
    log 'teardown complete'
}

cmd_stop() {
    [ -d "$RUN_DIR" ] || { log 'nothing to stop (no run/ dir)'; return 0; }
    cleanup
}

# --- Config -------------------------------------------------------------------

CONFIG_KNOBS="OWNER NAME WORKFLOW_FILE BASE_URL POLL_MS CI_STATUS_POLL_MS IDLE_POLL_MAX_MS RUN_SECS WEBHOOKS TRIGGER_BIND WEBHOOK_URL \
TEMPER_FORGEJO_GOMAXPROCS TEMPER_FORGEJO_BINARY TEMPER_FORGEJO_RUNNER_BINARY \
TEMPER_TESTING_WORKER_BIN TEMPER_PROVISION_BIN TEMPER_TRIGGER_BIN \
TEMPER_BUILD_PRODUCTION FAKE_CI_SENTINEL"

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

load_config() {
    [ -f "$CONFIG_DIR/temper.env" ] || die "missing $CONFIG_DIR/temper.env"
    _ci_status_poll_was_set=${CI_STATUS_POLL_MS+x}
    _pre_CI_STATUS_POLL_MS_VALUE=${CI_STATUS_POLL_MS-}
    for _k in $CONFIG_KNOBS; do
        eval "_pre_$_k=\${$_k:-}"
    done
    # shellcheck disable=SC1090
    . "$CONFIG_DIR/temper.env"
    if [ -f "$SECRETS_DIR/.env" ]; then
        # shellcheck disable=SC1090
        . "$SECRETS_DIR/.env"
    fi
    for _k in $CONFIG_KNOBS; do
        eval "_p=\${_pre_$_k}"
        [ -n "$_p" ] && eval "$_k=\$_p"
    done
    if [ -n "$_ci_status_poll_was_set" ]; then
        CI_STATUS_POLL_MS=${_pre_CI_STATUS_POLL_MS_VALUE}
    fi

    OWNER=${OWNER:-acme}
    NAME=${NAME:-service}
    WORKFLOW_FILE=${WORKFLOW_FILE:-workflow.json}
    BASE_URL=${BASE_URL:-http://127.0.0.1:4100}
    POLL_MS=${POLL_MS:-120000}
    if [ "${CI_STATUS_POLL_MS+x}" = "x" ]; then
        [ -n "$CI_STATUS_POLL_MS" ] || CI_STATUS_POLL_MS=$POLL_MS
    else
        CI_STATUS_POLL_MS=30000
    fi
    IDLE_POLL_MAX_MS=${IDLE_POLL_MAX_MS:-8000}
    RUN_SECS=${RUN_SECS:-600}
    WEBHOOKS=${WEBHOOKS:-1}
    TRIGGER_BIND=${TRIGGER_BIND:-127.0.0.1:38090}
    WEBHOOK_URL=${WEBHOOK_URL:-http://127.0.0.1:38090/forgejo/webhook}
    TEMPER_FORGEJO_GOMAXPROCS=${TEMPER_FORGEJO_GOMAXPROCS:-2}
    TEMPER_FORGEJO_BINARY=${TEMPER_FORGEJO_BINARY:-}
    TEMPER_FORGEJO_RUNNER_BINARY=${TEMPER_FORGEJO_RUNNER_BINARY:-}
    TEMPER_TESTING_WORKER_BIN=${TEMPER_TESTING_WORKER_BIN:-}
    TEMPER_PROVISION_BIN=${TEMPER_PROVISION_BIN:-}
    TEMPER_TRIGGER_BIN=${TEMPER_TRIGGER_BIN:-}
    TEMPER_BUILD_PRODUCTION=${TEMPER_BUILD_PRODUCTION:-temper}
    FAKE_CI_SENTINEL=${FAKE_CI_SENTINEL:-present}

    case "$FAKE_CI_SENTINEL" in present|deferred) ;; *) die "FAKE_CI_SENTINEL must be present or deferred" ;; esac

    # Single repo only: this example is deliberately one converging happy path.
    REPO="$OWNER/$NAME"
    validate_repo_path "$REPO"
    WORKER_REPO_ARGS="--repo $REPO"

    # Resolve the workflow file. A relative WORKFLOW_FILE is taken relative to
    # config/; an absolute path is used verbatim.
    case "$WORKFLOW_FILE" in
        /*) WORKFLOW_PATH="$WORKFLOW_FILE" ;;
        *)  WORKFLOW_PATH="$CONFIG_DIR/$WORKFLOW_FILE" ;;
    esac
    [ -f "$WORKFLOW_PATH" ] || die "workflow file not found: $WORKFLOW_PATH (set WORKFLOW_FILE in config/temper.env)"

    if [ -n "$TEMPER_FORGEJO_GOMAXPROCS" ]; then
        export GOMAXPROCS="$TEMPER_FORGEJO_GOMAXPROCS"
    fi

    _hostport=${BASE_URL#*://}
    _hostport=${_hostport%%/*}
    HOST=${_hostport%%:*}
    case "$_hostport" in
        *:*) PORT=${_hostport##*:} ;;
        *)   PORT=4100 ;;
    esac
}

# --- Binaries -----------------------------------------------------------------

resolve_binaries() {
    TESTING_WORKER_BIN=${TEMPER_TESTING_WORKER_BIN:-$WORKSPACE_ROOT/target/debug/temper-testing-worker}
    PROVISION_BIN=${TEMPER_PROVISION_BIN:-$WORKSPACE_ROOT/target/debug/temper-provision-forgejo}
    TRIGGER_BIN=${TEMPER_TRIGGER_BIN:-$WORKSPACE_ROOT/target/debug/temper-trigger-forgejo}

    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring production helper binaries are current (cargo build -p $TEMPER_BUILD_PRODUCTION)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$TEMPER_BUILD_PRODUCTION" ) || die 'cargo build failed'
        log "ensuring fake worker binary is current (cargo build -p temper-testing --bin temper-testing-worker)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p temper-testing --bin temper-testing-worker ) || die 'cargo build failed'
    fi

    [ -x "$TESTING_WORKER_BIN" ] || die "fake worker binary not found: $TESTING_WORKER_BIN"
    [ -x "$PROVISION_BIN" ] || die "provision binary not found: $PROVISION_BIN"
    [ -x "$TRIGGER_BIN" ] || die "trigger binary not found: $TRIGGER_BIN"

    # This example REQUIRES Temper's W1 (--workflow) support on both binaries and
    # the basic agent selector (--profile) on the worker. Refuse to run against a
    # stale build that lacks them.
    _worker_help=$("$TESTING_WORKER_BIN" --help 2>&1 || true)
    case "$_worker_help" in
        *--profile*--workflow* | *--workflow*--profile*) ;;
        *) die "fake worker binary is stale or incompatible: $TESTING_WORKER_BIN does not advertise --profile/--workflow. The basic-delivery example needs the basic agent selector (#62) and runtime --workflow (#63). Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p temper-testing --bin temper-testing-worker." ;;
    esac
    _provision_help=$("$PROVISION_BIN" --help 2>&1 || true)
    case "$_provision_help" in
        *--workflow*--seed-intake*) ;;
        *) die "provision binary is stale or incompatible: $PROVISION_BIN does not advertise --workflow/--seed-intake. The basic-delivery example needs Temper's runtime --workflow support so W1/W2 apply and the intake is seeded as the site admin. Re-run without TEMPER_SKIP_BUILD=1 or rebuild the entry-point package with cargo build -p $TEMPER_BUILD_PRODUCTION." ;;
    esac

    FORGEJO_BIN=${TEMPER_FORGEJO_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-$FORGEJO_VERSION-linux-amd64}
    RUNNER_BIN=${TEMPER_FORGEJO_RUNNER_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-runner-$FORGEJO_RUNNER_VERSION-linux-amd64}
    [ -x "$FORGEJO_BIN" ] || die "forgejo binary not found: $FORGEJO_BIN
       Set TEMPER_FORGEJO_BINARY, or pre-stage the pinned binary in .cache/forgejo/.
       Ignored Forgejo fixture tests fill that cache automatically on first startup."
    [ -x "$RUNNER_BIN" ] || die "forgejo-runner binary not found: $RUNNER_BIN
       Set TEMPER_FORGEJO_RUNNER_BINARY, or pre-stage the pinned binary in .cache/forgejo/.
       Ignored Forgejo fixture tests fill that cache automatically on first startup."
}

# --- Forgejo server + runner --------------------------------------------------

write_app_ini() {
    mkdir -p "$FORGEJO_DATA/custom/conf" "$FORGEJO_DATA/data" \
        "$FORGEJO_DATA/log" "$FORGEJO_DATA/repos"
    cat >"$APP_INI" <<EOF
APP_NAME = Basic Delivery Fake-Agent Example
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
        kill -0 "$SERVER_PID" 2>/dev/null || die "forgejo exited during startup (see logs/forgejo.log)"
        _i=$((_i + 1))
        [ "$_i" -gt 300 ] && die "forgejo did not become ready (see logs/forgejo.log)"
        sleep_short
    done
    log "Forgejo ready (pid $SERVER_PID)"
}

ensure_secret_file() {
    _file=$1
    [ -f "$_file" ] && return 0
    umask 077
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

wait_for_socket() {
    _socket=$1
    _pid=$2
    _label=$3
    _i=0
    while [ ! -S "$_socket" ]; do
        kill -0 "$_pid" 2>/dev/null || die "$_label exited before creating wake socket $_socket"
        _i=$((_i + 1))
        [ "$_i" -gt 100 ] && die "$_label did not create wake socket $_socket"
        sleep_short
    done
}

boot_trigger() {
    [ "$WEBHOOKS" = "1" ] || return 0
    log "starting webhook trigger at $TRIGGER_BIND ..."
    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    ensure_secret_file "$WAKE_SECRET_FILE"
    mkdir -p "$WAKE_DIR"
    : >"$LOG_DIR/trigger.log"
    "$TRIGGER_BIN" --bind "$TRIGGER_BIND" \
        --webhook-secret-file "$WEBHOOK_SECRET_FILE" \
        --wake-secret-file "$WAKE_SECRET_FILE" \
        --wake-dir "$WAKE_DIR" \
        >>"$LOG_DIR/trigger.log" 2>&1 &
    TRIGGER_PID=$!
    echo "$TRIGGER_PID" >"$TRIGGER_PID_FILE"
    wait_for_log_line "$LOG_DIR/trigger.log" 'listening on' "$TRIGGER_PID" 'webhook trigger'
    log "trigger running (pid $TRIGGER_PID; logs/trigger.log)"
}

# --- Provision + seed ---------------------------------------------------------

bootstrap_and_provision() {
    log 'bootstrapping setup-only site admin + provisioning the single repo against the bundled 3-role workflow ...'
    # Create the admin (tolerate a pre-existing one on a re-run), then mint an
    # all-scoped token. It stays in a shell variable, is never echoed, and reaches
    # the provision step only via the environment. The bundled workflow declares
    # intake_author = site_admin, so the provisioner files the seeded intake issue
    # as THIS admin; workflow automation (landing, CI reads) runs as the `bot`.
    forgejo_cli admin user create --username "$ADMIN_USER" --password "$ADMIN_PASSWORD" \
        --email "$ADMIN_EMAIL" --admin --must-change-password=false \
        >"$LOG_DIR/admin-create.log" 2>&1 || true
    ADMIN_TOKEN=$(forgejo_cli admin user generate-access-token --username "$ADMIN_USER" \
        --scopes all --raw | tr -d '[:space:]')
    [ -n "$ADMIN_TOKEN" ] || die 'failed to mint an admin access token'

    _webhook_args=
    if [ "$WEBHOOKS" = "1" ]; then
        _webhook_args="--webhook-url $WEBHOOK_URL --webhook-secret-file $WEBHOOK_SECRET_FILE"
    fi
    : >"$LOG_DIR/provision.log"

    _owner=$(repo_owner "$REPO")
    _name=$(repo_name "$REPO")
    log "provisioning $REPO (labels + CI + webhook + site-admin intake issue) ..."
    # _webhook_args intentionally word-split: POSIX sh has no arrays and the paths
    # above are controlled by this script/config. --workflow selects the bundled
    # 3-role spec (W1) whose intake_author = site_admin (W2/W3) seeds intake as the
    # admin; the default --seed-intake yes files that one issue.
    # shellcheck disable=SC2086
    _status=$(TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
        --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$ROLES_ENV" \
        --workflow "$WORKFLOW_PATH" \
        $_webhook_args) || die "provisioning $REPO failed"

    _issue=$(printf '%s\n' "$_status" | sed -n 's/.*intake issue #\([0-9][0-9]*\).*/\1/p')
    {
        printf 'repo=%s %s\n' "$REPO" "$_status"
        [ -n "$_issue" ] && printf 'repo=%s intake_issue_url=%s/%s/issues/%s\n' "$REPO" "$BASE_URL" "$REPO" "$_issue"
        if [ "$WEBHOOKS" = "1" ]; then
            printf 'repo=%s webhook registered url=%s\n' "$REPO" "$WEBHOOK_URL"
        else
            printf 'repo=%s webhook disabled\n' "$REPO"
        fi
    } >>"$LOG_DIR/provision.log"
    log "$_status"
    [ -n "$_issue" ] && log "  intake issue: $BASE_URL/$REPO/issues/$_issue"
    [ "$WEBHOOKS" = "1" ] && log "  webhook registered for $REPO ($WEBHOOK_URL)"

    [ -f "$ROLES_ENV" ] || die "provision did not write $ROLES_ENV"
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
}

# --- Workers ------------------------------------------------------------------

role_env_key() {
    printf '%s' "$1" | tr '[:lower:]' '[:upper:]' | tr -c 'A-Z0-9' '_'
}

resolve_role_identity() {
    _role=$1
    [ -f "$ROLES_ENV" ] || die "missing $ROLES_ENV"
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    _key=$(role_env_key "$_role")
    eval "ROLE_IDENTITY_USER=\${TEMPER_FORGEJO_USER_${_key}:-}"
    eval "ROLE_IDENTITY_TOKEN=\${TEMPER_FORGEJO_TOKEN_${_key}:-}"
    eval "ROLE_IDENTITY_PASSWORD=\${TEMPER_FORGEJO_PASSWORD_${_key}:-}"
}

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

launch_role_worker() {
    _role=$1
    resolve_role_identity "$_role"
    _user=$ROLE_IDENTITY_USER
    _token=$ROLE_IDENTITY_TOKEN
    _password=$ROLE_IDENTITY_PASSWORD
    [ -n "$_token" ] || die "no token for role '$_role' in $ROLES_ENV"

    _wake_args=
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/$_role.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    (
        printf 'temper-testing-worker: role=%s user=%s repositories=%s profile=basic\n' "$_role" "$_user" "$REPO"
        # shellcheck disable=SC2086
        TEMPER_FORGEJO_TOKEN="$_token" \
        TEMPER_FORGEJO_USERNAME="$_user" \
        TEMPER_FORGEJO_PASSWORD="$_password" \
            "$TESTING_WORKER_BIN" \
            --backend forgejo --base-url "$BASE_URL" $WORKER_REPO_ARGS \
            --workflow "$WORKFLOW_PATH" --profile basic \
            --root "$RUN_DIR/unused-store" --clock wall \
            --kind role --role "$_role" --user "$_user" \
            --ci-sentinel "$FAKE_CI_SENTINEL" --agents fake \
            --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" --run-secs "$RUN_SECS" \
            $_wake_args
    ) >"$LOG_DIR/$_role.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    if [ "$WEBHOOKS" = "1" ]; then
        wait_for_socket "$_wake_socket" "$_pid" "role:$_role"
    fi
    log "  role:$_role -> pid $_pid (logs/$_role.log)"
}

launch_workers() {
    : >"$WORKERS_PID_FILE"
    # Derive the role list from the provisioned secrets file (one
    # TEMPER_FORGEJO_USER_<KEY>=<role> per role binding) — never hardcoded. For the
    # basic-delivery workflow this naturally yields just architect + engineer (the
    # mechanical role has no queues and no role worker; it is serviced by the bot
    # below).
    _roles=$(sed -n 's/^TEMPER_FORGEJO_USER_\([A-Z0-9_][A-Z0-9_]*\)=.*/\1/p' "$ROLES_ENV" | tr '[:upper:]' '[:lower:]')
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"
    resolve_bot_identity

    log "mechanical automation user: $BOT_USER (landing + CI reads)"
    log 'launching basic-delivery fake-agent role workers (--profile basic) ...'
    _architect_role=
    for _r in $_roles; do
        if [ "$_r" = "architect" ]; then
            _architect_role=$_r
        else
            launch_role_worker "$_r"
        fi
    done

    # One mechanical reconciler runs the controller plane AND lands CI-green PRs as
    # the provisioned `bot` (Owners team) with its REST token plus web-UI
    # credentials for the ADR-0019 CI read fallback. With no reviewer/owner the bot
    # is the SOLE landing authority: it merges once the CI gate is green.
    _wake_args=
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/mechanical.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    (
        printf 'temper-testing-worker: mechanical repositories=%s automation_user=%s ci_reader=bot idle_poll_max_ms=%s\n' "$REPO" "$BOT_USER" "$IDLE_POLL_MAX_MS"
        # shellcheck disable=SC2086
        TEMPER_FORGEJO_TOKEN="$BOT_TOKEN" \
        TEMPER_FORGEJO_USERNAME="$BOT_USER" \
        TEMPER_FORGEJO_PASSWORD="$BOT_PASSWORD" \
            "$TESTING_WORKER_BIN" \
            --backend forgejo --base-url "$BASE_URL" $WORKER_REPO_ARGS \
            --workflow "$WORKFLOW_PATH" \
            --root "$RUN_DIR/unused-store" --clock wall \
            --kind mechanical \
            --poll-ms "$CI_STATUS_POLL_MS" --idle-poll-max-ms "$IDLE_POLL_MAX_MS" \
            --stop-file "$STOP_FILE" --run-secs "$RUN_SECS" \
            $_wake_args
    ) >"$LOG_DIR/mechanical.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    if [ "$WEBHOOKS" = "1" ]; then
        wait_for_socket "$_wake_socket" "$_pid" 'mechanical'
    fi
    log "  mechanical -> pid $_pid as bot $BOT_USER (logs/mechanical.log)"

    if [ -n "$_architect_role" ]; then
        launch_role_worker "$_architect_role"
    fi
}

# --- Validation ---------------------------------------------------------------

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

validate_mechanical_bot_config() {
    _ok=0
    if [ ! -f "$ROLES_ENV" ]; then
        log "missing: $ROLES_ENV not found; cannot confirm bot automation credentials"
        log 'diagnosis: Forgejo 7.0.x CI reads need web-UI credentials for the mechanical landing worker (ADR 0019)'
        return 1
    fi
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    if [ "${TEMPER_FORGEJO_BOT_USER:-}" = "bot" ] && [ -n "${TEMPER_FORGEJO_BOT_TOKEN:-}" ] \
        && [ -n "${TEMPER_FORGEJO_BOT_PASSWORD:-}" ]; then
        log 'ok: bot automation token + web-UI credentials present for the mechanical worker'
    else
        log "missing: bot automation user token/username/password in $ROLES_ENV"
        log 'diagnosis: provision the bot user and launch mechanical with its REST token plus TEMPER_FORGEJO_USERNAME/TEMPER_FORGEJO_PASSWORD for landing and the ADR-0019 CI read fallback'
        _ok=1
    fi
    return "$_ok"
}

validate_mechanical_ci_log() {
    _ok=0
    _mechanical_log="$LOG_DIR/mechanical.log"
    if [ ! -f "$_mechanical_log" ]; then
        log 'missing: logs/mechanical.log exists for mechanical CI-read diagnostics'
        return 1
    fi
    if ! grep -F -q 'ci_reader=bot' "$_mechanical_log" 2>/dev/null; then
        log 'missing: mechanical worker startup did not record the non-secret bot automation identity'
        log 'diagnosis: restart with the updated launcher so mechanical runs as the bot user for landing and CI reads'
        _ok=1
    fi
    if grep -F -q "$CI_FALLBACK_MISSING_CREDENTIALS" "$_mechanical_log" 2>/dev/null; then
        log 'missing: mechanical worker reported missing Forgejo web-UI credentials for CI reads'
        log 'diagnosis: the landing queue needs native CI; pass the bot TEMPER_FORGEJO_USERNAME/TEMPER_FORGEJO_PASSWORD to the mechanical worker (ADR 0019)'
        _ok=1
    fi
    if grep -F -q "$CI_FALLBACK_LOGIN_FAILED" "$_mechanical_log" 2>/dev/null; then
        log 'missing: mechanical worker could not log in to Forgejo web UI for CI reads'
        log 'diagnosis: verify the bot automation credentials in secrets/roles.env; the mechanical worker uses the Owners-team bot (not the setup-only admin) for landing and Forgejo 7.0.x Actions reads'
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
    _trigger_log="$LOG_DIR/trigger.log"
    _provision_log="$LOG_DIR/provision.log"

    [ -d "$LOG_DIR" ] || die "no logs/ directory yet; start a run first"
    log "validating webhook wake logs under $LOG_DIR"
    log "configured repo: $REPO"
    log "configured POLL_MS=$POLL_MS CI_STATUS_POLL_MS=$CI_STATUS_POLL_MS IDLE_POLL_MAX_MS=$IDLE_POLL_MAX_MS; long-poll smoke expects POLL_MS=120000"

    validate_mechanical_bot_config || _ok=1
    validate_mechanical_ci_log || _ok=1

    validate_contains "$_provision_log" 'webhook registered url=' \
        'repo webhook registration recorded' || _ok=1
    validate_contains "$_provision_log" 'intake_issue_url=' \
        'site-admin intake issue URL recorded' || _ok=1
    validate_contains "$_trigger_log" 'listening on' \
        'trigger reached listening readiness' || _ok=1
    validate_contains "$_trigger_log" 'webhook accepted' \
        'Forgejo delivered at least one accepted webhook' || _ok=1
    validate_contains "$_trigger_log" 'wake_delivery outcome=sent' \
        'trigger found sockets and sent at least one wake batch' || _ok=1

    _accepted=$(count_matches 'webhook accepted' "$_trigger_log")
    _sent=$(count_matches 'wake_delivery outcome=sent' "$_trigger_log")
    _no_sockets=$(count_matches 'wake_delivery outcome=no_sockets' "$_trigger_log")
    _failed=$(count_matches 'wake_send_failed' "$_trigger_log")
    log "trigger summary: accepted=$_accepted sent_batches=$_sent no_socket_batches=$_no_sockets send_failures=$_failed"

    _workers=0
    _consumed=0
    _ticks=0
    _progress=0
    _no_work=0
    for _log in "$LOG_DIR"/*.log; do
        [ -f "$_log" ] || continue
        grep -q 'temper-testing-worker:' "$_log" 2>/dev/null || continue
        _workers=$((_workers + 1))
        _name=${_log##*/}
        if grep -q 'consumed authenticated wake' "$_log" 2>/dev/null; then
            _consumed=$((_consumed + 1))
            _consumed_text=yes
        else
            _consumed_text=no
            _ok=1
        fi
        if grep -E -q 'completed tick .*actions=' "$_log" 2>/dev/null; then
            _ticks=$((_ticks + 1))
            _tick_text=yes
        else
            _tick_text=no
            _ok=1
        fi
        if grep -E -q 'completed tick .*actions=[1-9][0-9]*' "$_log" 2>/dev/null; then
            _progress=$((_progress + 1))
        fi
        if grep -E -q 'completed tick .*actions=0' "$_log" 2>/dev/null; then
            _no_work=$((_no_work + 1))
        fi
        log "worker $_name: consumed_wake=$_consumed_text tick=$_tick_text"
    done

    if [ "$_workers" -eq 0 ]; then
        log 'missing: no temper-testing-worker logs found'
        _ok=1
    fi
    if [ "$_progress" -eq 0 ]; then
        log 'missing: no fake worker tick reported actions>0'
        _ok=1
    fi
    log "worker summary: workers=$_workers consumed=$_consumed ticks=$_ticks progress=$_progress no_work=$_no_work"

    if [ "$_ok" -eq 0 ]; then
        log 'webhook wake validation passed'
    else
        log 'webhook wake validation failed; inspect logs/provision.log, logs/trigger.log, and worker logs'
    fi
    return "$_ok"
}

# --- Monitor ------------------------------------------------------------------

monitor() {
    log ''
    log "Forgejo UI:    $BASE_URL  (log in as any provisioned role)"
    log "Worker pool:   architect + engineer + mechanical(bot) scan: $REPO"
    log "Intake issue:  $BASE_URL/$REPO/issues"
    log "Worker logs:   $LOG_DIR/"
    log 'The bot stamps the site-admin intake untriaged; the architect triages it'
    log 'to a ready code issue, the engineer opens an implementation PR, CI runs'
    log 'and goes green, and the bot auto-merges it — no reviewer, owner, or human.'
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

    trap cleanup EXIT INT TERM

    boot_server
    boot_runner
    boot_trigger
    bootstrap_and_provision
    launch_workers
    monitor
}

case "${1:-start}" in
    start | "") cmd_start ;;
    validate-webhooks | smoke-webhooks) cmd_validate_webhooks ;;
    stop) cmd_stop ;;
    help | -h | --help) usage ;;
    *) usage >&2; exit 2 ;;
esac
