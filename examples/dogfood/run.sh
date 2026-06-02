#!/bin/sh
# Dogfood Harness against the live Forgejo repository at git.ekanayaka.io.
# Secrets are read from ~/Documents/personal/forgejo-rhi and emitted only into
# gitignored examples/dogfood/secrets/roles.env. Tokens/passwords travel via env,
# never on argv, except the one-time admin basic-auth fallback that mints a token.

set -eu

if [ -n "${HARNESS_DOGFOOD_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$HARNESS_DOGFOOD_SCRIPT_DIR
else
    SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fi
WORKSPACE_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
CONFIG_FILE="$SCRIPT_DIR/config/dogfood.env"
RUN_DIR="$SCRIPT_DIR/run"
LOG_DIR="$SCRIPT_DIR/logs"
SECRETS_DIR="$SCRIPT_DIR/secrets"
TOOLS_DIR="$SCRIPT_DIR/tools"
ROLES_ENV="$SECRETS_DIR/roles.env"
WEBHOOK_SECRET_FILE="$SECRETS_DIR/webhook-secret"
WAKE_SECRET_FILE="$SECRETS_DIR/wake-secret"
STOP_FILE="$RUN_DIR/stop"
WORKERS_PID_FILE="$RUN_DIR/workers.pids"
TRIGGER_PID_FILE="$RUN_DIR/trigger.pid"
TUNNEL_PID_FILE="$RUN_DIR/ssh-tunnel.pid"
RUNNER_PID_FILE="$RUN_DIR/runner.pid"
RUNNER_DIR="$RUN_DIR/forgejo-runner"
WAKE_DIR="$RUN_DIR/wake"

log() { printf '[dogfood] %s\n' "$*"; }
die() { printf '[dogfood] error: %s\n' "$*" >&2; exit 1; }

DISPLAY_SCRIPT=${HARNESS_DOGFOOD_ORIGINAL:-$SCRIPT_DIR/run.sh}

# Snapshot long-running starts so edits to this ignored script cannot corrupt a
# running teardown path (same rationale as examples/reference-delivery).
if [ "${HARNESS_DOGFOOD_SNAPSHOT:-0}" != "1" ]; then
    case "${1:-start}" in
        start | "")
            mkdir -p "$RUN_DIR"
            _snapshot="$RUN_DIR/run.sh.snapshot.$$"
            cp "$SCRIPT_DIR/run.sh" "$_snapshot"
            chmod 700 "$_snapshot"
            HARNESS_DOGFOOD_SNAPSHOT=1 \
            HARNESS_DOGFOOD_SCRIPT_DIR="$SCRIPT_DIR" \
            HARNESS_DOGFOOD_ORIGINAL="$DISPLAY_SCRIPT" \
                exec /bin/sh "$_snapshot" "$@"
            ;;
    esac
fi

