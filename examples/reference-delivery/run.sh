#!/bin/sh
# Fixed, minimal launcher for the reference-delivery reviewer-gate demo.
#
# It follows the long-term Temper UX: `temper --config <bundle-dir> init
# --apply --yes` writes and provisions a deployment bundle from the checked-in
# reference workflow, `temper --config <bundle-dir> check` validates it, then
# `temper --config <bundle-dir> serve standalone` runs the engine, worker, and
# agent in one process.
# The only direct Forgejo API use is demo setup: create the throwaway admin,
# seed a tiny repository baseline, and file the intake issue after Temper is
# ready so the issue-created webhook drives the wake path.

set -eu

SCRIPT_DIR=$(CDPATH=; cd -- "$(dirname -- "$0")" && pwd)
WORKSPACE_ROOT=$(CDPATH=; cd -- "$SCRIPT_DIR/../.." && pwd)
CONFIG_DIR="$SCRIPT_DIR/config"
RUN_DIR="$SCRIPT_DIR/run"
LOG_DIR="$SCRIPT_DIR/logs"

. "$SCRIPT_DIR/../forgejo-fixture.sh"

BASE_URL=http://127.0.0.1:4200
HOST=127.0.0.1
PORT=4200
DAEMON_BIND=127.0.0.1:38200
WEBHOOK_URL=http://$DAEMON_BIND/forgejo/webhook

OWNER=acme
NAME=service
REPO=$OWNER/$NAME
CANARY_NAME=service-canary
CANARY_REPO=$OWNER/$CANARY_NAME
MULTI_REPOS="$REPO $CANARY_REPO"
DEFAULT_BRANCH=main
INTAKE_TITLE='Service banner should identify the environment'
MULTI_INTAKE_TITLE='Ship cross-repo reference delivery'
INTAKE_BODY_PATH="$CONFIG_DIR/intake-issue.md"
WORKFLOW_PATH="$CONFIG_DIR/workflow.json"

RUN_BIN="$WORKSPACE_ROOT/target/debug/temper"
WORKER_BIN="$WORKSPACE_ROOT/target/debug/temper-testing-worker"
JIG_REPO="$HOME/src/rust/jig"
JIG_BIN="$JIG_REPO/target/debug/jig"
JIG_FIXTURE_PATH="$JIG_REPO/fixtures/reference-delivery.json"
INIT_PROVIDER_KEY=reference-delivery-jig-dummy-key

ADMIN_USER=refadmin
ADMIN_EMAIL=refadmin@example.invalid
ADMIN_PASSWORD='Ref-Delivery-Admin-1!'

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
WORKER_PID_FILE="$RUN_DIR/workers.pid"
JIG_PID_FILE="$RUN_DIR/jig.pid"
JIG_STDIN="$RUN_DIR/jig.stdin"
CONFIG_FILE="$RUN_DIR/config.toml"
CREDENTIALS_FILE="$RUN_DIR/credentials.toml"
INIT_WORKFLOW_PATH="$RUN_DIR/workflow.yaml"
WEBHOOK_SECRET_FILE="$RUN_DIR/webhook-secret"
MULTI_INTAKE_BODY_PATH="$RUN_DIR/cross-repo-intake.md"
MULTI_PARENT_FILE="$RUN_DIR/cross-repo-parent"
MULTI_WORKER_ROOT="$RUN_DIR/testing-worker"

log() { printf '[run.sh] %s\n' "$*"; }
die() { printf '[run.sh] error: %s\n' "$*" >&2; exit 1; }
sleep_short() { sleep 0.2 2>/dev/null || sleep 1; }

