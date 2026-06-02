#!/bin/sh
# Reference-delivery example — POSIX launcher / teardown.
#
# Boots EVERY process of the production topology from development-profile binaries:
#   1. a throwaway Forgejo server (SQLite, Actions enabled),
#   2. a host-mode forgejo-runner producing real CI,
#   3. admin bootstrap + the production provision/seed binary
#      (org/users/tokens/repo/labels/CI workflow + one cross-repo intake issue),
#   4. one fixed harness-worker pool (one process per workflow role, plus one
#      mechanical reconciler) scanning every configured repo — all against
#      Forgejo with real LLM agents and wall time,
# then tears them all down cleanly on Ctrl-C / signal / `./run.sh stop`.
#
# This script targets the operator-facing entry points from harness-production
# rather than the harness-testing entry points. By default it builds/uses the
# development-profile binaries under target/debug; override HARNESS_*_BIN if you
# want prebuilt or release artifacts.
#
# Usage:
#   ./run.sh [start]          boot everything and block until Ctrl-C / stop-file
#   ./run.sh validate-webhooks inspect logs from a running/completed long-poll run
#   ./run.sh validate-multi-repo inspect provisioning + wake logs for all configured repos
#   ./run.sh stop             tear down a previous run via the saved PIDs
#   ./run.sh help             show this usage
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
if [ -n "${HARNESS_REFERENCE_DELIVERY_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$HARNESS_REFERENCE_DELIVERY_SCRIPT_DIR
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

sleep_short() {
    sleep 0.2 2>/dev/null || sleep 1
}

DISPLAY_SCRIPT=${HARNESS_REFERENCE_DELIVERY_ORIGINAL:-$SCRIPT_DIR/run.sh}

# Dash reads long-running scripts lazily. If this file is edited while the demo
# is sleeping in monitor(), the running shell may parse a half-new tail and fail
# during teardown. Run starts from a private snapshot so source edits/rebuilds do
# not affect the already-running launcher.
if [ "${HARNESS_REFERENCE_DELIVERY_SNAPSHOT:-0}" != "1" ]; then
    case "${1:-start}" in
        start | "")
            mkdir -p "$RUN_DIR"
            _snapshot="$RUN_DIR/run.sh.snapshot.$$"
            cp "$SCRIPT_DIR/run.sh" "$_snapshot"
            chmod 700 "$_snapshot"
            HARNESS_REFERENCE_DELIVERY_SNAPSHOT=1 \
            HARNESS_REFERENCE_DELIVERY_SCRIPT_DIR="$SCRIPT_DIR" \
            HARNESS_REFERENCE_DELIVERY_ORIGINAL="$DISPLAY_SCRIPT" \
                exec /bin/sh "$_snapshot" "$@"
            ;;
    esac
fi