usage() {
    cat <<EOF
usage: $DISPLAY_SCRIPT [start|stop|status|help]

  start (default)  parse live credentials, register/refresh the webhook, open an
                   ssh reverse tunnel through 'rhi', launch trigger + workers,
                   then block until Ctrl-C.
  stop             stop local workers, trigger, and ssh tunnel from run/*.pid.
  status           show local process/log locations.
  help             show this message.

File new work in: ${BASE_URL:-https://git.ekanayaka.io}/${REPO:-ai/harness}/issues
EOF
}

stop_pid() {
    _pid=$1
    [ -n "$_pid" ] || return 0
    kill -0 "$_pid" 2>/dev/null || return 0
    kill -TERM "$_pid" 2>/dev/null || true
    _i=0
    while kill -0 "$_pid" 2>/dev/null && [ "$_i" -lt 25 ]; do
        sleep 0.2 2>/dev/null || sleep 1
        _i=$((_i + 1))
    done
    kill -KILL "$_pid" 2>/dev/null || true
}

stop_pid_file() {
    _file=$1
    [ -f "$_file" ] || return 0
    while IFS= read -r _pid; do
        [ -n "$_pid" ] && stop_pid "$_pid"
    done <"$_file"
    rm -f "$_file"
}

cleanup() {
    trap - EXIT INT TERM
    log 'tearing down local dogfood processes...'
    [ -d "$RUN_DIR" ] && : >"$STOP_FILE" 2>/dev/null || true
    sleep 1
    stop_pid_file "$WORKERS_PID_FILE"
    stop_pid_file "$TRIGGER_PID_FILE"
    stop_pid_file "$TUNNEL_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    if [ -n "${SSH_HOST:-}" ] && [ -n "${REMOTE_WEBHOOK_HOST:-}" ] && [ -n "${REMOTE_WEBHOOK_PORT:-}" ] && [ -n "${LOCAL_TRIGGER_PORT:-}" ]; then
        ssh -O cancel -R "$REMOTE_WEBHOOK_HOST:$REMOTE_WEBHOOK_PORT:127.0.0.1:$LOCAL_TRIGGER_PORT" "$SSH_HOST" >/dev/null 2>&1 || true
    fi
    rm -rf "$WAKE_DIR" "$STOP_FILE" "$RUN_DIR"/run.sh.snapshot.* 2>/dev/null || true
    rmdir "$RUN_DIR" 2>/dev/null || true
    log 'teardown complete (remote webhook is left registered for the next run)'
}

cmd_stop() {
    [ -d "$RUN_DIR" ] || { log 'nothing to stop (no run/ dir)'; return 0; }
    cleanup
}

load_config() {
    [ -f "$CONFIG_FILE" ] || die "missing $CONFIG_FILE"
    # shellcheck disable=SC1090
    . "$CONFIG_FILE"

    BASE_URL=${BASE_URL:-https://git.ekanayaka.io}
    REPO=${REPO:-ai/harness}
    SECRETS_SOURCE=${SECRETS_SOURCE:-$HOME/Documents/personal/forgejo-rhi}
    SSH_HOST=${SSH_HOST:-rhi}
    LOCAL_TRIGGER_BIND=${LOCAL_TRIGGER_BIND:-127.0.0.1:39080}
    REMOTE_WEBHOOK_HOST=${REMOTE_WEBHOOK_HOST:-127.0.0.1}
    REMOTE_WEBHOOK_PORT=${REMOTE_WEBHOOK_PORT:-39080}
    WEBHOOK_URL=${WEBHOOK_URL:-http://127.0.0.1:39080/forgejo/webhook}
    WEBHOOKS=${WEBHOOKS:-1}
    POLL_MS=${POLL_MS:-120000}
    HARNESS_AGENTS_AUTH=${HARNESS_AGENTS_AUTH:-chatgpt-oauth}
    HARNESS_AGENTS_CODEX_MODEL=${HARNESS_AGENTS_CODEX_MODEL:-}
    HARNESS_AGENTS_AUTH_FILE=${HARNESS_AGENTS_AUTH_FILE:-}
    HARNESS_AGENTS_ANTHROPIC_MODEL=${HARNESS_AGENTS_ANTHROPIC_MODEL:-}
    DOGFOOD_HUMAN_USER=${DOGFOOD_HUMAN_USER:-bot}
    DOGFOOD_MECHANICAL_USER=${DOGFOOD_MECHANICAL_USER:-bot}
    DOGFOOD_PRODUCT_MANAGER_USER=${DOGFOOD_PRODUCT_MANAGER_USER:-product-manager}
    DOGFOOD_REPO_PERMISSION=${DOGFOOD_REPO_PERMISSION:-write}
    HARNESS_WORKER_BIN=${HARNESS_WORKER_BIN:-}
    HARNESS_TRIGGER_BIN=${HARNESS_TRIGGER_BIN:-}
    HARNESS_BUILD_PACKAGE=${HARNESS_BUILD_PACKAGE:-harness-production}
    HARNESS_FORGEJO_RUNNER_BINARY=${HARNESS_FORGEJO_RUNNER_BINARY:-}
    DOGFOOD_RUNNER=${DOGFOOD_RUNNER:-1}
    DOGFOOD_DEFAULT_BRANCH=${DOGFOOD_DEFAULT_BRANCH:-main}
    DOGFOOD_REMOTE_FORGEJO_BIN=${DOGFOOD_REMOTE_FORGEJO_BIN:-/opt/forgejo/forgejo}
    DOGFOOD_REMOTE_FORGEJO_WORK_PATH=${DOGFOOD_REMOTE_FORGEJO_WORK_PATH:-/srv/data/git/forgejo}

    case "$REPO" in
        */*) OWNER=${REPO%%/*}; NAME=${REPO#*/} ;;
        *) die "REPO must be owner/name, got '$REPO'" ;;
    esac
    [ -n "$OWNER" ] && [ -n "$NAME" ] || die "REPO must be owner/name, got '$REPO'"
    LOCAL_TRIGGER_PORT=${LOCAL_TRIGGER_BIND##*:}
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

resolve_binaries() {
    WORKER_BIN=${HARNESS_WORKER_BIN:-$WORKSPACE_ROOT/target/debug/harness-worker}
    TRIGGER_BIN=${HARNESS_TRIGGER_BIN:-$WORKSPACE_ROOT/target/debug/harness-trigger-forgejo}
    RUNNER_BIN=${HARNESS_FORGEJO_RUNNER_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-runner-3.5.1-linux-amd64}
    if [ "${HARNESS_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring development-profile binaries are current (cargo build -p $HARNESS_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$HARNESS_BUILD_PACKAGE" ) || die 'cargo build failed'
    fi
    [ -x "$WORKER_BIN" ] || die "worker binary not found: $WORKER_BIN"
    [ -x "$TRIGGER_BIN" ] || die "trigger binary not found: $TRIGGER_BIN"
    if [ "$DOGFOOD_RUNNER" = "1" ]; then
        [ -x "$RUNNER_BIN" ] || die "forgejo-runner binary not found: $RUNNER_BIN"
    fi
}

check_auth() {
    CODEX_MODEL_ARG=
    AUTH_FILE_ARG=
    case "$HARNESS_AGENTS_AUTH" in
        chatgpt-oauth)
            _auth_file=${HARNESS_AGENTS_AUTH_FILE:-$HOME/.pi/agent/auth.json}
            [ -f "$_auth_file" ] || die "ChatGPT OAuth selected but $_auth_file is missing; run: pi /login openai-codex"
            grep -q 'openai-codex' "$_auth_file" 2>/dev/null || die "no openai-codex entry in $_auth_file; run: pi /login openai-codex"
            AUTH_FLAG=chatgpt-oauth
            [ -n "$HARNESS_AGENTS_CODEX_MODEL" ] && CODEX_MODEL_ARG="--codex-model $HARNESS_AGENTS_CODEX_MODEL"
            [ -n "$HARNESS_AGENTS_AUTH_FILE" ] && AUTH_FILE_ARG="--auth-file $HARNESS_AGENTS_AUTH_FILE"
            log "auth: ChatGPT OAuth ($_auth_file)"
            ;;
        anthropic-oauth)
            _auth_file=${HARNESS_AGENTS_AUTH_FILE:-$HOME/.pi/agent/auth.json}
            [ -f "$_auth_file" ] || die "Anthropic OAuth selected but $_auth_file is missing; run: pi /login anthropic"
            grep -q '"anthropic"' "$_auth_file" 2>/dev/null || die "no anthropic entry in $_auth_file; run: pi /login anthropic"
            AUTH_FLAG=anthropic-oauth
            [ -n "$HARNESS_AGENTS_AUTH_FILE" ] && AUTH_FILE_ARG="--auth-file $HARNESS_AGENTS_AUTH_FILE"
            [ -n "$HARNESS_AGENTS_ANTHROPIC_MODEL" ] && export HARNESS_AGENTS_ANTHROPIC_MODEL
            log "auth: Anthropic OAuth ($_auth_file)"
            ;;
        deepseek)
            [ -n "${HARNESS_DEEPSEEK_API_KEY:-}" ] || [ -n "${HARNESS_DEEPSEEK_API_KEY_PATH:-}" ] || die 'DeepSeek selected; set HARNESS_DEEPSEEK_API_KEY or HARNESS_DEEPSEEK_API_KEY_PATH'
            AUTH_FLAG=deepseek
            log 'auth: DeepSeek'
            ;;
        *) die "unknown HARNESS_AGENTS_AUTH '$HARNESS_AGENTS_AUTH'" ;;
    esac
}