usage() {
    cat <<EOF
usage: ./run.sh [start|multi-repo|single-repo|stop|help]

  start (default)      run the cross-repo fan-out demo across $REPO and $CANARY_REPO
  multi-repo           alias for start
  single-repo          run the fixed reviewer-gated single-repo demo
  stop                 tear down a previous run via run/*.pid
  help                 show this message

The default reference-delivery demo intentionally provisions exactly $REPO plus
$CANARY_REPO so one source intake can fan out to two repo-scoped child issues.
The optional single-repo demo is also fixed: repo, ports, cadences, jig fixture,
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
    stop_pid_file "$WORKER_PID_FILE"
    stop_pid_file "$JIG_PID_FILE"
    stop_pid_file "$RUNNER_PID_FILE"
    stop_pid_file "$SERVER_PID_FILE"
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$STOP_FILE" \
        "$RUN_DIR/repo-seed" "$RUN_DIR/workspaces" "$MULTI_WORKER_ROOT" \
        "$CONFIG_FILE" "$CREDENTIALS_FILE" "$INIT_WORKFLOW_PATH" \
        "$WEBHOOK_SECRET_FILE" "$JIG_STDIN" "$MULTI_INTAKE_BODY_PATH" \
        "$MULTI_PARENT_FILE" \
        2>/dev/null || true
    rmdir "$RUN_DIR" 2>/dev/null || true
    log 'teardown complete'
}

cmd_stop() {
    [ -d "$RUN_DIR" ] || { log 'nothing to stop (no run/ dir)'; return 0; }
    cleanup
}

assert_no_active_run() {
    for _file in "$SERVER_PID_FILE" "$RUN_PID_FILE" "$WORKER_PID_FILE" "$RUNNER_PID_FILE" "$JIG_PID_FILE"; do
        [ -f "$_file" ] || continue
        while IFS= read -r _pid; do
            [ -n "$_pid" ] || continue
            if kill -0 "$_pid" 2>/dev/null; then
                die "a run appears active (pid $_pid from $_file); stop it first: ./run.sh stop"
            fi
        done <"$_file"
    done
}

assert_bind_available() {
    _label=$1
    _bind=$2
    python3 - "$_label" "$_bind" <<'PY' || die "fixed bind $_bind is unavailable; stop a previous run with ./run.sh stop or free the port"
import socket
import sys

label, bind = sys.argv[1:3]
host, port_text = bind.rsplit(":", 1)
port = int(port_text)
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind((host, port))
        sock.listen(1)
    except OSError as exc:
        raise SystemExit(f"{label} fixed bind {bind} is unavailable: {exc}")
PY
}

resolve_common_binaries() {
    command -v curl >/dev/null 2>&1 || die 'curl is required'
    command -v git >/dev/null 2>&1 || die 'git is required'
    command -v mkfifo >/dev/null 2>&1 || die 'mkfifo is required'
    command -v python3 >/dev/null 2>&1 || die 'python3 is required'

    resolve_forgejo_fixture "$WORKSPACE_ROOT"
    log 'building temper development binary...'
    ( cd "$WORKSPACE_ROOT" && cargo build -p temper ) || die 'cargo build -p temper failed'
    [ -x "$RUN_BIN" ] || die "temper binary not found: $RUN_BIN"

    _global_help=$("$RUN_BIN" --help 2>&1 || true)
    case "$_global_help" in *--config*) ;; *) die 'temper help lacks global --config' ;; esac
    _init_help=$("$RUN_BIN" init --help 2>&1 || true)
    case "$_init_help" in *--non-interactive*) ;; *) die 'temper init lacks --non-interactive' ;; esac
    case "$_init_help" in *--apply*) ;; *) die 'temper init lacks --apply' ;; esac
    case "$_init_help" in *--yes*) ;; *) die 'temper init lacks --yes' ;; esac
    case "$_init_help" in *--workflow*) ;; *) die 'temper init lacks --workflow' ;; esac
    case "$_init_help" in *--provider-url*) ;; *) die 'temper init lacks --provider-url' ;; esac
    case "$_init_help" in *--workspace*) ;; *) die 'temper init lacks --workspace' ;; esac
    _serve_help=$("$RUN_BIN" serve standalone --help 2>&1 || true)
    case "$_serve_help" in *--config*) die 'temper serve standalone documents non-canonical --config' ;; *) ;; esac
}

resolve_single_binaries() {
    resolve_common_binaries
    [ -d "$JIG_REPO" ] || die "jig checkout not found: $JIG_REPO"
    [ -f "$JIG_FIXTURE_PATH" ] || die "jig fixture not found: $JIG_FIXTURE_PATH"
    log 'building jig development binary...'
    ( cd "$JIG_REPO" && cargo build -p jig ) || die 'cargo build -p jig failed'
    [ -x "$JIG_BIN" ] || die "jig binary not found: $JIG_BIN"
}

resolve_multi_binaries() {
    resolve_common_binaries
    log 'building temper-testing-worker development binary...'
    ( cd "$WORKSPACE_ROOT" && cargo build -p temper-testing --bin temper-testing-worker ) \
        || die 'cargo build -p temper-testing --bin temper-testing-worker failed'
    [ -x "$WORKER_BIN" ] || die "temper-testing-worker binary not found: $WORKER_BIN"
}

write_app_ini() {
    mkdir -p "$FORGEJO_DATA/custom/conf" "$FORGEJO_DATA/data" "$FORGEJO_DATA/log" "$FORGEJO_DATA/repos"
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
    verify_forgejo_fixture_version
    log "Forgejo ready (pid $SERVER_PID)"
}

boot_runner() {
    log 'registering host-mode forgejo-runner ...'
    mkdir -p "$RUNNER_DIR"
    _reg_token=$(forgejo_cli actions generate-runner-token | tr -d '[:space:]')
    [ -n "$_reg_token" ] || die 'failed to mint a runner registration token'
    ( cd "$RUNNER_DIR" && "$RUNNER_BIN" register --no-interactive \
        --instance "$BASE_URL" --token "$_reg_token" --name "reference-delivery-$$" --labels host:host ) \
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

limit_served_roles() {
    python3 - "$CONFIG_FILE" <<'PY'
import pathlib, re, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
text, changed = re.subn(
    r'^roles = \[[^\n]*\]$',
    'roles = ["architect", "engineer", "reviewer"]',
    text,
    count=1,
    flags=re.MULTILINE,
)
if changed != 1:
    raise SystemExit(f"could not find exactly one roles line in {path}")
path.write_text(text, encoding="utf-8")
PY
}

run_temper_init() {
    log "running temper --config <bundle-dir> init --apply --yes --non-interactive for $REPO with the reference workflow ..."
    : >"$LOG_DIR/init.log"
    : >"$LOG_DIR/provision.log"
    (
        TEMPER_INIT_ADMIN_PASSWORD="$ADMIN_PASSWORD" \
        TEMPER_INIT_PROVIDER_KEY="$INIT_PROVIDER_KEY" \
            "$RUN_BIN" --config "$RUN_DIR" \
                init --non-interactive --force --apply --yes \
                --forge "$BASE_URL" \
                --repo "$REPO" \
                --workflow "$WORKFLOW_PATH" \
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
    limit_served_roles

    {
        printf 'repo=%s initialized_by=temper_init_apply config=%s credentials=%s workflow=%s webhook_secret=%s\n' \
            "$REPO" "$CONFIG_FILE" "$CREDENTIALS_FILE" "$INIT_WORKFLOW_PATH" "$WEBHOOK_SECRET_FILE"
        printf 'repo=%s webhook registered url=%s\n' "$REPO" "$WEBHOOK_URL"
        printf 'repo=%s provider=deepseek provider_url=%s fixture=%s\n' "$REPO" "$JIG_PROVIDER_URL" "$JIG_FIXTURE_PATH"
        printf 'repo=%s served_roles=architect,engineer,reviewer\n' "$REPO"
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
    git -C "$_checkout" config user.name 'Reference Delivery Admin'
    git -C "$_checkout" config credential.helper "store --file=$_creds"
    git -C "$_checkout" remote add origin "$_remote"

    mkdir -p "$_checkout/.forgejo/workflows"
    cp "$CONFIG_DIR/ci.yml" "$_checkout/.forgejo/workflows/ci.yml"
    cat >"$_checkout/README.md" <<EOF
# $REPO

Minimal project baseline for the Temper reference-delivery demo.
EOF

    git -C "$_checkout" add README.md .forgejo/workflows/ci.yml
    git -C "$_checkout" commit --quiet -m 'chore: initialize reference-delivery demo repository' >>"$LOG_DIR/repo-populate.log" 2>&1 || die 'git commit failed (see logs/repo-populate.log)'
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
    log 'Expected path: intake -> architect triage -> engineer PR -> reviewer approval -> bot merge.'
    log "Press Ctrl-C (or run './run.sh stop') to tear everything down."

    _waited=0
    while [ ! -f "$STOP_FILE" ]; do
        sleep 2
        _waited=$((_waited + 2))
        kill -0 "$SERVER_PID" 2>/dev/null || { log 'forgejo server exited; shutting down.'; break; }
        [ "$_waited" -ge "$RUN_SECS" ] && { log "run backstop ($RUN_SECS s) reached; shutting down."; break; }
    done
}

credential_field() {
    _role=$1
    _field=$2
    python3 - "$CREDENTIALS_FILE" "$_role" "$_field" <<'PY'
import sys
try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit("python3 with tomllib is required to read credentials.toml")
path, role, field = sys.argv[1:4]
with open(path, "rb") as fh:
    data = tomllib.load(fh)
try:
    user = data["forge"]["users"][role]
except KeyError:
    raise SystemExit(f"missing [forge.users.{role}] in {path}")
value = user.get(field)
if field == "user" and (not isinstance(value, str) or not value.strip()):
    value = role
if not isinstance(value, str) or not value.strip():
    raise SystemExit(f"empty [forge.users.{role}] {field} in {path}")
print(value)
PY
}

write_cross_repo_intake_body() {
    mkdir -p "$RUN_DIR"
    cat >"$MULTI_INTAKE_BODY_PATH" <<EOF
A human asks Temper to coordinate one change across two repositories without
filing duplicate parent intakes. Start from this single parent in $REPO, fan out
one ready code child per target repository, and let the parent resolve only
after both child implementations have landed.

Target repositories:

- $REPO
- $CANARY_REPO

The deterministic reference-delivery architect reads the plan block below and
must create exactly these child code issues.

<!-- temper:architect-plan
{
  "children": [
    {
      "slug": "service",
      "target_repo": "forgejo:$REPO",
      "title": "Implement service-side cross-repo reference delivery",
      "body": "Add a small service-side reference-delivery change so the implementation PR has a real product diff."
    },
    {
      "slug": "canary",
      "target_repo": "forgejo:$CANARY_REPO",
      "title": "Implement canary-side cross-repo reference delivery",
      "body": "Add a small canary-side reference-delivery change so the implementation PR has a real product diff."
    }
  ]
}
-->
EOF
}

provision_multi_repo() {
    log "provisioning fixed multi-repo world: $MULTI_REPOS ..."
    : >"$LOG_DIR/init.log"
    : >"$LOG_DIR/provision.log"
    for _repo in $MULTI_REPOS; do
        _owner=${_repo%%/*}
        _name=${_repo#*/}
        (
            TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" \
                "$RUN_BIN" provision-forgejo \
                    --base-url "$BASE_URL" \
                    --owner "$_owner" \
                    --name "$_name" \
                    --out "$CREDENTIALS_FILE" \
                    --workflow "$WORKFLOW_PATH" \
                    --seed-intake no
        ) >>"$LOG_DIR/init.log" 2>&1 || die "provisioning $_repo failed (see logs/init.log)"
        printf 'repo=%s provisioned_by=temper_provision_forgejo credentials=%s workflow=%s intake_seeded=no\n' \
            "$_repo" "$CREDENTIALS_FILE" "$WORKFLOW_PATH" >>"$LOG_DIR/provision.log"
    done
    printf 'source_repo=%s target_repos=%s expected_children=2\n' "$REPO" "$MULTI_REPOS" >>"$LOG_DIR/provision.log"
    [ -f "$CREDENTIALS_FILE" ] || die "multi-repo provisioning did not write $CREDENTIALS_FILE"
    log 'multi-repo repositories, labels, role users, CI, and credentials are provisioned'
}

