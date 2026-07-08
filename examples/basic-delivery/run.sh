#!/bin/sh
# Fixed, minimal launcher for the basic-delivery happy-path demo.
#
# It boots throwaway Forgejo + forgejo-runner, starts the local jig fake LLM,
# runs `temper --config <bundle-dir> init --apply --yes`, commits the tiny
# demo project/CI baseline, launches `temper --config <bundle-dir> serve
# standalone`, then files one site-admin intake issue after the
# webhook listener is ready.

set -eu

SCRIPT_DIR=$(CDPATH=; cd -- "$(dirname -- "$0")" && pwd)
WORKSPACE_ROOT=$(CDPATH=; cd -- "$SCRIPT_DIR/../.." && pwd)
CONFIG_DIR="$SCRIPT_DIR/config"
RUN_DIR="$SCRIPT_DIR/run"
LOG_DIR="$SCRIPT_DIR/logs"

BASE_URL=http://127.0.0.1:4100
HOST=127.0.0.1
PORT=4100
DAEMON_BIND=127.0.0.1:38100
WEBHOOK_URL=http://$DAEMON_BIND/forgejo/webhook

OWNER=acme
NAME=service
REPO=$OWNER/$NAME
DEFAULT_BRANCH=main
INTAKE_TITLE='Service banner should identify the environment'
INTAKE_BODY_PATH="$CONFIG_DIR/intake-issue.md"

FORGEJO_VERSION=7.0.12
FORGEJO_RUNNER_VERSION=3.5.1
FORGEJO_BIN="$WORKSPACE_ROOT/.cache/forgejo/forgejo-$FORGEJO_VERSION-linux-amd64"
RUNNER_BIN="$WORKSPACE_ROOT/.cache/forgejo/forgejo-runner-$FORGEJO_RUNNER_VERSION-linux-amd64"
RUN_BIN="$WORKSPACE_ROOT/target/debug/temper"
JIG_REPO="$HOME/src/rust/jig"
JIG_BIN="$JIG_REPO/target/debug/jig"
JIG_FIXTURE_PATH="$JIG_REPO/fixtures/basic-delivery.json"
INIT_PROVIDER_KEY=basic-delivery-jig-dummy-key
ADMIN_USER=basicadmin
ADMIN_EMAIL=basicadmin@example.invalid
ADMIN_PASSWORD='Basic-Delivery-Admin-1!'

RUN_SECS=600
GOMAXPROCS=2
export GOMAXPROCS

FORGEJO_DATA="$RUN_DIR/forgejo"
APP_INI="$FORGEJO_DATA/custom/conf/app.ini"
RUNNER_DIR="$RUN_DIR/runner"
STOP_FILE="$RUN_DIR/stop"
SERVER_PID_FILE="$RUN_DIR/server.pid"
RUNNER_PID_FILE="$RUN_DIR/runner.pid"
RUN_PID_FILE="$RUN_DIR/run.pid"
JIG_PID_FILE="$RUN_DIR/jig.pid"
JIG_STDIN="$RUN_DIR/jig.stdin"
CONFIG_FILE="$RUN_DIR/config.toml"
CREDENTIALS_FILE="$RUN_DIR/credentials.toml"
INIT_WORKFLOW_PATH="$RUN_DIR/workflow.yaml"
WEBHOOK_SECRET_FILE="$RUN_DIR/webhook-secret"

log() { printf '[run.sh] %s\n' "$*"; }
die() { printf '[run.sh] error: %s\n' "$*" >&2; exit 1; }
sleep_short() { sleep 0.2 2>/dev/null || sleep 1; }