parse_live_secrets() {
    mkdir -p "$SECRETS_DIR"
    python3 "$TOOLS_DIR/parse_secrets.py" \
        --source "$SECRETS_SOURCE" \
        --out "$ROLES_ENV" \
        --human-user "$DOGFOOD_HUMAN_USER" \
        --mechanical-user "$DOGFOOD_MECHANICAL_USER" \
        --product-manager-user "$DOGFOOD_PRODUCT_MANAGER_USER" \
        >"$LOG_DIR/parse-secrets.log" 2>&1 || die "failed to parse secrets (see logs/parse-secrets.log)"
    # shellcheck disable=SC1090
    . "$ROLES_ENV"
    log "parsed live credentials into $ROLES_ENV"
}

mint_admin_token() {
    [ -n "${DOGFOOD_ADMIN_USER:-}" ] || die 'no admin user in parsed secrets'
    [ -n "${DOGFOOD_ADMIN_PASSWORD:-}" ] || die 'no admin password in parsed secrets'
    python3 "$TOOLS_DIR/mint_admin_token.py" \
        --base-url "$BASE_URL" \
        --user "$DOGFOOD_ADMIN_USER" \
        --password "$DOGFOOD_ADMIN_PASSWORD"
}

configure_forgejo() {
    _webhook_args=
    if [ "$WEBHOOKS" = "1" ]; then
        _webhook_args="--webhook-url $WEBHOOK_URL --webhook-secret-file $WEBHOOK_SECRET_FILE"
    fi
    # _webhook_args intentionally word-split: values are generated by this script/config.
    # shellcheck disable=SC2086
    DOGFOOD_ADMIN_TOKEN="$ADMIN_TOKEN" python3 "$TOOLS_DIR/configure_forgejo.py" \
        --base-url "$BASE_URL" \
        --owner "$OWNER" \
        --repo "$NAME" \
        --roles-env "$ROLES_ENV" \
        --permission "$DOGFOOD_REPO_PERMISSION" \
        --ci-workflow-file "$SCRIPT_DIR/config/ci.yml" \
        --default-branch "$DOGFOOD_DEFAULT_BRANCH" \
        $_webhook_args
}