start_role_worker() {
    _role=$1
    _token=$(credential_field "$_role" token) || die "cannot read token for role $_role"
    _username=$(credential_field "$_role" user) || die "cannot read username for role $_role"
    _log="$LOG_DIR/worker-$_role.log"
    : >"$_log"
    _architect_args=
    [ "$_role" = architect ] && _architect_args='--architect closing'
    log "starting multi-repo $_role worker ..."
    (
        TEMPER_FORGEJO_TOKEN="$_token" \
            "$WORKER_BIN" \
                --kind role \
                --role "$_role" \
                --user "$_username" \
                --backend forgejo \
                --base-url "$BASE_URL" \
                --clock wall \
                --root "$MULTI_WORKER_ROOT" \
                --repo "$REPO" \
                --repo "$CANARY_REPO" \
                --workflow "$WORKFLOW_PATH" \
                --poll-ms 500 \
                --audit-ms 2000 \
                --stop-file "$STOP_FILE" \
                --run-secs "$RUN_SECS" \
                $_architect_args
    ) >"$_log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKER_PID_FILE"
    log "$_role worker running (pid $_pid; $_log)"
}

start_mechanical_worker() {
    _token=$(credential_field bot token) || die 'cannot read token for bot'
    _log="$LOG_DIR/worker-mechanical.log"
    : >"$_log"
    log 'starting multi-repo mechanical worker ...'
    (
        TEMPER_FORGEJO_TOKEN="$_token" \
            "$WORKER_BIN" \
                --kind mechanical \
                --backend forgejo \
                --base-url "$BASE_URL" \
                --clock wall \
                --root "$MULTI_WORKER_ROOT" \
                --repo "$REPO" \
                --repo "$CANARY_REPO" \
                --workflow "$WORKFLOW_PATH" \
                --poll-ms 500 \
                --idle-poll-max-ms 1000 \
                --audit-ms 2000 \
                --stop-file "$STOP_FILE" \
                --run-secs "$RUN_SECS"
    ) >"$_log" 2>&1 &
    _pid=$!
    echo "$_pid" >>"$WORKER_PID_FILE"
    log "mechanical worker running (pid $_pid; $_log)"
}

