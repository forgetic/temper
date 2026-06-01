#!/bin/sh
# Reference-delivery example — POSIX launcher / teardown.
#
# Boots EVERY process of the production topology from compiled binaries:
#   1. a throwaway Forgejo server (SQLite, Actions enabled),
#   2. a host-mode forgejo-runner producing real CI,
#   3. admin bootstrap + the production provision/seed binary
#      (org/users/tokens/repo/labels/CI workflow + one intake issue),
#   4. one harness-worker per workflow role-with-an-agent, plus one mechanical
#      reconciler — all against Forgejo with real LLM agents and wall time,
# then tears them all down cleanly on Ctrl-C / signal / `./run.sh stop`.
#
# This script now targets the planned production binaries from
# plans/production-binaries/README.md instead of the harness-testing entry
# points. Until that plan lands, it is forward-looking wiring rather than a
# runnable demo from a clean checkout.
#
# Usage:
#   ./run.sh [start]   boot everything and block until Ctrl-C / stop-file
#   ./run.sh stop      tear down a previous run via the saved PIDs
#   ./run.sh help      show this usage
#
# Orphan cleanup (lesson 0009) — if a run is force-killed (SIGKILL) the Drop/
# trap guards do not fire; clean up survivors by hand with:
#       pkill -f forgejo
#       pkill -f forgejo-runner
#       pkill -f harness-worker
#       pkill -f harness-trigger-forgejo
#       rm -rf examples/reference-delivery/run
#
# POSIX sh only (no bashisms). Validate with `sh -n run.sh` (and shellcheck).
# Secrets travel by env or the sourced secrets files, NEVER on a command line.

set -eu

# --- Locations ----------------------------------------------------------------
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
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

# Pinned versions for the bundled throwaway server/runner used by this example.
FORGEJO_VERSION=7.0.12
FORGEJO_RUNNER_VERSION=3.5.1

# Throwaway admin identity (this server is killed + wiped on teardown; never a
# credential that reaches anything real, and never echoed).
ADMIN_USER=refadmin
ADMIN_EMAIL=refadmin@example.invalid
ADMIN_PASSWORD='Ref-Delivery-Admin-1!'