prepare_remote_repo() {
    ADMIN_TOKEN=${DOGFOOD_ADMIN_TOKEN:-}
    if [ -z "$ADMIN_TOKEN" ]; then
        log 'no reusable admin token found; minting a short-lived setup token from admin credentials...'
        ADMIN_TOKEN=$(mint_admin_token) || die 'failed to mint admin token'
    fi
    if configure_forgejo >"$LOG_DIR/configure-forgejo.log" 2>&1; then
        log "Forgejo repo prepared: $BASE_URL/$REPO"
        return 0
    fi
    if [ -n "${DOGFOOD_ADMIN_USER:-}" ] && [ -n "${DOGFOOD_ADMIN_PASSWORD:-}" ]; then
        log 'configured token failed; retrying with freshly minted admin token...'
        ADMIN_TOKEN=$(mint_admin_token) || die 'failed to mint admin token'
        configure_forgejo >>"$LOG_DIR/configure-forgejo.log" 2>&1 || die 'Forgejo setup failed (see logs/configure-forgejo.log)'
        log "Forgejo repo prepared: $BASE_URL/$REPO"
    else
        die 'Forgejo setup failed (see logs/configure-forgejo.log)'
    fi
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
        sleep 0.2 2>/dev/null || sleep 1
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
        sleep 0.2 2>/dev/null || sleep 1
    done
}

boot_trigger() {
    [ "$WEBHOOKS" = "1" ] || return 0
    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    ensure_secret_file "$WAKE_SECRET_FILE"
    mkdir -p "$WAKE_DIR"
    : >"$LOG_DIR/trigger.log"
    "$TRIGGER_BIN" --bind "$LOCAL_TRIGGER_BIND" \
        --webhook-secret-file "$WEBHOOK_SECRET_FILE" \
        --wake-secret-file "$WAKE_SECRET_FILE" \
        --wake-dir "$WAKE_DIR" \
        >>"$LOG_DIR/trigger.log" 2>&1 &
    TRIGGER_PID=$!
    echo "$TRIGGER_PID" >"$TRIGGER_PID_FILE"
    wait_for_log_line "$LOG_DIR/trigger.log" 'listening on' "$TRIGGER_PID" 'webhook trigger'
    log "trigger listening locally on $LOCAL_TRIGGER_BIND"
}