usage() {
    cat <<EOF
usage: ./run.sh [start|stop|help]

  start (default)      run the fixed basic-delivery demo
  stop                 tear down a previous run via run/*.pid
  help                 show this message

The demo intentionally has no config knobs: repo, ports, cadences, jig fixture,
and binary locations are fixed in this script.
EOF
}

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
    stop_pid_file "$RUN_PID_FILE"
    stop_pid_file "$JIG_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    stop_pid_file "$SERVER_PID_FILE"
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$STOP_FILE" \
        "$RUN_DIR/repo-seed" "$RUN_DIR/workspaces" \
        "$CONFIG_FILE" "$CREDENTIALS_FILE" "$INIT_WORKFLOW_PATH" \
        "$WEBHOOK_SECRET_FILE" "$JIG_STDIN" \
        2>/dev/null || true
    rmdir "$RUN_DIR" 2>/dev/null || true
    log 'teardown complete'
}

cmd_stop() {
    [ -d "$RUN_DIR" ] || { log 'nothing to stop (no run/ dir)'; return 0; }
    cleanup
}

resolve_binaries() {
    command -v curl >/dev/null 2>&1 || die 'curl is required'
    command -v git >/dev/null 2>&1 || die 'git is required'
    command -v mkfifo >/dev/null 2>&1 || die 'mkfifo is required'
    command -v python3 >/dev/null 2>&1 || die 'python3 is required'

    log 'building temper development binary...'
    ( cd "$WORKSPACE_ROOT" && cargo build -p temper ) || die 'cargo build -p temper failed'
    [ -x "$RUN_BIN" ] || die "temper binary not found: $RUN_BIN"
    [ -x "$FORGEJO_BIN" ] || die "forgejo binary not found: $FORGEJO_BIN (pre-stage with: cargo test -p temper-forgejo-fixture --test cache -- --ignored)"
    [ -x "$RUNNER_BIN" ] || die "forgejo-runner binary not found: $RUNNER_BIN (pre-stage with: cargo test -p temper-forgejo-fixture --test cache -- --ignored)"
    [ -d "$JIG_REPO" ] || die "jig checkout not found: $JIG_REPO"
    [ -f "$JIG_FIXTURE_PATH" ] || die "jig fixture not found: $JIG_FIXTURE_PATH"
    log 'building jig development binary...'
    ( cd "$JIG_REPO" && cargo build -p jig ) || die 'cargo build -p jig failed'
    [ -x "$JIG_BIN" ] || die "jig binary not found: $JIG_BIN"
}

write_app_ini() {
    mkdir -p "$FORGEJO_DATA/custom/conf" "$FORGEJO_DATA/data" "$FORGEJO_DATA/log" "$FORGEJO_DATA/repos"
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

forgejo_cli() {
    GITEA_WORK_DIR="$FORGEJO_DATA" "$FORGEJO_BIN" --config "$APP_INI" "$@"
}

boot_server() {
    log "booting Forgejo at $BASE_URL ..."
    if curl -fsS "$BASE_URL/api/v1/version" >/dev/null 2>&1; then
        die "Forgejo already responds at $BASE_URL; stop the existing run first"
    fi
    write_app_ini
    forgejo_cli migrate >"$LOG_DIR/forgejo-migrate.log" 2>&1 || die 'forgejo migrate failed (see logs/forgejo-migrate.log)'
    GITEA_WORK_DIR="$FORGEJO_DATA" "$FORGEJO_BIN" --config "$APP_INI" web >"$LOG_DIR/forgejo.log" 2>&1 &
    SERVER_PID=$!
    echo "$SERVER_PID" >"$SERVER_PID_FILE"

    _i=0
    until curl -fsS "$BASE_URL/api/v1/version" >/dev/null 2>&1; do
        kill -0 "$SERVER_PID" 2>/dev/null || die 'forgejo exited during startup (see logs/forgejo.log)'
        _i=$((_i + 1))
        [ "$_i" -gt 300 ] && die 'forgejo did not become ready (see logs/forgejo.log)'
        sleep_short
    done
    log "Forgejo ready (pid $SERVER_PID)"
}

boot_runner() {
    log 'registering host-mode forgejo-runner ...'
    mkdir -p "$RUNNER_DIR"
    _reg_token=$(forgejo_cli actions generate-runner-token | tr -d '[:space:]')
    [ -n "$_reg_token" ] || die 'failed to mint a runner registration token'
    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" register --no-interactive \
        --instance "$BASE_URL" --token "$_reg_token" --name "basic-delivery-$$" --labels host:host ) \
        >"$LOG_DIR/runner-register.log" 2>&1 || die 'forgejo-runner register failed (see logs/runner-register.log)'
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

boot_jig() {
    log "starting jig fake LLM from $JIG_FIXTURE_PATH ..."
    : >"$LOG_DIR/jig.log"
    rm -f "$JIG_STDIN"
    mkfifo "$JIG_STDIN"
    "$JIG_BIN" "$JIG_FIXTURE_PATH" <>"$JIG_STDIN" >"$LOG_DIR/jig.log" 2>&1 &
    JIG_PID=$!
    echo "$JIG_PID" >"$JIG_PID_FILE"

    _i=0
    JIG_URL=
    while [ -z "$JIG_URL" ]; do
        kill -0 "$JIG_PID" 2>/dev/null || die 'jig exited during startup (see logs/jig.log)'
        JIG_URL=$(sed -n 's#^\(http://[^[:space:]]*\).*#\1#p' "$LOG_DIR/jig.log" 2>/dev/null | sed -n '1p')
        [ -n "$JIG_URL" ] && break
        _i=$((_i + 1))
        [ "$_i" -gt 100 ] && die 'jig did not print a base URL (see logs/jig.log)'
        sleep_short
    done
    # DeepSeek appends /chat/completions itself; jig serves that route at this base, so do not add /v1.
    JIG_PROVIDER_URL=$JIG_URL
    log "jig ready at $JIG_URL"
}

bootstrap_admin() {
    log 'bootstrapping throwaway Forgejo site admin ...'
    forgejo_cli admin user create --username "$ADMIN_USER" --password "$ADMIN_PASSWORD" \
        --email "$ADMIN_EMAIL" --admin --must-change-password=false \
        >"$LOG_DIR/admin-create.log" 2>&1 || true
    ADMIN_TOKEN=$(forgejo_cli admin user generate-access-token --username "$ADMIN_USER" --scopes all --raw | tr -d '[:space:]')
    [ -n "$ADMIN_TOKEN" ] || die 'failed to mint an admin access token'
}

run_temper_init() {
    log "running temper --config <bundle-dir> init --apply --yes --non-interactive for $REPO ..."
    : >"$LOG_DIR/init.log"
    : >"$LOG_DIR/provision.log"
    (
        TEMPER_INIT_ADMIN_PASSWORD="$ADMIN_PASSWORD" \
        TEMPER_INIT_PROVIDER_KEY="$INIT_PROVIDER_KEY" \
            "$RUN_BIN" --config "$RUN_DIR" \
                init --non-interactive --force --apply --yes \
                --forge "$BASE_URL" \
                --repo "$REPO" \
                --bind "$DAEMON_BIND" \
                --workspace "$RUN_DIR/workspaces" \
                --admin-user "$ADMIN_USER" \
                --provider deepseek \
                --provider-url "$JIG_PROVIDER_URL"
    ) >"$LOG_DIR/init.log" 2>&1 || die 'temper init failed (see logs/init.log)'

    [ -f "$CONFIG_FILE" ] || die "temper init did not write $CONFIG_FILE"
    [ -f "$CREDENTIALS_FILE" ] || die "temper init did not write $CREDENTIALS_FILE"
    [ -f "$INIT_WORKFLOW_PATH" ] || die "temper init did not write $INIT_WORKFLOW_PATH"
    [ -f "$WEBHOOK_SECRET_FILE" ] || die "temper init did not write $WEBHOOK_SECRET_FILE"

    {
        printf 'repo=%s initialized_by=temper_init_apply config=%s credentials=%s workflow=%s webhook_secret=%s\n' \
            "$REPO" "$CONFIG_FILE" "$CREDENTIALS_FILE" "$INIT_WORKFLOW_PATH" "$WEBHOOK_SECRET_FILE"
        printf 'repo=%s webhook registered url=%s\n' "$REPO" "$WEBHOOK_URL"
        printf 'repo=%s provider=deepseek provider_url=%s fixture=%s\n' "$REPO" "$JIG_PROVIDER_URL" "$JIG_FIXTURE_PATH"
    } >>"$LOG_DIR/provision.log"
    log "temper init --apply wrote config/credentials and registered $WEBHOOK_URL"
}

run_temper_check() {
    log 'running temper check for the generated standalone bundle ...'
    : >"$LOG_DIR/check.log"
    "$RUN_BIN" --config "$RUN_DIR" check --component standalone \
        >"$LOG_DIR/check.log" 2>&1 || die 'temper check failed (see logs/check.log)'
    printf 'repo=%s check=standalone status=ok\n' "$REPO" >>"$LOG_DIR/provision.log"
}

percent_encode() {
    python3 -c 'import sys, urllib.parse; sys.stdout.write(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

populate_repo() {
    _seed_dir="$RUN_DIR/repo-seed"
    _checkout="$_seed_dir/service"
    _creds="$_seed_dir/git-credentials"
    _remote="$BASE_URL/$REPO.git"
    _hostport=${BASE_URL#*://}

    log "creating initial $DEFAULT_BRANCH commit for $REPO ..."
    rm -rf "$_seed_dir"
    mkdir -p "$_checkout"
    ( umask 077; printf 'http://%s:%s@%s\n' "$(percent_encode "$ADMIN_USER")" "$(percent_encode "$ADMIN_TOKEN")" "$_hostport" >"$_creds" )
    : >"$LOG_DIR/repo-populate.log"

    if ! git -C "$_checkout" init -b "$DEFAULT_BRANCH" >>"$LOG_DIR/repo-populate.log" 2>&1; then
        git -C "$_checkout" init >>"$LOG_DIR/repo-populate.log" 2>&1 || die 'git init failed (see logs/repo-populate.log)'
        git -C "$_checkout" checkout -b "$DEFAULT_BRANCH" >>"$LOG_DIR/repo-populate.log" 2>&1 || die 'git checkout failed (see logs/repo-populate.log)'
    fi
    git -C "$_checkout" config user.email "$ADMIN_EMAIL"
    git -C "$_checkout" config user.name 'Basic Delivery Admin'
    git -C "$_checkout" config credential.helper "store --file=$_creds"
    git -C "$_checkout" remote add origin "$_remote"

    mkdir -p "$_checkout/.forgejo/workflows"
    cp "$CONFIG_DIR/ci.yml" "$_checkout/.forgejo/workflows/ci.yml"
    cat >"$_checkout/README.md" <<EOF
# $REPO

Minimal project baseline for the Temper basic-delivery demo.
EOF

    git -C "$_checkout" add README.md .forgejo/workflows/ci.yml
    git -C "$_checkout" commit --quiet -m 'chore: initialize basic-delivery demo repository' >>"$LOG_DIR/repo-populate.log" 2>&1 || die 'git commit failed (see logs/repo-populate.log)'
    git -C "$_checkout" push --quiet --set-upstream origin "HEAD:$DEFAULT_BRANCH" >>"$LOG_DIR/repo-populate.log" 2>&1 || die 'git push failed (see logs/repo-populate.log)'
    printf 'repo=%s initial_commit_branch=%s files=README.md,.forgejo/workflows/ci.yml\n' "$REPO" "$DEFAULT_BRANCH" >>"$LOG_DIR/provision.log"
}

seed_intake() {
    log 'filing the site-admin intake issue after standalone readiness ...'
    _issue_info=$(
        TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" \
        TEMPER_FORGEJO_BASE_URL="$BASE_URL" \
        TEMPER_FORGEJO_OWNER="$OWNER" \
        TEMPER_FORGEJO_REPO="$NAME" \
        TEMPER_INTAKE_TITLE="$INTAKE_TITLE" \
        TEMPER_INTAKE_BODY_PATH="$INTAKE_BODY_PATH" \
            python3 <<'PY'
import json, os, pathlib, sys, urllib.error, urllib.parse, urllib.request

base = os.environ["TEMPER_FORGEJO_BASE_URL"].rstrip("/")
owner = urllib.parse.quote(os.environ["TEMPER_FORGEJO_OWNER"], safe="")
repo = urllib.parse.quote(os.environ["TEMPER_FORGEJO_REPO"], safe="")
body = pathlib.Path(os.environ["TEMPER_INTAKE_BODY_PATH"]).read_text(encoding="utf-8")
payload = json.dumps({"title": os.environ["TEMPER_INTAKE_TITLE"], "body": body}).encode()
request = urllib.request.Request(
    f"{base}/api/v1/repos/{owner}/{repo}/issues",
    data=payload,
    headers={"Accept": "application/json", "Authorization": f"token {os.environ['TEMPER_FORGEJO_ADMIN_TOKEN']}", "Content-Type": "application/json"},
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        issue = json.loads(response.read().decode())
except urllib.error.HTTPError as exc:
    print(exc.read().decode("utf-8", "replace"), file=sys.stderr)
    raise
number = issue.get("number")
if number is None:
    raise SystemExit("Forgejo issue create response lacked number")
print(number)
print(issue.get("html_url") or f"{base}/{owner}/{repo}/issues/{number}")
PY
    ) || die "filing intake issue for $REPO failed"

    _issue=$(printf '%s\n' "$_issue_info" | sed -n '1p')
    _issue_url=$(printf '%s\n' "$_issue_info" | sed -n '2p')
    [ -n "$_issue" ] || die 'intake issue create returned no number'
    [ -n "$_issue_url" ] || _issue_url="$BASE_URL/$REPO/issues/$_issue"
    printf 'repo=%s intake_issue_number=%s intake_issue_url=%s\n' "$REPO" "$_issue" "$_issue_url" >>"$LOG_DIR/provision.log"
    log "created intake issue #$_issue at $_issue_url"
}

boot_run() {
    mkdir -p "$RUN_DIR/workspaces"
    log "starting temper serve standalone at $DAEMON_BIND ..."
    : >"$LOG_DIR/run.log"
    "$RUN_BIN" --config "$RUN_DIR" serve standalone \
        >"$LOG_DIR/run.log" 2>&1 &
    RUN_PID=$!
    echo "$RUN_PID" >"$RUN_PID_FILE"
    wait_for_log_line "$LOG_DIR/run.log" 'webhook listener up' "$RUN_PID" 'temper serve standalone'
    wait_for_log_line "$LOG_DIR/run.log" 'worker:  capacity:' "$RUN_PID" 'temper serve standalone'
    wait_for_log_line "$LOG_DIR/run.log" 'ready -- watching' "$RUN_PID" 'temper serve standalone'
    log "temper serve standalone up (pid $RUN_PID; logs/run.log)"
}

monitor() {
    log ''
    log "Forgejo UI:   $BASE_URL"
    log "temper serve: http://$DAEMON_BIND"
    log "Jig LLM:      ${JIG_URL:-unknown}"
    log "Intake issue: $BASE_URL/$REPO/issues"
    log "Logs:         $LOG_DIR/run.log"
    log "Press Ctrl-C (or run './run.sh stop') to tear everything down."

    _waited=0
    while [ ! -f "$STOP_FILE" ]; do
        sleep 2
        _waited=$((_waited + 2))
        kill -0 "$SERVER_PID" 2>/dev/null || { log 'forgejo server exited; shutting down.'; break; }
        [ "$_waited" -ge "$RUN_SECS" ] && { log "run backstop ($RUN_SECS s) reached; shutting down."; break; }
    done
}

cmd_start() {
    [ -f "$INTAKE_BODY_PATH" ] || die "missing $INTAKE_BODY_PATH"
    [ -f "$CONFIG_DIR/ci.yml" ] || die "missing $CONFIG_DIR/ci.yml"
    resolve_binaries
    if [ -f "$SERVER_PID_FILE" ] && kill -0 "$(cat "$SERVER_PID_FILE" 2>/dev/null)" 2>/dev/null; then
        die "a run appears active; stop it first: ./run.sh stop"
    fi

    mkdir -p "$RUN_DIR" "$LOG_DIR"
    rm -f "$STOP_FILE"
    trap cleanup EXIT INT TERM

    boot_server
    boot_runner
    boot_jig
    bootstrap_admin
    run_temper_init
    run_temper_check
    populate_repo
    boot_run
    seed_intake
    monitor
}

case "${1:-start}" in
    start | "") cmd_start ;;
    stop) cmd_stop ;;
    help | -h | --help) usage ;;
    *) usage >&2; exit 2 ;;
esac
