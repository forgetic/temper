#!/bin/sh
# Dogfood Temper against the live Forgejo repository at git.ekanayaka.io.
# Secrets are read from ~/Documents/personal/forgejo-rhi and emitted only into
# gitignored examples/dogfood/secrets/roles.env. Tokens/passwords travel via env,
# never on argv; product-chat authorship fails closed when the human token is missing.

set -eu

if [ -n "${TEMPER_DOGFOOD_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$TEMPER_DOGFOOD_SCRIPT_DIR
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

DISPLAY_SCRIPT=${TEMPER_DOGFOOD_ORIGINAL:-$SCRIPT_DIR/run.sh}

# Snapshot long-running starts so edits to this ignored script cannot corrupt a
# running teardown path (same rationale as examples/reference-delivery).
if [ "${TEMPER_DOGFOOD_SNAPSHOT:-0}" != "1" ]; then
    case "${1:-start}" in
        start | "" | product-chat)
            mkdir -p "$RUN_DIR"
            _snapshot="$RUN_DIR/run.sh.snapshot.$$"
            cp "$SCRIPT_DIR/run.sh" "$_snapshot"
            chmod 700 "$_snapshot"
            TEMPER_DOGFOOD_SNAPSHOT=1 \
            TEMPER_DOGFOOD_SCRIPT_DIR="$SCRIPT_DIR" \
            TEMPER_DOGFOOD_ORIGINAL="$DISPLAY_SCRIPT" \
            TEMPER_DOGFOOD_SNAPSHOT_FILE="$_snapshot" \
                exec /bin/sh "$_snapshot" "$@"
            ;;
    esac
fi