usage() {
    cat <<EOF
usage: $DISPLAY_SCRIPT [start|validate-webhooks|validate-multi-repo|smoke-webhooks|stop|help]

  start (default)      boot Forgejo + runner, provision every configured repo,
                       seed the source intake issue, launch one fixed worker pool, then block until
                       Ctrl-C or the stop-file.
  validate-webhooks    inspect logs/ and report whether webhook wakes were
                       registered, accepted, delivered, consumed, and acted on.
  validate-multi-repo  additionally require every configured repo to appear in
                       provisioning, trigger, and worker startup logs.
  stop                 tear down a previous run via run/*.pid.
  help                 show this message.

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
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$WAKE_DIR" "$STOP_FILE" \
        "$RUN_DIR/cross-repo-intake.md" "$RUN_DIR"/run.sh.snapshot.* \
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
# (precedence: CLI/env > config/harness.env > built-in default). The file is
# the operator's edited config; a `VAR=x ./run.sh` still overrides it.
CONFIG_KNOBS="OWNER NAME REPOS CROSS_REPO_INTAKE CROSS_REPO_INTAKE_TITLE BASE_URL POLL_MS RUN_SECS WEBHOOKS TRIGGER_BIND WEBHOOK_URL \
ARCHITECT_CLOSE_PRODUCED_ISSUES \
HARNESS_AGENTS_AUTH HARNESS_AGENTS_CODEX_MODEL HARNESS_AGENTS_ANTHROPIC_MODEL \
HARNESS_AGENTS_AUTH_FILE HARNESS_FORGEJO_GOMAXPROCS HARNESS_FORGEJO_BINARY \
HARNESS_FORGEJO_RUNNER_BINARY HARNESS_WORKER_BIN HARNESS_PROVISION_BIN \
HARNESS_TRIGGER_BIN HARNESS_BUILD_PACKAGE"

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
    [ -f "$CONFIG_DIR/harness.env" ] || die "missing $CONFIG_DIR/harness.env"
    # Snapshot any pre-existing env values so they survive the file sourcing.
    # REPOS is special: an intentionally empty `REPOS=` selects legacy
    # OWNER/NAME mode, so presence matters even when the value is empty.
    _repos_was_set=${REPOS+x}
    _pre_REPOS_VALUE=${REPOS-}
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
    if [ -n "$_repos_was_set" ]; then
        REPOS=${_pre_REPOS_VALUE}
    fi

    OWNER=${OWNER:-acme}
    NAME=${NAME:-service}
    REPOS=${REPOS:-}
    CROSS_REPO_INTAKE=${CROSS_REPO_INTAKE:-auto}
    CROSS_REPO_INTAKE_TITLE=${CROSS_REPO_INTAKE_TITLE:-Coordinate greeting across service and canary}
    BASE_URL=${BASE_URL:-http://127.0.0.1:3000}
    POLL_MS=${POLL_MS:-2000}
    RUN_SECS=${RUN_SECS:-600}
    WEBHOOKS=${WEBHOOKS:-1}
    TRIGGER_BIND=${TRIGGER_BIND:-127.0.0.1:38080}
    WEBHOOK_URL=${WEBHOOK_URL:-http://127.0.0.1:38080/forgejo/webhook}
    ARCHITECT_CLOSE_PRODUCED_ISSUES=${ARCHITECT_CLOSE_PRODUCED_ISSUES:-1}
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

    _raw_repos=${REPOS:-$OWNER/$NAME}
    CONFIGURED_REPOS=
    WORKER_REPO_ARGS=
    FIRST_CONFIGURED_REPO=
    REPO_COUNT=0
    for _repo in $_raw_repos; do
        validate_repo_path "$_repo"
        case " $CONFIGURED_REPOS " in
            *" $_repo "*) continue ;;
        esac
        CONFIGURED_REPOS="${CONFIGURED_REPOS:+$CONFIGURED_REPOS }$_repo"
        WORKER_REPO_ARGS="$WORKER_REPO_ARGS --repo $_repo"
        [ -z "$FIRST_CONFIGURED_REPO" ] && FIRST_CONFIGURED_REPO=$_repo
        REPO_COUNT=$((REPO_COUNT + 1))
    done
    [ -n "$CONFIGURED_REPOS" ] || die 'no repositories configured'
    case "$CROSS_REPO_INTAKE" in
        auto) [ "$REPO_COUNT" -gt 1 ] && CROSS_REPO_ENABLED=1 || CROSS_REPO_ENABLED=0 ;;
        1 | yes | true) CROSS_REPO_ENABLED=1 ;;
        0 | no | false) CROSS_REPO_ENABLED=0 ;;
        *) die "CROSS_REPO_INTAKE must be auto, 1, or 0" ;;
    esac
    if [ "$CROSS_REPO_ENABLED" = "1" ] && [ "$REPO_COUNT" -lt 2 ]; then
        die 'cross-repo intake requires at least two repos; add REPOS or set CROSS_REPO_INTAKE=0'
    fi

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
    WORKER_BIN=${HARNESS_WORKER_BIN:-$WORKSPACE_ROOT/target/debug/harness-worker}
    PROVISION_BIN=${HARNESS_PROVISION_BIN:-$WORKSPACE_ROOT/target/debug/harness-provision-forgejo}
    TRIGGER_BIN=${HARNESS_TRIGGER_BIN:-$WORKSPACE_ROOT/target/debug/harness-trigger-forgejo}

    # Keep the demo entry point self-healing after source changes. Cargo is a
    # cheap no-op when the development binaries are already current; skipping
    # this is an explicit operator choice for prebuilt/current binaries.
    if [ "${HARNESS_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring development binaries are current (cargo build -p $HARNESS_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$HARNESS_BUILD_PACKAGE" ) \
            || die 'cargo build failed'
    fi

    [ -x "$WORKER_BIN" ] || die "worker binary not found: $WORKER_BIN"
    [ -x "$PROVISION_BIN" ] || die "provision binary not found: $PROVISION_BIN"
    [ -x "$TRIGGER_BIN" ] || die "trigger binary not found: $TRIGGER_BIN"

    _provision_help=$("$PROVISION_BIN" --help 2>&1 || true)
    case "$_provision_help" in
        *--seed-intake*--intake-title*--intake-body-file*) ;;
        *) die "provision binary is stale or incompatible: $PROVISION_BIN does not advertise --seed-intake/--intake-title/--intake-body-file. Re-run without HARNESS_SKIP_BUILD=1 or rebuild harness-production with cargo build -p $HARNESS_BUILD_PACKAGE." ;;
    esac
    _worker_help=$("$WORKER_BIN" --help 2>&1 || true)
    case "$_worker_help" in
        *--architect-close-produced-issues*) ;;
        *) die "worker binary is stale or incompatible: $WORKER_BIN does not advertise --architect-close-produced-issues. Re-run without HARNESS_SKIP_BUILD=1 or rebuild harness-production with cargo build -p $HARNESS_BUILD_PACKAGE." ;;
    esac

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

repo_slug() {
    repo_name "$1" | tr -c '[:alnum:]' '-' | tr '[:upper:]' '[:lower:]' | sed 's/^-*//;s/-*$//'
}