boot_tunnel() {
    [ "$WEBHOOKS" = "1" ] || return 0
    : >"$LOG_DIR/ssh-tunnel.log"
    # First cancel any stale multiplexed forward from an earlier interrupted run,
    # then start a dedicated non-multiplexed process so run/*.pid can stop it.
    ssh -O cancel -R "$REMOTE_WEBHOOK_HOST:$REMOTE_WEBHOOK_PORT:127.0.0.1:$LOCAL_TRIGGER_PORT" "$SSH_HOST" >/dev/null 2>&1 || true
    ssh -N -o ControlMaster=no -o ControlPath=none \
        -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 \
        -R "$REMOTE_WEBHOOK_HOST:$REMOTE_WEBHOOK_PORT:127.0.0.1:$LOCAL_TRIGGER_PORT" \
        "$SSH_HOST" >"$LOG_DIR/ssh-tunnel.log" 2>&1 &
    TUNNEL_PID=$!
    echo "$TUNNEL_PID" >"$TUNNEL_PID_FILE"
    sleep 1
    kill -0 "$TUNNEL_PID" 2>/dev/null || die 'ssh reverse tunnel failed (see logs/ssh-tunnel.log)'
    log "ssh reverse tunnel active: $SSH_HOST $REMOTE_WEBHOOK_HOST:$REMOTE_WEBHOOK_PORT -> local $LOCAL_TRIGGER_BIND"
}

mint_runner_token() {
    ssh "$SSH_HOST" "$DOGFOOD_REMOTE_FORGEJO_BIN --work-path $DOGFOOD_REMOTE_FORGEJO_WORK_PATH actions generate-runner-token" | tr -d '[:space:]'
}

boot_runner() {
    [ "$DOGFOOD_RUNNER" = "1" ] || return 0
    _runner_token=$(mint_runner_token) || die "failed to mint Forgejo runner registration token via ssh $SSH_HOST"
    [ -n "$_runner_token" ] || die "empty Forgejo runner registration token from ssh $SSH_HOST"
    mkdir -p "$RUNNER_DIR"
    : >"$LOG_DIR/runner-register.log"
    : >"$LOG_DIR/runner.log"
    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" register --no-interactive \
        --instance "$BASE_URL" --token "$_runner_token" \
        --name "dogfood-$$" --labels host:host ) \
        >"$LOG_DIR/runner-register.log" 2>&1 \
        || die "forgejo-runner register failed (see logs/runner-register.log)"
    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" daemon ) >"$LOG_DIR/runner.log" 2>&1 &
    _pid=$!
    echo "$_pid" >"$RUNNER_PID_FILE"
    log "forgejo-runner daemon pid=$_pid (logs/runner.log)"
}

role_env_key() {
    printf '%s' "$1" | tr '[:lower:]' '[:upper:]' | tr -c 'A-Z0-9' '_'
}

launch_role_worker() {
    _role=$1
    _key=$(role_env_key "$_role")
    eval "_user=\${HARNESS_FORGEJO_USER_${_key}:-}"
    eval "_token=\${HARNESS_FORGEJO_TOKEN_${_key}:-}"
    eval "_password=\${HARNESS_FORGEJO_PASSWORD_${_key}:-}"
    if [ -z "$_token" ]; then
        log "skipping role:$_role (no token in $ROLES_ENV)"
        return 0
    fi

    _wake_args=
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/$_role.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    _architect_args=
    [ "$_role" = "architect" ] && _architect_args="--architect-close-produced-issues" || _architect_args=

    # shellcheck disable=SC2086
    HARNESS_FORGEJO_TOKEN="$_token" \
    HARNESS_FORGEJO_USERNAME="$_user" \
    HARNESS_FORGEJO_PASSWORD="$_password" \
        "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" --repo "$REPO" \
        --kind role --role "$_role" --user "$_user" \
        --auth "$AUTH_FLAG" $CODEX_MODEL_ARG $AUTH_FILE_ARG \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" \
        $_wake_args $_architect_args \
        >"$LOG_DIR/$_role.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    [ "$WEBHOOKS" = "1" ] && wait_for_socket "$_wake_socket" "$_pid" "role:$_role"
    log "role:$_role user=$_user pid=$_pid (logs/$_role.log)"
}