log() { printf '[run.sh] %s\n' "$*"; }
die() { printf '[run.sh] error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<EOF
usage: $0 [start|stop|help]

  start (default)  boot Forgejo + runner, provision + seed, launch the workers,
                   then block until Ctrl-C or the stop-file.
  stop             tear down a previous run via run/*.pid.
  help             show this message.

Configuration is read from config/harness.env (no secrets). Auth selection is
HARNESS_AGENTS_AUTH (default chatgpt-oauth); see secrets/.env.example.
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

# Tears down workers, runner, and server (in that order) and clears run state.
# Idempotent: safe to call from the EXIT trap and from `./run.sh stop`.
cleanup() {
    trap - EXIT INT TERM
    log 'tearing down...'
    # Signal the workers to stop cooperatively first, then hard-stop survivors.
    [ -d "$RUN_DIR" ] && : >"$STOP_FILE" 2>/dev/null || true
    sleep 1
    stop_pid_file "$WORKERS_PID_FILE"
    stop_pid_file "$TRIGGER_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    stop_pid_file "$SERVER_PID_FILE"
    # Drop the throwaway server/runner data + sentinel so a re-run starts fresh;
    # keep logs/ for inspection.
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$WAKE_DIR" "$STOP_FILE" 2>/dev/null || true
    log 'teardown complete'
}

cmd_stop() {
    [ -d "$RUN_DIR" ] || { log 'nothing to stop (no run/ dir)'; return 0; }
    cleanup
}

# --- Config + secrets ---------------------------------------------------------

# Config knobs whose pre-existing environment value should win over the file
# (precedence: CLI/env > config/harness.env > built-in default). The file is
# the operator's edited config; a `VAR=x ./run.sh` still overrides it.
CONFIG_KNOBS="OWNER NAME BASE_URL POLL_MS RUN_SECS WEBHOOKS TRIGGER_BIND WEBHOOK_URL \
HARNESS_AGENTS_AUTH HARNESS_AGENTS_CODEX_MODEL HARNESS_AGENTS_ANTHROPIC_MODEL \
HARNESS_AGENTS_AUTH_FILE HARNESS_FORGEJO_GOMAXPROCS HARNESS_FORGEJO_BINARY \
HARNESS_FORGEJO_RUNNER_BINARY HARNESS_WORKER_BIN HARNESS_PROVISION_BIN \
HARNESS_TRIGGER_BIN HARNESS_BUILD_PACKAGE"

load_config() {
    [ -f "$CONFIG_DIR/harness.env" ] || die "missing $CONFIG_DIR/harness.env"
    # Snapshot any pre-existing env values so they survive the file sourcing.
    for _k in $CONFIG_KNOBS; do
        eval "_pre_$_k=\${$_k:-}"
    done
    # shellcheck disable=SC1090
    . "$CONFIG_DIR/harness.env"
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
    BASE_URL=${BASE_URL:-http://127.0.0.1:3000}
    POLL_MS=${POLL_MS:-2000}
    RUN_SECS=${RUN_SECS:-600}
    WEBHOOKS=${WEBHOOKS:-1}
    TRIGGER_BIND=${TRIGGER_BIND:-127.0.0.1:38080}
    WEBHOOK_URL=${WEBHOOK_URL:-http://127.0.0.1:38080/forgejo/webhook}
    HARNESS_AGENTS_AUTH=${HARNESS_AGENTS_AUTH:-chatgpt-oauth}
    HARNESS_AGENTS_CODEX_MODEL=${HARNESS_AGENTS_CODEX_MODEL:-}
    HARNESS_AGENTS_ANTHROPIC_MODEL=${HARNESS_AGENTS_ANTHROPIC_MODEL:-}
    HARNESS_AGENTS_AUTH_FILE=${HARNESS_AGENTS_AUTH_FILE:-}
    HARNESS_FORGEJO_GOMAXPROCS=${HARNESS_FORGEJO_GOMAXPROCS:-2}
    HARNESS_FORGEJO_BINARY=${HARNESS_FORGEJO_BINARY:-}
    HARNESS_FORGEJO_RUNNER_BINARY=${HARNESS_FORGEJO_RUNNER_BINARY:-}
    HARNESS_WORKER_BIN=${HARNESS_WORKER_BIN:-}
    HARNESS_PROVISION_BIN=${HARNESS_PROVISION_BIN:-}
    HARNESS_TRIGGER_BIN=${HARNESS_TRIGGER_BIN:-}
    HARNESS_BUILD_PACKAGE=${HARNESS_BUILD_PACKAGE:-harness-production}

    # Cap the Go runtime of the spawned forgejo + forgejo-runner (lesson 0009).
    # Exported so both Go processes inherit it; harmless for the Rust workers.
    if [ -n "$HARNESS_FORGEJO_GOMAXPROCS" ]; then
        export GOMAXPROCS="$HARNESS_FORGEJO_GOMAXPROCS"
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

# Validates the selected auth's input and sets CLI flag fragments. ChatGPT OAuth
# (default) and Anthropic OAuth read the shared pi auth.json in place; DeepSeek
# reads a key file.
# Fails fast with a clear message when the selected auth's input is missing.
CODEX_MODEL_ARG=
AUTH_FILE_ARG=
check_auth() {
    case "$HARNESS_AGENTS_AUTH" in
        chatgpt-oauth)
            _auth_file=${HARNESS_AGENTS_AUTH_FILE:-$HOME/.pi/agent/auth.json}
            if [ ! -f "$_auth_file" ]; then
                die "ChatGPT OAuth selected but $_auth_file is missing.
       Log in once:  pi /login openai-codex
       (or set HARNESS_AGENTS_AUTH=deepseek to use a DeepSeek key instead)"
            fi
            if ! grep -q 'openai-codex' "$_auth_file" 2>/dev/null; then
                die "no 'openai-codex' entry in $_auth_file.
       Log in once:  pi /login openai-codex"
            fi
            AUTH_FLAG=chatgpt-oauth
            [ -n "$HARNESS_AGENTS_CODEX_MODEL" ] && CODEX_MODEL_ARG="--codex-model $HARNESS_AGENTS_CODEX_MODEL"
            [ -n "$HARNESS_AGENTS_AUTH_FILE" ] && AUTH_FILE_ARG="--auth-file $HARNESS_AGENTS_AUTH_FILE"
            log "auth: ChatGPT OAuth (reads $_auth_file)"
            ;;
        anthropic-oauth)
            _auth_file=${HARNESS_AGENTS_AUTH_FILE:-$HOME/.pi/agent/auth.json}
            if [ ! -f "$_auth_file" ]; then
                die "Anthropic OAuth selected but $_auth_file is missing.
       Log in once:  pi /login anthropic"
            fi
            if ! grep -q '"anthropic"' "$_auth_file" 2>/dev/null; then
                die "no 'anthropic' entry in $_auth_file.
       Log in once:  pi /login anthropic"
            fi
            AUTH_FLAG=anthropic-oauth
            [ -n "$HARNESS_AGENTS_AUTH_FILE" ] && AUTH_FILE_ARG="--auth-file $HARNESS_AGENTS_AUTH_FILE"
            # Anthropic model selection is env-only in the worker/provider seam.
            [ -n "$HARNESS_AGENTS_ANTHROPIC_MODEL" ] && export HARNESS_AGENTS_ANTHROPIC_MODEL
            log "auth: Anthropic OAuth (reads $_auth_file; model ${HARNESS_AGENTS_ANTHROPIC_MODEL:-claude-opus-4-8})"
            ;;
        deepseek)
            _key_file="$SECRETS_DIR/deepseek-api-key"
            [ -f "$_key_file" ] || die "DeepSeek selected but $_key_file is missing.
       Create it with your key (see secrets/deepseek-api-key.example),
       or set HARNESS_AGENTS_AUTH=chatgpt-oauth to use a ChatGPT login."
            # Exported so every worker child resolves the key from the file;
            # the key value never appears on argv.
            export HARNESS_DEEPSEEK_API_KEY_PATH="$_key_file"
            AUTH_FLAG=deepseek
            log "auth: DeepSeek (key file $_key_file)"
            ;;
        *)
            die "unknown HARNESS_AGENTS_AUTH '$HARNESS_AGENTS_AUTH' (expected chatgpt-oauth|deepseek|anthropic-oauth)"
            ;;
    esac
}

# --- Binaries -----------------------------------------------------------------

resolve_binaries() {
    WORKER_BIN=${HARNESS_WORKER_BIN:-$WORKSPACE_ROOT/target/release/harness-worker}
    PROVISION_BIN=${HARNESS_PROVISION_BIN:-$WORKSPACE_ROOT/target/release/harness-provision-forgejo}
    TRIGGER_BIN=${HARNESS_TRIGGER_BIN:-$WORKSPACE_ROOT/target/release/harness-trigger-forgejo}
    if [ ! -x "$WORKER_BIN" ] || [ ! -x "$PROVISION_BIN" ] || [ ! -x "$TRIGGER_BIN" ]; then
        if [ "${HARNESS_SKIP_BUILD:-0}" = "1" ]; then
            die "production binaries missing under target/release and HARNESS_SKIP_BUILD=1"
        fi
        log "building production binaries (cargo build --release -p $HARNESS_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build --release -p "$HARNESS_BUILD_PACKAGE" ) \
            || die 'cargo build failed'
    fi
    [ -x "$WORKER_BIN" ] || die "worker binary not found: $WORKER_BIN"
    [ -x "$PROVISION_BIN" ] || die "provision binary not found: $PROVISION_BIN"
    [ -x "$TRIGGER_BIN" ] || die "trigger binary not found: $TRIGGER_BIN"

    # Pinned Forgejo + runner: env override, else the cached pinned path.
    FORGEJO_BIN=${HARNESS_FORGEJO_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-$FORGEJO_VERSION-linux-amd64}
    RUNNER_BIN=${HARNESS_FORGEJO_RUNNER_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-runner-$FORGEJO_RUNNER_VERSION-linux-amd64}
    [ -x "$FORGEJO_BIN" ] || die "forgejo binary not found: $FORGEJO_BIN
       Set HARNESS_FORGEJO_BINARY, or pre-stage the pinned binary in .cache/forgejo/
       (running the gated forgejo_multiprocess test once downloads + checksums it)."
    [ -x "$RUNNER_BIN" ] || die "forgejo-runner binary not found: $RUNNER_BIN
       Set HARNESS_FORGEJO_RUNNER_BINARY, or pre-stage the pinned binary in .cache/forgejo/."
}

# --- Forgejo server -----------------------------------------------------------

write_app_ini() {
    mkdir -p "$FORGEJO_DATA/custom/conf" "$FORGEJO_DATA/data" \
        "$FORGEJO_DATA/log" "$FORGEJO_DATA/repos"
    cat >"$APP_INI" <<EOF
APP_NAME = Reference Delivery Example
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
SECRET_KEY = reference-delivery-example-not-for-production
INTERNAL_TOKEN = reference-delivery-example-internal-not-for-production

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
        --name "ref-delivery-$$" --labels host:host ) \
        >"$LOG_DIR/runner-register.log" 2>&1 \
        || die 'forgejo-runner register failed (see logs/runner-register.log)'

    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" daemon ) >"$LOG_DIR/runner.log" 2>&1 &
    RUNNER_PID=$!
    echo "$RUNNER_PID" >"$RUNNER_PID_FILE"
    log "runner daemon running (pid $RUNNER_PID)"
}

boot_trigger() {
    [ "$WEBHOOKS" = "1" ] || return 0
    log "starting webhook trigger at $TRIGGER_BIND ..."
    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    ensure_secret_file "$WAKE_SECRET_FILE"
    mkdir -p "$WAKE_DIR"
    "$TRIGGER_BIN" --bind "$TRIGGER_BIND" \
        --webhook-secret-file "$WEBHOOK_SECRET_FILE" \
        --wake-secret-file "$WAKE_SECRET_FILE" \
        --wake-dir "$WAKE_DIR" \
        >"$LOG_DIR/trigger.log" 2>&1 &
    TRIGGER_PID=$!
    echo "$TRIGGER_PID" >"$TRIGGER_PID_FILE"
    log "trigger running (pid $TRIGGER_PID; logs/trigger.log)"
}

# --- Provision + seed ---------------------------------------------------------

bootstrap_and_provision() {
    log 'bootstrapping admin + provisioning (org/users/tokens/repo/labels/CI/issue) ...'
    # Create the admin (tolerate a pre-existing one on a re-run), then mint an
    # all-scoped token. The token stays in a shell variable; it is never echoed
    # and reaches the provision step only via the environment.
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
    # _webhook_args intentionally word-split: POSIX sh has no arrays and the
    # example paths above are controlled by this script/config.
    # shellcheck disable=SC2086
    _status=$(HARNESS_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
        --base-url "$BASE_URL" --owner "$OWNER" --name "$NAME" --out "$ROLES_ENV" \
        $_webhook_args) \
        || die 'provisioning failed'
    log "$_status"

    [ -f "$ROLES_ENV" ] || die "provision did not write $ROLES_ENV"
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
}

# --- Workers ------------------------------------------------------------------

# Uppercases a role id and replaces non-alphanumerics with `_` (matching the
# provision binary's env_role_key), yielding the secrets-file variable suffix.
role_env_key() {
    printf '%s' "$1" | tr '[:lower:]' '[:upper:]' | tr -c 'A-Z0-9' '_'
}

launch_role_worker() {
    _role=$1
    _key=$(role_env_key "$_role")
    eval "_user=\${HARNESS_FORGEJO_USER_${_key}:-}"
    eval "_token=\${HARNESS_FORGEJO_TOKEN_${_key}:-}"
    eval "_password=\${HARNESS_FORGEJO_PASSWORD_${_key}:-}"
    [ -n "$_token" ] || die "no token for role '$_role' in $ROLES_ENV"

    _wake_args=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_args="--wake-socket $WAKE_DIR/$_role.sock --wake-secret-file $WAKE_SECRET_FILE"
    fi

    # Per-role secrets are literal env-assignment prefixes (never on argv). The
    # auth-mode env (DeepSeek key path) is exported globally by check_auth.
    # CODEX_MODEL_ARG / AUTH_FILE_ARG / _wake_args intentionally word-split
    # (POSIX has no arrays); they are empty unless configured.
    # shellcheck disable=SC2086
    HARNESS_FORGEJO_TOKEN="$_token" \
    HARNESS_FORGEJO_USERNAME="$_user" \
    HARNESS_FORGEJO_PASSWORD="$_password" \
        "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" --repo "$OWNER/$NAME" \
        --kind role --role "$_role" --user "$_user" \
        --auth "$AUTH_FLAG" $CODEX_MODEL_ARG $AUTH_FILE_ARG \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" --run-secs "$RUN_SECS" \
        $_wake_args \
        >"$LOG_DIR/$_role.log" 2>&1 &
    echo "$!" >>"$WORKERS_PID_FILE"
    log "  role:$_role -> pid $! (logs/$_role.log)"
}

launch_workers() {
    : >"$WORKERS_PID_FILE"
    # Derive the role list from the provisioned secrets file (one HARNESS_FORGEJO_
    # USER_<KEY>=<role> per role binding) — never hardcoded. The value is both the
    # role id and the user handle (the Forgejo id==handle requirement).
    _roles=$(sed -n "s/^HARNESS_FORGEJO_USER_[A-Z0-9_]*='\(.*\)'\$/\1/p" "$ROLES_ENV")
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"

    log 'launching role workers (production binary, real agents) ...'
    for _r in $_roles; do
        launch_role_worker "$_r"
    done

    # One mechanical reconciler (controller plane; admin token, no agent).
    _wake_args=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_args="--wake-socket $WAKE_DIR/mechanical.sock --wake-secret-file $WAKE_SECRET_FILE"
    fi
    # shellcheck disable=SC2086
    HARNESS_FORGEJO_TOKEN="$ADMIN_TOKEN" "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" --repo "$OWNER/$NAME" \
        --kind mechanical \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" --run-secs "$RUN_SECS" \
        $_wake_args \
        >"$LOG_DIR/mechanical.log" 2>&1 &
    echo "$!" >>"$WORKERS_PID_FILE"
    log "  mechanical -> pid $! (logs/mechanical.log)"
}

# --- Monitor ------------------------------------------------------------------

# Blocks until the stop-file appears, the server dies, or RUN_SECS elapses, so
# the EXIT/INT/TERM trap can tear everything down on Ctrl-C.
monitor() {
    log ''
    log "Forgejo UI:    $BASE_URL  (log in as any provisioned role)"
    log "Repo + issues: $BASE_URL/$OWNER/$NAME/issues"
    log "Worker logs:   $LOG_DIR/"
    log 'Watch the intake issue get triaged, a PR open, CI run, the review land,'
    log 'and the merge + reconcile labels move — all in the Forgejo UI above.'
    log ''
    log "Press Ctrl-C (or run '$0 stop') to tear everything down."

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
    check_auth
    resolve_binaries

    if [ -f "$SERVER_PID_FILE" ] && kill -0 "$(cat "$SERVER_PID_FILE" 2>/dev/null)" 2>/dev/null; then
        die "a run appears active (run/server.pid). Stop it first: $0 stop"
    fi

    mkdir -p "$RUN_DIR" "$LOG_DIR"
    rm -f "$STOP_FILE"

    # From here on, tear everything down on any exit/interrupt.
    trap cleanup EXIT INT TERM

    boot_server
    boot_runner
    boot_trigger
    bootstrap_and_provision
    launch_workers
    monitor
    # cleanup runs via the EXIT trap.
}

# --- Dispatch -----------------------------------------------------------------

case "${1:-start}" in
    start | "") cmd_start ;;
    stop) cmd_stop ;;
    help | -h | --help) usage ;;
    *)
        usage >&2
        exit 2
        ;;
esac