boot_multi_workers() {
    rm -f "$WORKER_PID_FILE"
    mkdir -p "$MULTI_WORKER_ROOT"
    start_mechanical_worker
    start_role_worker architect
    start_role_worker engineer
    start_role_worker reviewer
    printf 'repo_set="%s" workers=mechanical,architect,engineer,reviewer architect=closing reviewer=default ci=forgejo-runner\n' \
        "$MULTI_REPOS" >>"$LOG_DIR/provision.log"
}

seed_multi_intake() {
    write_cross_repo_intake_body
    log 'filing one cross-repo parent intake in the source repo after workers are running ...'
    _old_body=$INTAKE_BODY_PATH
    _old_title=$INTAKE_TITLE
    INTAKE_BODY_PATH=$MULTI_INTAKE_BODY_PATH
    INTAKE_TITLE=$MULTI_INTAKE_TITLE
    seed_intake
    INTAKE_BODY_PATH=$_old_body
    INTAKE_TITLE=$_old_title
    _parent=$(sed -n 's/.*intake_issue_number=\([0-9][0-9]*\).*/\1/p' "$LOG_DIR/provision.log" | tail -n 1)
    [ -n "$_parent" ] || die 'could not determine cross-repo parent issue number from provision.log'
    echo "$_parent" >"$MULTI_PARENT_FILE"
    printf 'repo=%s cross_repo_parent_issue_number=%s expected_children=2 target_repos="%s"\n' \
        "$REPO" "$_parent" "$MULTI_REPOS" >>"$LOG_DIR/provision.log"
}