usage() {
    cat <<EOF
usage: $DISPLAY_SCRIPT [start|preflight|product-chat|stop|status|help]

  start (default)  parse live credentials, register/refresh the webhook, open an
                   ssh reverse tunnel through 'rhi', launch trigger + workers,
                   then block until Ctrl-C.
  preflight        explain whether engineer automation has the declared tool,
                   runner binding, diff guard, credentials, and workspace paths;
                   also reports why code+ready issues are idle when possible.
  product-chat     start an interactive terminal product-manager conversation;
                   pass extra args after the command (for example
                   --transcript-issue 3) to resume a transcript.
  stop             stop local workers, trigger, and ssh tunnel from run/*.pid.
  status           show local process/log locations.
  help             show this message.

File new work in: ${BASE_URL:-https://git.ekanayaka.io}/${REPO:-ai/temper}/issues
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
    REPO=${REPO:-ai/temper}
    SECRETS_SOURCE=${SECRETS_SOURCE:-$HOME/Documents/personal/forgejo-rhi}
    SSH_HOST=${SSH_HOST:-rhi}
    LOCAL_TRIGGER_BIND=${LOCAL_TRIGGER_BIND:-127.0.0.1:39080}
    REMOTE_WEBHOOK_HOST=${REMOTE_WEBHOOK_HOST:-127.0.0.1}
    REMOTE_WEBHOOK_PORT=${REMOTE_WEBHOOK_PORT:-39080}
    WEBHOOK_URL=${WEBHOOK_URL:-http://127.0.0.1:39080/forgejo/webhook}
    WEBHOOKS=${WEBHOOKS:-1}
    POLL_MS=${POLL_MS:-120000}
    DOGFOOD_HUMAN_USER=${DOGFOOD_HUMAN_USER:-bot}
    DOGFOOD_PRODUCT_CHAT_HUMAN_USER=${DOGFOOD_PRODUCT_CHAT_HUMAN_USER:-}
    DOGFOOD_MECHANICAL_USER=${DOGFOOD_MECHANICAL_USER:-bot}
    DOGFOOD_PRODUCT_MANAGER_USER=${DOGFOOD_PRODUCT_MANAGER_USER:-product-manager}
    DOGFOOD_REPO_PERMISSION=${DOGFOOD_REPO_PERMISSION:-write}
    DOGFOOD_CONFIGURE_CI=${DOGFOOD_CONFIGURE_CI:-0}
    DOGFOOD_ENABLE_ENGINEER_AUTOMATION=${DOGFOOD_ENABLE_ENGINEER_AUTOMATION:-0}
    DOGFOOD_PR_DIFF_GUARD=${DOGFOOD_PR_DIFF_GUARD:-1}
    DOGFOOD_ALLOW_BOOKKEEPING_ONLY_PR=${DOGFOOD_ALLOW_BOOKKEEPING_ONLY_PR:-0}
    DOGFOOD_PREFLIGHT_QUERY_ISSUES=${DOGFOOD_PREFLIGHT_QUERY_ISSUES:-1}
    TEMPER_CODING_WORKSPACE_ROOT=${TEMPER_CODING_WORKSPACE_ROOT:-}
    TEMPER_CODING_WORKSPACE_COMMAND=${TEMPER_CODING_WORKSPACE_COMMAND:-}
    TEMPER_CODING_WORKSPACE_REMOTE=${TEMPER_CODING_WORKSPACE_REMOTE:-origin}
    TEMPER_CODING_WORKSPACE_PUSH=${TEMPER_CODING_WORKSPACE_PUSH:-1}
    TEMPER_CODING_WORKSPACE_PR_LABELS=${TEMPER_CODING_WORKSPACE_PR_LABELS:-implementation,needs-reviewer,needs-merge}
    TEMPER_WORKER_BIN=${TEMPER_WORKER_BIN:-}
    TEMPER_TRIGGER_BIN=${TEMPER_TRIGGER_BIN:-}
    TEMPER_PRODUCT_CHAT_BIN=${TEMPER_PRODUCT_CHAT_BIN:-}
    TEMPER_BUILD_PACKAGE=${TEMPER_BUILD_PACKAGE:-temper-production}
    TEMPER_FORGEJO_RUNNER_BINARY=${TEMPER_FORGEJO_RUNNER_BINARY:-}
    DOGFOOD_ROLE_DECISION=${DOGFOOD_ROLE_DECISION:-smith}
    DOGFOOD_PRODUCT_CHAT_RESPONDER=${DOGFOOD_PRODUCT_CHAT_RESPONDER:-smith}
    SMITH_WORKSPACE_ROOT=${SMITH_WORKSPACE_ROOT:-$HOME/src/rust/smith}
    SMITH_WORKFLOW_ROLE_DECISION_BIN=${SMITH_WORKFLOW_ROLE_DECISION_BIN:-}
    SMITH_WORKFLOW_ROLE_DECISION_ARGS_JSON=${SMITH_WORKFLOW_ROLE_DECISION_ARGS_JSON:-}
    if [ -z "$SMITH_WORKFLOW_ROLE_DECISION_ARGS_JSON" ]; then
        SMITH_WORKFLOW_ROLE_DECISION_ARGS_JSON='["--auth","chatgpt-oauth"]'
    fi
    SMITH_WORKFLOW_ROLE_DECISION_ENV_ALLOWLIST=${SMITH_WORKFLOW_ROLE_DECISION_ENV_ALLOWLIST:-}
    SMITH_WORKFLOW_ROLE_DECISION_CWD=${SMITH_WORKFLOW_ROLE_DECISION_CWD:-}
    SMITH_WORKFLOW_ROLE_DECISION_TIMEOUT_SECS=${SMITH_WORKFLOW_ROLE_DECISION_TIMEOUT_SECS:-}
    SMITH_PRODUCT_MANAGER_RESPONDER_BIN=${SMITH_PRODUCT_MANAGER_RESPONDER_BIN:-}
    SMITH_PRODUCT_MANAGER_RESPONDER_ARGS_JSON=${SMITH_PRODUCT_MANAGER_RESPONDER_ARGS_JSON:-}
    if [ -z "$SMITH_PRODUCT_MANAGER_RESPONDER_ARGS_JSON" ]; then
        SMITH_PRODUCT_MANAGER_RESPONDER_ARGS_JSON='["--auth","chatgpt-oauth"]'
    fi
    SMITH_PRODUCT_MANAGER_RESPONDER_ENV_ALLOWLIST=${SMITH_PRODUCT_MANAGER_RESPONDER_ENV_ALLOWLIST:-}
    SMITH_PRODUCT_MANAGER_RESPONDER_CWD=${SMITH_PRODUCT_MANAGER_RESPONDER_CWD:-}
    SMITH_PRODUCT_MANAGER_RESPONDER_TIMEOUT_SECS=${SMITH_PRODUCT_MANAGER_RESPONDER_TIMEOUT_SECS:-}
    SMITH_BUILD_PACKAGE=${SMITH_BUILD_PACKAGE:-smith-temper-agent-cli}
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
    WORKER_BIN=${TEMPER_WORKER_BIN:-$WORKSPACE_ROOT/target/debug/temper-worker}
    TRIGGER_BIN=${TEMPER_TRIGGER_BIN:-$WORKSPACE_ROOT/target/debug/temper-trigger-forgejo}
    RUNNER_BIN=${TEMPER_FORGEJO_RUNNER_BINARY:-$WORKSPACE_ROOT/.cache/forgejo/forgejo-runner-3.5.1-linux-amd64}
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring development-profile binaries are current (cargo build -p $TEMPER_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$TEMPER_BUILD_PACKAGE" ) || die 'cargo build failed'
    fi
    [ -x "$WORKER_BIN" ] || die "worker binary not found: $WORKER_BIN"
    [ -x "$TRIGGER_BIN" ] || die "trigger binary not found: $TRIGGER_BIN"
    if [ "$DOGFOOD_RUNNER" = "1" ]; then
        [ -x "$RUNNER_BIN" ] || die "forgejo-runner binary not found: $RUNNER_BIN"
    fi
    case "$DOGFOOD_ROLE_DECISION" in
        smith | "") resolve_smith_workflow_role_decision ;;
        *) die "unknown DOGFOOD_ROLE_DECISION '$DOGFOOD_ROLE_DECISION' (expected smith)" ;;
    esac
}

resolve_product_chat_binary() {
    PRODUCT_CHAT_BIN=${TEMPER_PRODUCT_CHAT_BIN:-$WORKSPACE_ROOT/target/debug/temper-product-manager-chat}
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring product-chat binary is current (cargo build -p $TEMPER_BUILD_PACKAGE --bin temper-product-manager-chat)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$TEMPER_BUILD_PACKAGE" --bin temper-product-manager-chat ) || die 'cargo build failed'
    fi
    [ -x "$PRODUCT_CHAT_BIN" ] || die "product-chat binary not found: $PRODUCT_CHAT_BIN"
}

resolve_smith_product_manager_responder() {
    [ -d "$SMITH_WORKSPACE_ROOT" ] || die "Smith workspace not found: $SMITH_WORKSPACE_ROOT"
    SMITH_PM_RESPONDER_BIN=${SMITH_PRODUCT_MANAGER_RESPONDER_BIN:-$SMITH_WORKSPACE_ROOT/target/debug/smith-product-manager-responder}
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring Smith product-manager responder is current (cargo build -p $SMITH_BUILD_PACKAGE --bin smith-product-manager-responder)..."
        ( cd "$SMITH_WORKSPACE_ROOT" && cargo build -p "$SMITH_BUILD_PACKAGE" --bin smith-product-manager-responder ) || die 'Smith cargo build failed'
    fi
    [ -x "$SMITH_PM_RESPONDER_BIN" ] || die "Smith product-manager responder binary not found: $SMITH_PM_RESPONDER_BIN"
}

resolve_smith_workflow_role_decision() {
    [ -d "$SMITH_WORKSPACE_ROOT" ] || die "Smith workspace not found: $SMITH_WORKSPACE_ROOT"
    SMITH_ROLE_DECISION_BIN=${SMITH_WORKFLOW_ROLE_DECISION_BIN:-$SMITH_WORKSPACE_ROOT/target/debug/smith-workflow-role-decision}
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring Smith workflow-role decision responder is current (cargo build -p $SMITH_BUILD_PACKAGE --bin smith-workflow-role-decision)..."
        ( cd "$SMITH_WORKSPACE_ROOT" && cargo build -p "$SMITH_BUILD_PACKAGE" --bin smith-workflow-role-decision ) || die 'Smith cargo build failed'
    fi
    [ -x "$SMITH_ROLE_DECISION_BIN" ] || die "Smith workflow-role decision binary not found: $SMITH_ROLE_DECISION_BIN"
    ROLE_DECISION_ARGS="--role-decision-command $SMITH_ROLE_DECISION_BIN"
    [ -n "$SMITH_WORKFLOW_ROLE_DECISION_CWD" ] && ROLE_DECISION_ARGS="$ROLE_DECISION_ARGS --role-decision-cwd $SMITH_WORKFLOW_ROLE_DECISION_CWD"
    [ -n "$SMITH_WORKFLOW_ROLE_DECISION_TIMEOUT_SECS" ] && ROLE_DECISION_ARGS="$ROLE_DECISION_ARGS --role-decision-timeout-secs $SMITH_WORKFLOW_ROLE_DECISION_TIMEOUT_SECS"
    log "role decisions: Smith process ($SMITH_ROLE_DECISION_BIN)"
}

dogfood_preflight() {
    _strict=${1:-0}
    _strict_args=
    [ "$_strict" = "1" ] && _strict_args="--strict"
    _query_args=
    [ "${DOGFOOD_PREFLIGHT_QUERY_ISSUES:-1}" = "1" ] && _query_args="--query-issues"
    # _strict_args/_query_args are generated flags and intentionally word-split.
    # shellcheck disable=SC2086
    python3 "$TOOLS_DIR/preflight.py" \
        --workflow-file "$WORKSPACE_ROOT/crates/temper-workflow/fixtures/reference-delivery.json" \
        --roles-env "$ROLES_ENV" \
        --base-url "$BASE_URL" \
        --owner "$OWNER" \
        --repo "$NAME" \
        --enable-engineer-automation "$DOGFOOD_ENABLE_ENGINEER_AUTOMATION" \
        --workspace-root "$TEMPER_CODING_WORKSPACE_ROOT" \
        --workspace-command "$TEMPER_CODING_WORKSPACE_COMMAND" \
        --pr-diff-guard "$DOGFOOD_PR_DIFF_GUARD" \
        --allow-bookkeeping-only-pr "$DOGFOOD_ALLOW_BOOKKEEPING_ONLY_PR" \
        $_query_args $_strict_args
}

check_coding_workspace() {
    dogfood_preflight 1 || die 'engineer automation preflight failed'
    if [ "$DOGFOOD_ENABLE_ENGINEER_AUTOMATION" = "1" ]; then
        log "coding workspace: $TEMPER_CODING_WORKSPACE_ROOT (local-git provider)"
    fi
}

# Smith owns provider/auth validation for configured responders.

configure_product_chat_responder_args() {
    PRODUCT_CHAT_RESPONDER_ARGS=
    case "$DOGFOOD_PRODUCT_CHAT_RESPONDER" in
        smith | "")
            resolve_smith_product_manager_responder
            PRODUCT_CHAT_RESPONDER_ARGS="--responder-command $SMITH_PM_RESPONDER_BIN"
            [ -n "$SMITH_PRODUCT_MANAGER_RESPONDER_CWD" ] && PRODUCT_CHAT_RESPONDER_ARGS="$PRODUCT_CHAT_RESPONDER_ARGS --responder-cwd $SMITH_PRODUCT_MANAGER_RESPONDER_CWD"
            [ -n "$SMITH_PRODUCT_MANAGER_RESPONDER_TIMEOUT_SECS" ] && PRODUCT_CHAT_RESPONDER_ARGS="$PRODUCT_CHAT_RESPONDER_ARGS --responder-timeout-secs $SMITH_PRODUCT_MANAGER_RESPONDER_TIMEOUT_SECS"
            log "product-chat responder: Smith process ($SMITH_PM_RESPONDER_BIN)"
            ;;
        *) die "unknown DOGFOOD_PRODUCT_CHAT_RESPONDER '$DOGFOOD_PRODUCT_CHAT_RESPONDER' (expected smith)" ;;
    esac
}

parse_live_secrets() {
    mkdir -p "$SECRETS_DIR"
    python3 "$TOOLS_DIR/parse_secrets.py" \
        --source "$SECRETS_SOURCE" \
        --out "$ROLES_ENV" \
        --human-user "$DOGFOOD_HUMAN_USER" \
        --product-chat-human-user "$DOGFOOD_PRODUCT_CHAT_HUMAN_USER" \
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
    _ci_args=
    if [ "$DOGFOOD_CONFIGURE_CI" = "1" ]; then
        _ci_args="--install-ci --ci-workflow-file $SCRIPT_DIR/config/ci.yml"
    fi
    # _webhook_args/_ci_args intentionally word-split: values are generated by this script/config.
    # shellcheck disable=SC2086
    DOGFOOD_ADMIN_TOKEN="$ADMIN_TOKEN" python3 "$TOOLS_DIR/configure_forgejo.py" \
        --base-url "$BASE_URL" \
        --owner "$OWNER" \
        --repo "$NAME" \
        --roles-env "$ROLES_ENV" \
        --permission "$DOGFOOD_REPO_PERMISSION" \
        --default-branch "$DOGFOOD_DEFAULT_BRANCH" \
        $_ci_args $_webhook_args
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
    eval "_user=\${TEMPER_FORGEJO_USER_${_key}:-}"
    eval "_token=\${TEMPER_FORGEJO_TOKEN_${_key}:-}"
    eval "_password=\${TEMPER_FORGEJO_PASSWORD_${_key}:-}"
    if [ -z "$_token" ]; then
        log "skipping role:$_role (no token in $ROLES_ENV)"
        return 0
    fi
    if [ "$DOGFOOD_ENABLE_ENGINEER_AUTOMATION" != "1" ]; then
        if [ "$_role" = "engineer" ]; then
            log 'skipping role:engineer (DOGFOOD_ENABLE_ENGINEER_AUTOMATION=0; coding workspace not enabled)'
            return 0
        fi
        if [ "$_role" = "owner" ]; then
            log 'skipping role:owner (DOGFOOD_ENABLE_ENGINEER_AUTOMATION=0; auto-merge disabled)'
            return 0
        fi
    fi

    _wake_args=
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/$_role.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    # shellcheck disable=SC2086
    TEMPER_FORGEJO_TOKEN="$_token" \
    TEMPER_FORGEJO_USERNAME="$_user" \
    TEMPER_FORGEJO_PASSWORD="$_password" \
    TEMPER_CODING_WORKSPACE_ROOT="$TEMPER_CODING_WORKSPACE_ROOT" \
    TEMPER_CODING_WORKSPACE_COMMAND="$TEMPER_CODING_WORKSPACE_COMMAND" \
    TEMPER_CODING_WORKSPACE_REMOTE="$TEMPER_CODING_WORKSPACE_REMOTE" \
    TEMPER_CODING_WORKSPACE_PUSH="$TEMPER_CODING_WORKSPACE_PUSH" \
    TEMPER_CODING_WORKSPACE_PR_LABELS="$TEMPER_CODING_WORKSPACE_PR_LABELS" \
    TEMPER_WORKER_ROLE_DECISION_ARGS_JSON="$SMITH_WORKFLOW_ROLE_DECISION_ARGS_JSON" \
    TEMPER_WORKER_ROLE_DECISION_ENV_ALLOWLIST="$SMITH_WORKFLOW_ROLE_DECISION_ENV_ALLOWLIST" \
        "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" --repo "$REPO" \
        --kind role --role "$_role" --user "$_user" \
        $ROLE_DECISION_ARGS \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" \
        $_wake_args \
        >"$LOG_DIR/$_role.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    [ "$WEBHOOKS" = "1" ] && wait_for_socket "$_wake_socket" "$_pid" "role:$_role"
    log "role:$_role user=$_user pid=$_pid (logs/$_role.log)"
}

launch_intake_labeler() {
    _token=${DOGFOOD_MECHANICAL_TOKEN:-$ADMIN_TOKEN}
    TEMPER_FORGEJO_TOKEN="$_token" \
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
    TEMPER_FORGEJO_TOKEN="$_token" \
    TEMPER_FORGEJO_USERNAME="$_user" \
    TEMPER_FORGEJO_PASSWORD="$_password" \
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

cleanup_product_chat_snapshot() {
    [ -n "${TEMPER_DOGFOOD_SNAPSHOT_FILE:-}" ] && rm -f "$TEMPER_DOGFOOD_SNAPSHOT_FILE" 2>/dev/null || true
    rmdir "$RUN_DIR" 2>/dev/null || true
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

cmd_preflight() {
    load_config
    mkdir -p "$RUN_DIR" "$LOG_DIR" "$SECRETS_DIR"
    if [ ! -f "$ROLES_ENV" ] && [ -f "$SECRETS_SOURCE" ]; then
        parse_live_secrets
    elif [ ! -f "$ROLES_ENV" ]; then
        log "no $ROLES_ENV yet; credential and live issue checks will explain what is missing"
    fi
    dogfood_preflight 0
}

cmd_product_chat() {
    load_config
    mkdir -p "$RUN_DIR" "$LOG_DIR" "$SECRETS_DIR"
    trap cleanup_product_chat_snapshot EXIT INT TERM
    resolve_product_chat_binary
    configure_product_chat_responder_args
    parse_live_secrets

    _pm_token=${TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER:-}
    [ -n "$_pm_token" ] || die "product-chat requires TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER in $ROLES_ENV"

    [ -n "$DOGFOOD_PRODUCT_CHAT_HUMAN_USER" ] || die 'product-chat requires DOGFOOD_PRODUCT_CHAT_HUMAN_USER in config/dogfood.env'
    _human_token=${TEMPER_FORGEJO_TOKEN_PRODUCT_CHAT_HUMAN:-}
    [ -n "$_human_token" ] || die "product-chat requires a token for DOGFOOD_PRODUCT_CHAT_HUMAN_USER=$DOGFOOD_PRODUCT_CHAT_HUMAN_USER in $ROLES_ENV"

    # PRODUCT_CHAT_RESPONDER_ARGS is generated from Smith process config and
    # intentionally word-split, matching role-worker launch style.
    # shellcheck disable=SC2086
    TEMPER_PRODUCT_CHAT_HUMAN_TOKEN="$_human_token" \
    TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN="$_pm_token" \
    TEMPER_PRODUCT_CHAT_RESPONDER_ARGS_JSON="$SMITH_PRODUCT_MANAGER_RESPONDER_ARGS_JSON" \
    TEMPER_PRODUCT_CHAT_RESPONDER_ENV_ALLOWLIST="$SMITH_PRODUCT_MANAGER_RESPONDER_ENV_ALLOWLIST" \
        "$PRODUCT_CHAT_BIN" repl \
        --base-url "$BASE_URL" --repo "$REPO" \
        $PRODUCT_CHAT_RESPONDER_ARGS \
        "$@"
}

cmd_start() {
    load_config
    mkdir -p "$RUN_DIR" "$LOG_DIR" "$SECRETS_DIR"
    rm -f "$STOP_FILE"
    RUN_STARTED_AT=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    parse_live_secrets
    check_coding_workspace
    resolve_binaries

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
    preflight) cmd_preflight ;;
    product-chat) shift; cmd_product_chat "$@" ;;
    stop) cmd_stop ;;
    status) cmd_status ;;
    help | -h | --help) usage ;;
    *) usage >&2; exit 2 ;;
esac