launch_intake_labeler() {
    _token=${DOGFOOD_MECHANICAL_TOKEN:-$ADMIN_TOKEN}
    HARNESS_FORGEJO_TOKEN="$_token" \
        python3 "$TOOLS_DIR/label_intake.py" \
        --base-url "$BASE_URL" \
        --owner "$OWNER" \
        --repo "$NAME" \
        --started-at "$RUN_STARTED_AT" \
        --stop-file "$STOP_FILE" \
        >"$LOG_DIR/intake-labeler.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    log "intake-labeler pid=$_pid (logs/intake-labeler.log)"
}

launch_mechanical_worker() {
    _token=${DOGFOOD_MECHANICAL_TOKEN:-$ADMIN_TOKEN}
    _user=${DOGFOOD_MECHANICAL_USER:-${DOGFOOD_ADMIN_USER:-}}
    _password=${DOGFOOD_MECHANICAL_PASSWORD:-${DOGFOOD_ADMIN_PASSWORD:-}}
    _wake_args=
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/mechanical.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    # shellcheck disable=SC2086
    HARNESS_FORGEJO_TOKEN="$_token" \
    HARNESS_FORGEJO_USERNAME="$_user" \
    HARNESS_FORGEJO_PASSWORD="$_password" \
        "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" --repo "$REPO" \
        --kind mechanical \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" \
        $_wake_args \
        >"$LOG_DIR/mechanical.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    [ "$WEBHOOKS" = "1" ] && wait_for_socket "$_wake_socket" "$_pid" 'mechanical'
    log "mechanical user=${_user:-admin-token} pid=$_pid (logs/mechanical.log)"
}

launch_workers() {
    : >"$WORKERS_PID_FILE"
    log 'launching worker pool...'
    launch_role_worker engineer
    launch_role_worker reviewer
    launch_role_worker owner
    launch_role_worker human
    launch_mechanical_worker
    launch_intake_labeler
    launch_role_worker architect
}

monitor() {
    log ''
    log "Dogfood target: $BASE_URL/$REPO"
    log "File issues at:   $BASE_URL/$REPO/issues"
    log "Worker logs:      $LOG_DIR"
    if [ "$WEBHOOKS" = "1" ]; then
        log "Webhook URL from Forgejo host: $WEBHOOK_URL"
        log "SSH tunnel:       $SSH_HOST:$REMOTE_WEBHOOK_HOST:$REMOTE_WEBHOOK_PORT -> $LOCAL_TRIGGER_BIND"
    fi
    log 'Press Ctrl-C to stop local workers/trigger/tunnel.'
    while [ ! -f "$STOP_FILE" ]; do
        sleep 2
        if [ "$WEBHOOKS" = "1" ] && ! kill -0 "$(cat "$TUNNEL_PID_FILE" 2>/dev/null)" 2>/dev/null; then
            log 'ssh tunnel exited; shutting down.'
            break
        fi
    done
}

cmd_start() {
    load_config
    mkdir -p "$RUN_DIR" "$LOG_DIR" "$SECRETS_DIR"
    rm -f "$STOP_FILE"
    RUN_STARTED_AT=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    check_auth
    resolve_binaries
    parse_live_secrets

    if [ -f "$WORKERS_PID_FILE" ]; then
        die "a run may already be active; stop it first: $DISPLAY_SCRIPT stop"
    fi

    trap cleanup EXIT INT TERM
    boot_trigger
    boot_tunnel
    prepare_remote_repo
    boot_runner
    launch_workers
    monitor
}

cmd_status() {
    load_config
    log "target: $BASE_URL/$REPO"
    log "run dir: $RUN_DIR"
    log "logs: $LOG_DIR"
    for _file in "$TRIGGER_PID_FILE" "$TUNNEL_PID_FILE" "$WORKERS_PID_FILE"; do
        [ -f "$_file" ] && log "pid file: $_file" || true
    done
}

case "${1:-start}" in
    start | "") cmd_start ;;
    stop) cmd_stop ;;
    status) cmd_status ;;
    help | -h | --help) usage ;;
    *) usage >&2; exit 2 ;;
esac