monitor_multi() {
    log ''
    log "Forgejo UI:   $BASE_URL"
    log "Source issue: $BASE_URL/$REPO/issues"
    log "Canary repo:  $BASE_URL/$CANARY_REPO"
    log "Logs:         $LOG_DIR/worker-*.log and $LOG_DIR/runner.log"
    log 'Expected path: one source parent -> two child code issues -> reviewer-approved, green PRs -> child PRs merge -> parent closes.'
    log "Press Ctrl-C (or run './run.sh stop') to tear everything down."

    _waited=0
    while [ ! -f "$STOP_FILE" ]; do
        sleep 5
        _waited=$((_waited + 5))
        kill -0 "$SERVER_PID" 2>/dev/null || { log 'forgejo server exited; shutting down.'; break; }
        [ "$_waited" -ge "$RUN_SECS" ] && { log "run backstop ($RUN_SECS s) reached; shutting down."; break; }
    done
}

cmd_multi_repo() {
    [ -f "$WORKFLOW_PATH" ] || die "missing $WORKFLOW_PATH"
    resolve_multi_binaries
    assert_no_active_run
    assert_bind_available 'Forgejo' "$HOST:$PORT"

    mkdir -p "$RUN_DIR" "$LOG_DIR"
    rm -f "$STOP_FILE" "$MULTI_PARENT_FILE"
    trap cleanup EXIT INT TERM

    boot_server
    boot_runner
    bootstrap_admin
    provision_multi_repo
    boot_multi_workers
    seed_multi_intake
    monitor_multi
}

cmd_start() {
    cmd_multi_repo
}

cmd_single_repo() {
    [ -f "$INTAKE_BODY_PATH" ] || die "missing $INTAKE_BODY_PATH"
    [ -f "$WORKFLOW_PATH" ] || die "missing $WORKFLOW_PATH"
    [ -f "$CONFIG_DIR/ci.yml" ] || die "missing $CONFIG_DIR/ci.yml"
    resolve_single_binaries
    assert_no_active_run
    assert_bind_available 'Forgejo' "$HOST:$PORT"
    assert_bind_available 'temper serve standalone' "$DAEMON_BIND"

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
    multi-repo) cmd_multi_repo ;;
    single-repo) cmd_single_repo ;;
    stop) cmd_stop ;;
    help | -h | --help) usage ;;
    *) usage >&2; exit 2 ;;
esac