write_cross_repo_intake_body() {
    _body_file="$RUN_DIR/cross-repo-intake.md"
    {
        printf 'As an operator I want one visible greeting change coordinated across these repositories:\n\n'
        for _repo in $CONFIGURED_REPOS; do
            _slug=$(repo_slug "$_repo")
            printf -- '- `%s` (`target_repo`: `forgejo:%s`, child `slug`: `%s`)\n' "$_repo" "$_repo" "$_slug"
        done
        printf '\nArchitect guidance: triage this intake with one child code issue per repository listed above. '
        printf 'Use the exact `target_repo` and stable `slug` values shown, and keep each child scoped to its repository. '
        printf 'The parent issue should remain blocked until every child issue lands.\n\n'
        printf 'Acceptance: each repository receives its own implementation PR, CI passes, review approves, and all child PRs merge before this parent resolves.\n'
    } >"$_body_file"
    printf '%s\n' "$_body_file"
}

bootstrap_and_provision() {
    log 'bootstrapping admin + provisioning every configured repo (org/users/tokens/repo/labels/CI/webhook) ...'
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
    : >"$LOG_DIR/provision.log"
    _cross_body=
    if [ "$CROSS_REPO_ENABLED" = "1" ]; then
        _cross_body=$(write_cross_repo_intake_body)
        log "cross-repo intake enabled: seeding one parent issue in $FIRST_CONFIGURED_REPO"
    fi
    for _repo in $CONFIGURED_REPOS; do
        _owner=$(repo_owner "$_repo")
        _name=$(repo_name "$_repo")
        if [ "$CROSS_REPO_ENABLED" = "1" ] && [ "$_repo" != "$FIRST_CONFIGURED_REPO" ]; then
            log "provisioning $_repo (labels + CI + webhook; no separate intake) ..."
            # _webhook_args intentionally word-split: POSIX sh has no arrays and the
            # example paths above are controlled by this script/config.
            # shellcheck disable=SC2086
            _status=$(HARNESS_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
                --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$ROLES_ENV" \
                $_webhook_args --seed-intake no) \
                || die "provisioning $_repo failed"
        elif [ "$CROSS_REPO_ENABLED" = "1" ]; then
            log "provisioning $_repo (labels + CI + cross-repo parent intake) ..."
            # shellcheck disable=SC2086
            _status=$(HARNESS_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
                --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$ROLES_ENV" \
                $_webhook_args --intake-title "$CROSS_REPO_INTAKE_TITLE" \
                --intake-body-file "$_cross_body") \
                || die "provisioning $_repo failed"
        else
            log "provisioning $_repo (labels + CI + seeded intake issue) ..."
            # shellcheck disable=SC2086
            _status=$(HARNESS_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$PROVISION_BIN" \
                --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$ROLES_ENV" \
                $_webhook_args) \
                || die "provisioning $_repo failed"
        fi
        _issue=$(printf '%s\n' "$_status" | sed -n 's/.*intake issue #\([0-9][0-9]*\).*/\1/p')
        {
            printf 'repo=%s %s\n' "$_repo" "$_status"
            [ -n "$_issue" ] && printf 'repo=%s intake_issue_url=%s/%s/issues/%s\n' "$_repo" "$BASE_URL" "$_repo" "$_issue"
            if [ "$CROSS_REPO_ENABLED" = "1" ] && [ "$_repo" = "$FIRST_CONFIGURED_REPO" ] && [ -n "$_issue" ]; then
                printf 'repo=%s cross_repo_parent_url=%s/%s/issues/%s\n' "$_repo" "$BASE_URL" "$_repo" "$_issue"
            fi
            if [ "$CROSS_REPO_ENABLED" = "1" ] && [ "$_repo" != "$FIRST_CONFIGURED_REPO" ]; then
                printf 'repo=%s no_intake_seeded=cross-repo-target\n' "$_repo"
            fi
            if [ "$WEBHOOKS" = "1" ]; then
                printf 'repo=%s webhook registered url=%s\n' "$_repo" "$WEBHOOK_URL"
            else
                printf 'repo=%s webhook disabled\n' "$_repo"
            fi
        } >>"$LOG_DIR/provision.log"
        log "$_status"
        [ -n "$_issue" ] && log "  intake issue: $BASE_URL/$_repo/issues/$_issue"
        [ "$WEBHOOKS" = "1" ] && log "  webhook registered for $_repo ($WEBHOOK_URL)"
    done

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
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/$_role.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    _architect_args=
    if [ "$_role" = "architect" ] && [ "$ARCHITECT_CLOSE_PRODUCED_ISSUES" = "1" ]; then
        _architect_args="--architect-close-produced-issues"
    fi

    # Per-role secrets are literal env-assignment prefixes (never on argv). The
    # auth-mode env (DeepSeek key path) is exported globally by check_auth.
    # WORKER_REPO_ARGS / CODEX_MODEL_ARG / AUTH_FILE_ARG / _wake_args intentionally
    # word-split (POSIX has no arrays); repo values are owner/name with no spaces.
    # shellcheck disable=SC2086
    HARNESS_FORGEJO_TOKEN="$_token" \
    HARNESS_FORGEJO_USERNAME="$_user" \
    HARNESS_FORGEJO_PASSWORD="$_password" \
        "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" $WORKER_REPO_ARGS \
        --kind role --role "$_role" --user "$_user" \
        --auth "$AUTH_FLAG" $CODEX_MODEL_ARG $AUTH_FILE_ARG \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" --run-secs "$RUN_SECS" \
        --allow-synthetic-pr-prep --allow-bookkeeping-only-pr \
        $_wake_args $_architect_args \
        >"$LOG_DIR/$_role.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    if [ "$WEBHOOKS" = "1" ]; then
        wait_for_socket "$_wake_socket" "$_pid" "role:$_role"
    fi
    log "  role:$_role -> pid $_pid (logs/$_role.log)"
}

launch_workers() {
    : >"$WORKERS_PID_FILE"
    # Derive the role list from the provisioned secrets file (one HARNESS_FORGEJO_
    # USER_<KEY>=<role> per role binding) — never hardcoded. The value is both the
    # role id and the user handle (the Forgejo id==handle requirement).
    _roles=$(sed -n "s/^HARNESS_FORGEJO_USER_[A-Z0-9_]*='\(.*\)'\$/\1/p" "$ROLES_ENV")
    [ -n "$_roles" ] || die "no roles found in $ROLES_ENV"

    log 'launching role workers (production binary, real agents) ...'
    # The seeded intake issue is immediately available to the architect. Start
    # every other wake listener first, then launch architect last, so the first
    # role handoff webhook can find all downstream sockets even with a long poll.
    _architect_role=
    for _r in $_roles; do
        if [ "$_r" = "architect" ]; then
            _architect_role=$_r
        else
            launch_role_worker "$_r"
        fi
    done

    # One mechanical reconciler (controller plane; admin token, no agent).
    _wake_args=
    _wake_socket=
    if [ "$WEBHOOKS" = "1" ]; then
        _wake_socket="$WAKE_DIR/mechanical.sock"
        _wake_args="--wake-socket $_wake_socket --wake-secret-file $WAKE_SECRET_FILE"
    fi
    # shellcheck disable=SC2086
    HARNESS_FORGEJO_TOKEN="$ADMIN_TOKEN" "$WORKER_BIN" \
        --backend forgejo --base-url "$BASE_URL" $WORKER_REPO_ARGS \
        --kind mechanical \
        --poll-ms "$POLL_MS" --stop-file "$STOP_FILE" --run-secs "$RUN_SECS" \
        $_wake_args \
        >"$LOG_DIR/mechanical.log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKERS_PID_FILE"
    if [ "$WEBHOOKS" = "1" ]; then
        wait_for_socket "$_wake_socket" "$_pid" 'mechanical'
    fi
    log "  mechanical -> pid $_pid (logs/mechanical.log)"

    if [ -n "$_architect_role" ]; then
        launch_role_worker "$_architect_role"
    fi
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

validate_repo_specific_logs() {
    _ok=0
    _provision_log="$LOG_DIR/provision.log"
    _trigger_log="$LOG_DIR/trigger.log"
    for _repo in $CONFIGURED_REPOS; do
        validate_contains "$_provision_log" "repo=$_repo " \
            "provisioning recorded for $_repo" || _ok=1
        if [ "$CROSS_REPO_ENABLED" = "1" ]; then
            if [ "$_repo" = "$FIRST_CONFIGURED_REPO" ]; then
                validate_contains "$_provision_log" "repo=$_repo cross_repo_parent_url=" \
                    "cross-repo parent issue URL recorded for $_repo" || _ok=1
            else
                validate_contains "$_provision_log" "repo=$_repo no_intake_seeded=cross-repo-target" \
                    "target repo $_repo provisioned without a duplicate intake" || _ok=1
            fi
        else
            validate_contains "$_provision_log" "repo=$_repo intake_issue_url=" \
                "seeded intake issue URL recorded for $_repo" || _ok=1
        fi
        if [ "$WEBHOOKS" = "1" ]; then
            validate_contains "$_provision_log" "repo=$_repo webhook registered" \
                "webhook registration recorded for $_repo" || _ok=1
            validate_contains "$_trigger_log" "repo=$_repo " \
                "trigger accepted at least one webhook for $_repo" || _ok=1
        fi
        _worker_mentioned=0
        for _log in "$LOG_DIR"/*.log; do
            [ -f "$_log" ] || continue
            grep -q 'harness-worker:' "$_log" 2>/dev/null || continue
            if grep -F -q "$_repo" "$_log" 2>/dev/null; then
                _worker_mentioned=1
            fi
        done
        if [ "$_worker_mentioned" -eq 1 ]; then
            log "ok: worker startup logs mention $_repo"
        else
            log "missing: no worker startup log mentions $_repo"
            _ok=1
        fi
    done
    return "$_ok"
}

cmd_validate_webhooks() {
    load_config
    _ok=0
    _trigger_log="$LOG_DIR/trigger.log"
    _provision_log="$LOG_DIR/provision.log"

    [ -d "$LOG_DIR" ] || die "no logs/ directory yet; start a run first"
    log "validating webhook wake logs under $LOG_DIR"
    log "configured repos: $CONFIGURED_REPOS"
    log "configured POLL_MS=$POLL_MS; long-poll smoke expects POLL_MS=120000"

    if [ "${VALIDATE_REPO_SPECIFIC:-0}" = "1" ]; then
        validate_repo_specific_logs || _ok=1
    fi

    validate_contains "$_provision_log" 'webhook registered url=' \
        'repo webhook registration recorded' || _ok=1
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
    _wake_ticks=0
    _wake_progress=0
    _wake_no_work=0
    for _log in "$LOG_DIR"/*.log; do
        [ -f "$_log" ] || continue
        grep -q 'harness-worker:' "$_log" 2>/dev/null || continue
        _workers=$((_workers + 1))
        _name=${_log##*/}
        if grep -q 'consumed authenticated wake' "$_log" 2>/dev/null; then
            _consumed=$((_consumed + 1))
            _consumed_text=yes
        else
            _consumed_text=no
            _ok=1
        fi
        if grep -q 'completed tick trigger=wake actions=' "$_log" 2>/dev/null; then
            _wake_ticks=$((_wake_ticks + 1))
            _tick_text=yes
        else
            _tick_text=no
            _ok=1
        fi
        if grep -E -q 'completed tick trigger=wake actions=[1-9][0-9]*' "$_log" 2>/dev/null; then
            _wake_progress=$((_wake_progress + 1))
        fi
        if grep -q 'completed tick trigger=wake actions=0' "$_log" 2>/dev/null; then
            _wake_no_work=$((_wake_no_work + 1))
        fi
        log "worker $_name: consumed_wake=$_consumed_text wake_tick=$_tick_text"
    done

    if [ "$_workers" -eq 0 ]; then
        log 'missing: no harness-worker logs found'
        _ok=1
    fi
    if [ "$_wake_progress" -eq 0 ]; then
        log 'missing: no wake-triggered worker tick reported actions>0'
        _ok=1
    fi
    log "worker summary: workers=$_workers consumed=$_consumed wake_ticks=$_wake_ticks wake_progress=$_wake_progress wake_no_work=$_wake_no_work"
    log 'wake_no_work>0 means a worker woke, scanned fresh Forge state, and found no queue item.'

    if [ "$_ok" -eq 0 ]; then
        log 'webhook wake validation passed'
    else
        log 'webhook wake validation failed; inspect logs/provision.log, logs/trigger.log, and worker logs'
    fi
    return "$_ok"
}

cmd_validate_multi_repo() {
    VALIDATE_REPO_SPECIFIC=1 cmd_validate_webhooks
}

# --- Monitor ------------------------------------------------------------------

# Blocks until the stop-file appears, the server dies, or RUN_SECS elapses, so
# the EXIT/INT/TERM trap can tear everything down on Ctrl-C.
monitor() {
    log ''
    log "Forgejo UI:    $BASE_URL  (log in as any provisioned role)"
    log "Worker pool:   one worker per role scans: $CONFIGURED_REPOS"
    if [ "$CROSS_REPO_ENABLED" = "1" ]; then
        log "Cross-repo:    one parent intake in $FIRST_CONFIGURED_REPO fans out across the repo set"
    fi
    log 'Repo issue URLs:'
    for _repo in $CONFIGURED_REPOS; do
        log "  $_repo -> $BASE_URL/$_repo/issues"
    done
    log "Worker logs:   $LOG_DIR/ (role logs include the resolved repo set)"
    if [ "$CROSS_REPO_ENABLED" = "1" ]; then
        log 'Watch the source intake issue create one child issue per repo; each child'
        log 'should open its own PR, pass CI, receive review, merge, and then unblock'
        log 'the parent aggregation issue.'
    else
        log 'Watch each repo independently: its intake issue should be triaged, a PR'
        log 'should open, CI should run, review should land, and merge + reconcile labels'
        log 'should move — all in the Forgejo UI above.'
    fi
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
    check_auth
    resolve_binaries

    if [ -f "$SERVER_PID_FILE" ] && kill -0 "$(cat "$SERVER_PID_FILE" 2>/dev/null)" 2>/dev/null; then
        die "a run appears active (run/server.pid). Stop it first: $DISPLAY_SCRIPT stop"
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
    validate-webhooks | smoke-webhooks) cmd_validate_webhooks ;;
    validate-multi-repo) cmd_validate_multi_repo ;;
    stop) cmd_stop ;;
    help | -h | --help) usage ;;
    *)
        usage >&2
        exit 2
        ;;
esac
