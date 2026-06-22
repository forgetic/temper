#!/bin/sh
# Reference-delivery example — POSIX launcher / teardown.
#
# Boots the production topology from development-profile binaries, but as a
# SINGLE process:
#   1. a throwaway Forgejo server (SQLite, Actions enabled),
#   2. a host-mode forgejo-runner producing real CI,
#   3. admin bootstrap + the production provision binary against the bundled
#      reference workflow for every configured repo, registering the webhook but
#      deliberately NOT yet filing intake,
#   4. one `temper run`: the unified daemon + worker + coding agent on ONE event
#      loop. The daemon hosts the webhook route, the long poll backstop, the short
#      mechanical CI/landing backstop, leases, per-role apply tokens, cross-repo
#      child materialisation, and result appliers; the in-process worker drives
#      the in-process coding agent for architect, engineer, and reviewer across
#      every configured repo,
#   5. only once `temper run` is ready, a second seed-only provision pass files
#      the human-authored intake issue(s), so issue-created webhooks demonstrate
#      the wake path.
#
# The default single-repo path converges through architect triage, engineer PR,
# reviewer approval, landing, and mechanical bot merge. In multi-repo mode the
# cross-repo intake can fan out into child code issues in their target repos; the
# owner and human roles remain workflow-declared but unserved by this launcher.
#
# This script targets the operator-facing `temper` entry point. By default it
# builds/uses the development binary under target/debug; override TEMPER_RUN_BIN
# for a prebuilt or release artifact.
#
# Usage:
#   ./run.sh [start]          boot everything and block until Ctrl-C / stop-file
#   ./run.sh validate-webhooks inspect logs from a running/completed run
#   ./run.sh validate-multi-repo inspect provisioning + run logs and the
#                                cross-repo Forge state
#   ./run.sh stop             tear down a previous run via the saved PIDs
#   ./run.sh help             show this usage
#
# Orphan cleanup (lesson 0009) — if a run is force-killed (SIGKILL) the Drop/
# trap guards do not fire; clean up survivors by hand with:
#       pkill -f forgejo
#       pkill -f forgejo-runner
#       pkill -f 'target/debug/temper'
#       rm -rf examples/reference-delivery/run
#
# POSIX sh only (no bashisms). Validate with `sh -n run.sh` (and shellcheck).
# Secrets travel by env or the sourced secrets files, NEVER on a command line.

set -eu

# --- Locations ----------------------------------------------------------------
if [ -n "${TEMPER_REFERENCE_DELIVERY_SCRIPT_DIR:-}" ]; then
    SCRIPT_DIR=$TEMPER_REFERENCE_DELIVERY_SCRIPT_DIR
else
    SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
fi
WORKSPACE_ROOT=${TEMPER_WORKSPACE_ROOT:-$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd)}
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
# The provisioner writes the live forge identities in the runtime's own
# credentials.toml format ([forge.users.<role>] + a bot user). The daemon loads
# it via `temper daemon --secrets`; the launcher only reads a couple of
# fields back (the bot identity for the mechanical backstop, the engineer for the
# demo CI seed, and the architect read token for the Forge-state validator) —
# never on argv.
CREDENTIALS_FILE="$SECRETS_DIR/credentials.toml"
WEBHOOK_SECRET_FILE="$SECRETS_DIR/webhook-secret"

# Pinned versions for the bundled throwaway server/runner used by this example.
FORGEJO_VERSION=7.0.12
FORGEJO_RUNNER_VERSION=3.5.1

# Throwaway admin identity. The workflow's intake_author is the provisioned
# `human` role, not this setup-only admin. This server is killed + wiped on
# teardown; the credential never reaches anything real and is never echoed.
ADMIN_USER=refadmin
ADMIN_EMAIL=refadmin@example.invalid
ADMIN_PASSWORD='Ref-Delivery-Admin-1!'

# Diagnostic strings emitted when Forgejo 7.0.x Actions status cannot be read by
# the ADR-0019 web-UI fallback. `temper run` hosts the mechanical CI-read path.
CI_FALLBACK_MISSING_CREDENTIALS='no web-UI credentials configured for the CI read fallback'
CI_FALLBACK_LOGIN_FAILED='forgejo web-ui login failed'

log() { printf '[run.sh] %s\n' "$*"; }
die() { printf '[run.sh] error: %s\n' "$*" >&2; exit 1; }

sleep_short() {
    sleep 0.2 2>/dev/null || sleep 1
}

DISPLAY_SCRIPT=${TEMPER_REFERENCE_DELIVERY_ORIGINAL:-$SCRIPT_DIR/run.sh}

# Dash reads long-running scripts lazily. If this file is edited while the demo
# is sleeping in monitor(), the running shell may parse a half-new tail and fail
# during teardown. Run starts from a private snapshot so source edits/rebuilds do
# not affect the already-running launcher.
if [ "${TEMPER_REFERENCE_DELIVERY_SNAPSHOT:-0}" != "1" ]; then
    case "${1:-start}" in
        start | "")
            mkdir -p "$RUN_DIR"
            _snapshot="$RUN_DIR/run.sh.snapshot.$$"
            cp "$SCRIPT_DIR/run.sh" "$_snapshot"
            chmod 700 "$_snapshot"
            TEMPER_REFERENCE_DELIVERY_SNAPSHOT=1 \
            TEMPER_REFERENCE_DELIVERY_SCRIPT_DIR="$SCRIPT_DIR" \
            TEMPER_REFERENCE_DELIVERY_ORIGINAL="$DISPLAY_SCRIPT" \
                exec /bin/sh "$_snapshot" "$@"
            ;;
    esac
fi

usage() {
    cat <<EOF
usage: $DISPLAY_SCRIPT [start|validate-webhooks|validate-multi-repo|stop|help]

  start (default)      boot Forgejo + runner, provision every configured repo
                       against the bundled workflow, launch one \`temper run\`
                       (daemon + worker + coding agent for architect + engineer +
                       reviewer), then file intake so webhooks wake the run.
  validate-webhooks    inspect logs/ and report whether the webhook was
                       registered, accepted, scanned, assigned, and completed.
  validate-multi-repo  additionally require every configured repo to appear in
                       provisioning, daemon assignment, worker assignment, and
                       (for cross-repo intake) the live Forge dependency state.
  stop                 tear down a previous run via run/*.pid.
  help                 show this message.

Configuration is read from config/temper.env (no secrets). The default coding
agent provider is the local jig fake LLM; set TEMPER_RUN_AUTH to a real provider
only when intentionally using credentials from secrets/.env (gitignored).
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
    # Drop throwaway server/runner data + runtime checkouts + sentinel so a
    # re-run starts fresh; keep logs/ for inspection.
    rm -rf "$FORGEJO_DATA" "$RUNNER_DIR" "$STOP_FILE" \
        "$RUN_DIR/ci-seed" "$RUN_DIR/workspaces" \
        "$RUN_DIR/cross-repo-intake.md" "$RUN_DIR"/run.sh.snapshot.* \
        "$JIG_STDIN" \
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
CONFIG_KNOBS="OWNER NAME REPOS DEFAULT_BRANCH WORKFLOW_FILE SERVED_ROLES INTAKE_TITLE INTAKE_BODY_FILE \
CROSS_REPO_INTAKE CROSS_REPO_INTAKE_TITLE BASE_URL DAEMON_BIND WEBHOOK_URL \
DAEMON_POLL_CADENCE_SECS DAEMON_MECHANICAL_CADENCE_SECS DAEMON_LEASE_TTL_SECS RUN_SECS \
TEMPER_RUN_AUTH RUN_MAX_ITERATIONS \
TEMPER_FORGEJO_GOMAXPROCS TEMPER_FORGEJO_BINARY TEMPER_FORGEJO_RUNNER_BINARY \
TEMPER_RUN_BIN TEMPER_BUILD_PACKAGE \
JIG_REPO JIG_BIN JIG_FIXTURE_PATH"

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
    # REPOS is special: an intentionally empty `REPOS=` selects OWNER/NAME mode,
    # so presence matters even when the value is empty.
    _repos_was_set=${REPOS+x}
    _pre_REPOS_VALUE=${REPOS-}
    for _k in $CONFIG_KNOBS; do
        eval "_pre_$_k=\${$_k:-}"
    done
    # shellcheck disable=SC1090,SC1091
    . "$CONFIG_DIR/temper.env"
    # Optional operator secret overrides (gitignored). This is where the coding
    # agent's provider credentials live (e.g. TEMPER_DEEPSEEK_API_KEY); they are
    # exported so the single `temper run` process inherits them.
    if [ -f "$SECRETS_DIR/.env" ]; then
        set -a
        # shellcheck disable=SC1090,SC1091
        . "$SECRETS_DIR/.env"
        set +a
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
    DEFAULT_BRANCH=${DEFAULT_BRANCH:-main}
    WORKFLOW_FILE=${WORKFLOW_FILE:-workflow.json}
    SERVED_ROLES=${SERVED_ROLES:-architect engineer reviewer}
    INTAKE_TITLE=${INTAKE_TITLE:-Service banner should identify the environment}
    INTAKE_BODY_FILE=${INTAKE_BODY_FILE:-intake-issue.md}
    CROSS_REPO_INTAKE=${CROSS_REPO_INTAKE:-auto}
    CROSS_REPO_INTAKE_TITLE=${CROSS_REPO_INTAKE_TITLE:-Coordinate greeting across service and canary}
    BASE_URL=${BASE_URL:-http://127.0.0.1:4200}
    DAEMON_BIND=${DAEMON_BIND:-127.0.0.1:38200}
    WEBHOOK_URL=${WEBHOOK_URL:-http://$DAEMON_BIND/forgejo/webhook}
    DAEMON_POLL_CADENCE_SECS=${DAEMON_POLL_CADENCE_SECS:-120}
    DAEMON_MECHANICAL_CADENCE_SECS=${DAEMON_MECHANICAL_CADENCE_SECS:-2}
    DAEMON_LEASE_TTL_SECS=${DAEMON_LEASE_TTL_SECS:-300}
    RUN_SECS=${RUN_SECS:-600}
    # Coding-agent LLM provider. The checked-in demo defaults to jig, a local
    # fake LLM served through Temper's DeepSeek-compatible provider path. Real
    # providers remain available as an explicit opt-in and require credentials in
    # secrets/.env or the provider's normal auth file.
    TEMPER_RUN_AUTH=${TEMPER_RUN_AUTH:-jig}
    case "$TEMPER_RUN_AUTH" in
        jig | deepseek | chatgpt-oauth | anthropic-oauth) ;;
        *) die "TEMPER_RUN_AUTH must be jig|deepseek|chatgpt-oauth|anthropic-oauth, got '$TEMPER_RUN_AUTH'" ;;
    esac
    RUN_MAX_ITERATIONS=${RUN_MAX_ITERATIONS:-250}
    TEMPER_FORGEJO_GOMAXPROCS=${TEMPER_FORGEJO_GOMAXPROCS:-2}
    TEMPER_FORGEJO_BINARY=${TEMPER_FORGEJO_BINARY:-}
    TEMPER_FORGEJO_RUNNER_BINARY=${TEMPER_FORGEJO_RUNNER_BINARY:-}
    TEMPER_RUN_BIN=${TEMPER_RUN_BIN:-}
    TEMPER_BUILD_PACKAGE=${TEMPER_BUILD_PACKAGE:-temper}
    JIG_REPO=${JIG_REPO:-${HOME:-}/src/rust/jig}
    JIG_BIN=${JIG_BIN:-$JIG_REPO/target/debug/jig}
    JIG_FIXTURE_PATH=${JIG_FIXTURE_PATH:-$CONFIG_DIR/jig-reference-delivery.json}

    require_positive_int DAEMON_POLL_CADENCE_SECS "$DAEMON_POLL_CADENCE_SECS"
    require_positive_int DAEMON_MECHANICAL_CADENCE_SECS "$DAEMON_MECHANICAL_CADENCE_SECS"
    require_positive_int DAEMON_LEASE_TTL_SECS "$DAEMON_LEASE_TTL_SECS"
    require_positive_int RUN_SECS "$RUN_SECS"
    require_positive_int RUN_MAX_ITERATIONS "$RUN_MAX_ITERATIONS"

    CONFIGURED_ROLES=
    for _role in $SERVED_ROLES; do
        [ -n "$_role" ] || continue
        case " $CONFIGURED_ROLES " in
            *" $_role "*) continue ;;
        esac
        CONFIGURED_ROLES="${CONFIGURED_ROLES:+$CONFIGURED_ROLES }$_role"
    done
    [ -n "$CONFIGURED_ROLES" ] || die 'SERVED_ROLES must name at least one workflow role'
    SERVED_ROLES=$CONFIGURED_ROLES

    _raw_repos=${REPOS:-$OWNER/$NAME}
    CONFIGURED_REPOS=
    FIRST_CONFIGURED_REPO=
    REPO_COUNT=0
    for _repo in $_raw_repos; do
        validate_repo_path "$_repo"
        case " $CONFIGURED_REPOS " in
            *" $_repo "*) continue ;;
        esac
        CONFIGURED_REPOS="${CONFIGURED_REPOS:+$CONFIGURED_REPOS }$_repo"
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
    # One unified binary provides everything this example needs: `temper daemon`
    # (engine + worker + agent), the `temper provision-forgejo` subcommand, and
    # the `temper validate-reference-delivery` cross-repo Forge-state validator.
    RUN_BIN=${TEMPER_RUN_BIN:-$WORKSPACE_ROOT/target/debug/temper}

    # Keep the demo entry point self-healing after source changes. Cargo is a
    # cheap no-op when the development binaries are already current; skipping
    # this is an explicit operator choice for prebuilt/current binaries.
    if [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
        log "ensuring the Temper development binary is current (cargo build -p $TEMPER_BUILD_PACKAGE)..."
        ( cd "$WORKSPACE_ROOT" && cargo build -p "$TEMPER_BUILD_PACKAGE" ) \
            || die 'Temper cargo build failed'
    fi

    [ -x "$RUN_BIN" ] || die "temper binary not found: $RUN_BIN"

    # This example requires the runtime workflow provisioner subcommand and the
    # config-driven `temper daemon` command (engine + worker + agent in one
    # process). Refuse to run against a stale development binary.
    _provision_help=$("$RUN_BIN" provision-forgejo --help 2>&1 || true)
    case "$_provision_help" in
        *--workflow*--seed-intake*--seed-only*) ;;
        *) die "temper binary is stale or incompatible: $RUN_BIN 'provision-forgejo' does not advertise --workflow/--seed-intake/--seed-only. Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
    esac
    _daemon_help=$("$RUN_BIN" daemon --help 2>&1 || true)
    case "$_daemon_help" in
        *--config*) ;;
        *) die "temper binary is stale or incompatible: $RUN_BIN 'daemon' does not advertise --config. Re-run without TEMPER_SKIP_BUILD=1 or rebuild with cargo build -p $TEMPER_BUILD_PACKAGE." ;;
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

    if [ "$TEMPER_RUN_AUTH" = "jig" ]; then
        command -v mkfifo >/dev/null 2>&1 || die 'mkfifo is required for jig stdin management'
        [ -f "$JIG_FIXTURE_PATH" ] || die "jig fixture not found: $JIG_FIXTURE_PATH"
        if [ ! -x "$JIG_BIN" ] || [ "${TEMPER_SKIP_BUILD:-0}" != "1" ]; then
            [ -d "$JIG_REPO" ] || die "jig checkout not found: $JIG_REPO"
            log 'ensuring the jig fake-LLM binary is current (cargo build -p jig)...'
            ( cd "$JIG_REPO" && cargo build -p jig ) || die 'jig cargo build failed'
        fi
        [ -x "$JIG_BIN" ] || die "jig binary not found: $JIG_BIN"
        log "coding agent: in-process (temper daemon; provider=jig via $JIG_BIN)"
    else
        log "coding agent: in-process (temper daemon; provider from TEMPER_RUN_AUTH=$TEMPER_RUN_AUTH)"
    fi
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
        --name "reference-delivery-$$" --labels host:host ) \
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

# Reads one `[forge.users.<key>]` field from the credentials.toml the
# provisioner wrote. `$1` is the user/role key, `$2` the field name
# (`user`/`token`/`password`/`email`). Prints the unquoted value, or nothing if
# the section/field is absent (e.g. `user` is omitted when it equals the key).
# POSIX awk only; values never contain embedded quotes in practice.
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

assert_served_roles_provisioned() {
    [ -f "$CREDENTIALS_FILE" ] || die "missing $CREDENTIALS_FILE"
    for _role in $SERVED_ROLES; do
        _token=$(toml_forge_user_field "$_role" token)
        [ -n "$_token" ] || die "served role '$_role' has no token in $CREDENTIALS_FILE"
    done
}

write_cross_repo_intake_body() {
    _body_file="$RUN_DIR/cross-repo-intake.md"
    {
        printf 'As an operator I want one visible greeting change coordinated across these repositories:\n\n'
        for _repo in $CONFIGURED_REPOS; do
            _slug=$(repo_slug "$_repo")
            printf -- "- \`%s\` (\`target_repo\`: \`%s\`, child \`slug\`: \`%s\`)\n" "$_repo" "$_repo" "$_slug"
        done
        printf '\nArchitect guidance: triage this intake with one child code issue per repository listed above. '
        printf "Use the exact \`target_repo\` and stable \`slug\` values shown, and keep each child scoped to its repository. "
        printf 'The parent issue should remain blocked until every child issue lands.\n\n'
        printf 'Acceptance: each repository receives its own implementation PR, CI passes, review approves, and all child PRs merge before this parent resolves.\n'
    } >"$_body_file"
    printf '%s\n' "$_body_file"
}

bootstrap_and_provision() {
    log 'bootstrapping admin + provisioning every configured repo against the bundled workflow (intake held back) ...'
    # Create the admin (tolerate a pre-existing one on a re-run), then mint an
    # all-scoped token. The token stays in a shell variable; it is never echoed
    # and reaches the provision steps only via the environment. This pass runs
    # with --seed-intake no: it sets up org/users/repo/labels/CI and registers
    # webhooks but does NOT file intake, so `temper run` can come up first.
    forgejo_cli admin user create --username "$ADMIN_USER" --password "$ADMIN_PASSWORD" \
        --email "$ADMIN_EMAIL" --admin --must-change-password=false \
        >"$LOG_DIR/admin-create.log" 2>&1 || true
    ADMIN_TOKEN=$(forgejo_cli admin user generate-access-token --username "$ADMIN_USER" \
        --scopes all --raw | tr -d '[:space:]')
    [ -n "$ADMIN_TOKEN" ] || die 'failed to mint an admin access token'

    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    : >"$LOG_DIR/provision.log"

    for _repo in $CONFIGURED_REPOS; do
        _owner=$(repo_owner "$_repo")
        _name=$(repo_name "$_repo")
        log "provisioning $_repo (labels + CI + webhook; intake filed after temper run readiness) ..."
        _status=$(TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$RUN_BIN" provision-forgejo \
            --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$CREDENTIALS_FILE" \
            --workflow "$WORKFLOW_PATH" --seed-intake no \
            --webhook-url "$WEBHOOK_URL" --webhook-secret-file "$WEBHOOK_SECRET_FILE") \
            || die "provisioning $_repo failed"

        {
            printf 'repo=%s %s\n' "$_repo" "$_status"
            printf 'repo=%s webhook registered url=%s\n' "$_repo" "$WEBHOOK_URL"
        } >>"$LOG_DIR/provision.log"
        log "$_status"
        log "  webhook registered for $_repo ($WEBHOOK_URL)"
    done

    [ -f "$CREDENTIALS_FILE" ] || die "provision did not write $CREDENTIALS_FILE"
    assert_served_roles_provisioned
}

# Files intake issue(s) AFTER `temper run` is up. This is a second, seed-only
# provision pass (--seed-only): org/users/repo/labels/CI and webhooks already
# exist, so the issue creation webhook demonstrates the wake path.
seed_intake() {
    [ -n "${ADMIN_TOKEN:-}" ] || die 'seed_intake: no admin token (bootstrap_and_provision must run first)'

    if [ "$CROSS_REPO_ENABLED" = "1" ]; then
        _cross_body=$(write_cross_repo_intake_body)
        log "filing one cross-repo parent intake in $FIRST_CONFIGURED_REPO now that temper run is ready ..."
        for _repo in $CONFIGURED_REPOS; do
            if [ "$_repo" != "$FIRST_CONFIGURED_REPO" ]; then
                printf 'repo=%s no_intake_seeded=cross-repo-target\n' "$_repo" >>"$LOG_DIR/provision.log"
                log "  $_repo: no duplicate intake (cross-repo target)"
                continue
            fi
            _owner=$(repo_owner "$_repo")
            _name=$(repo_name "$_repo")
            _status=$(TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$RUN_BIN" provision-forgejo \
                --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$CREDENTIALS_FILE" \
                --workflow "$WORKFLOW_PATH" --seed-only \
                --intake-title "$CROSS_REPO_INTAKE_TITLE" --intake-body-file "$_cross_body") \
                || die "seeding cross-repo intake issue for $_repo failed"
            _issue=$(printf '%s\n' "$_status" | sed -n 's/.*intake issue #\([0-9][0-9]*\).*/\1/p')
            {
                printf 'repo=%s %s\n' "$_repo" "$_status"
                [ -n "$_issue" ] && printf 'repo=%s intake_issue_url=%s/%s/issues/%s\n' "$_repo" "$BASE_URL" "$_repo" "$_issue"
                [ -n "$_issue" ] && printf 'repo=%s cross_repo_parent_url=%s/%s/issues/%s\n' "$_repo" "$BASE_URL" "$_repo" "$_issue"
            } >>"$LOG_DIR/provision.log"
            log "$_status"
            [ -n "$_issue" ] && log "  cross-repo parent issue: $BASE_URL/$_repo/issues/$_issue (filing it should drive the webhook path)"
        done
        return 0
    fi

    log 'filing per-repo human intake issues now that temper run is ready ...'
    for _repo in $CONFIGURED_REPOS; do
        _owner=$(repo_owner "$_repo")
        _name=$(repo_name "$_repo")
        _status=$(TEMPER_FORGEJO_ADMIN_TOKEN="$ADMIN_TOKEN" "$RUN_BIN" provision-forgejo \
            --base-url "$BASE_URL" --owner "$_owner" --name "$_name" --out "$CREDENTIALS_FILE" \
            --workflow "$WORKFLOW_PATH" --seed-only \
            --intake-title "$INTAKE_TITLE" --intake-body-file "$INTAKE_BODY_PATH") \
            || die "seeding intake issue for $_repo failed"
        _issue=$(printf '%s\n' "$_status" | sed -n 's/.*intake issue #\([0-9][0-9]*\).*/\1/p')
        {
            printf 'repo=%s %s\n' "$_repo" "$_status"
            [ -n "$_issue" ] && printf 'repo=%s intake_issue_url=%s/%s/issues/%s\n' "$_repo" "$BASE_URL" "$_repo" "$_issue"
        } >>"$LOG_DIR/provision.log"
        log "$_status"
        [ -n "$_issue" ] && log "  intake issue: $BASE_URL/$_repo/issues/$_issue (filing it should drive the webhook path)"
    done
}

# --- Demo CI seed -------------------------------------------------------------

# URL-encodes one value for a git-credentials store entry. python3 is already a
# demo dependency (the bundled CI workflow runs it via actions/checkout).
percent_encode() {
    python3 -c 'import sys, urllib.parse; sys.stdout.write(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

# Replaces the provisioned commit-message-marker CI with the bundled pass-through
# workflow so real coder PR heads (ordinary commit messages) clear the landing CI
# gate. Non-fatal: if this setup fails the topology still boots, but landing CI
# may not pass.
apply_demo_ci() {
    # The engineer `user` may be omitted from credentials.toml when it equals the
    # section key, so fall back to the role name (`engineer`).
    _eng_user=$(toml_forge_user_field engineer user)
    [ -n "$_eng_user" ] || _eng_user=engineer
    _eng_password=$(toml_forge_user_field engineer password)
    if [ -z "$_eng_password" ]; then
        log "demo CI seed: no engineer password in $CREDENTIALS_FILE; landing CI may not pass"
        return 0
    fi

    _seed_dir="$RUN_DIR/ci-seed"
    _creds="$_seed_dir/git-credentials"
    _without_scheme=${BASE_URL#*://}
    rm -rf "$_seed_dir"
    mkdir -p "$_seed_dir"
    ( umask 077; printf 'http://%s:%s@%s\n' "$(percent_encode "$_eng_user")" "$(percent_encode "$_eng_password")" "$_without_scheme" >"$_creds" )

    for _repo in $CONFIGURED_REPOS; do
        _checkout="$_seed_dir/$(repo_slug "$_repo")"
        _remote="$BASE_URL/$_repo.git"
        log "demo CI seed: cloning $_repo to apply bundled CI ..."
        if ! git -c credential.helper="store --file=$_creds" clone --quiet "$_remote" "$_checkout" >>"$LOG_DIR/ci-seed.log" 2>&1; then
            log "demo CI seed: clone of $_repo failed (see logs/ci-seed.log); landing CI may not pass"
            continue
        fi
        if ! { git -C "$_checkout" config user.email "$_eng_user@example.invalid" \
            && git -C "$_checkout" config user.name 'Temper Engineer' \
            && git -C "$_checkout" config credential.helper "store --file=$_creds"; }; then
            log "demo CI seed: could not configure the checkout for $_repo; landing CI may not pass"
            continue
        fi

        _base=$(git -C "$_checkout" rev-parse --abbrev-ref HEAD 2>/dev/null || printf '%s' "$DEFAULT_BRANCH")
        mkdir -p "$_checkout/.forgejo/workflows"
        if cp "$CONFIG_DIR/ci.yml" "$_checkout/.forgejo/workflows/ci.yml" \
            && ! git -C "$_checkout" diff --quiet -- .forgejo/workflows/ci.yml; then
            if git -C "$_checkout" add .forgejo/workflows/ci.yml \
                && git -C "$_checkout" commit --quiet -m 'ci: use reference-delivery demo CI workflow' >>"$LOG_DIR/ci-seed.log" 2>&1 \
                && git -C "$_checkout" push --quiet origin "HEAD:$_base" >>"$LOG_DIR/ci-seed.log" 2>&1; then
                log "demo CI seed: applied bundled CI to $_repo@$_base"
            else
                log "demo CI seed: could not apply bundled CI to $_repo (see logs/ci-seed.log); landing CI may not pass"
            fi
        else
            log "demo CI seed: bundled CI already present for $_repo"
        fi
    done
}

# --- temper run ---------------------------------------------------------------

# Resolves the provisioned `bot` automation identity from the secrets file.
# `temper run` uses it for the mechanical backstop: landing CI-green PRs and the
# ADR-0019 web-UI CI read fallback. The setup-only site admin never participates
# in the workflow.
resolve_bot_identity() {
    [ -f "$CREDENTIALS_FILE" ] || die "missing $CREDENTIALS_FILE"
    # The bot `user` is omitted from credentials.toml when it equals the section
    # key, so default it to the literal `bot` automation login.
    BOT_USER=$(toml_forge_user_field bot user)
    [ -n "$BOT_USER" ] || BOT_USER=bot
    BOT_TOKEN=$(toml_forge_user_field bot token)
    BOT_PASSWORD=$(toml_forge_user_field bot password)
    [ -n "$BOT_USER" ] || die "automation user 'bot' has no username in $CREDENTIALS_FILE"
    [ "$BOT_USER" = "bot" ] || die "automation user must be 'bot' in $CREDENTIALS_FILE, got '$BOT_USER'"
    [ -n "$BOT_TOKEN" ] || die "automation user 'bot' has no token in $CREDENTIALS_FILE"
    [ -n "$BOT_PASSWORD" ] || die "automation user 'bot' has no password in $CREDENTIALS_FILE"
}

# Boots one `temper run`: the daemon, one worker, and the in-process coding agent
# on a single event loop. Replaces the split daemon + worker boot of the legacy
# topology.
boot_run() {
    resolve_bot_identity
    ensure_secret_file "$WEBHOOK_SECRET_FILE"
    mkdir -p "$RUN_DIR/workspaces"

    # The new CLI is config-file driven: standalone `temper daemon` (no
    # --service) runs engine + worker + agent in one process. Write the
    # deployment to a config file; the per-role and bot secrets come from the
    # provisioned credentials.toml via `--secrets` (never on argv).
    _repos_toml=$(printf '"%s", ' $CONFIGURED_REPOS); _repos_toml="[${_repos_toml%, }]"
    _roles_toml=$(printf '"%s", ' $SERVED_ROLES); _roles_toml="[${_roles_toml%, }]"
    case "$TEMPER_RUN_AUTH" in
        chatgpt-oauth) _provider=chatgpt ;;
        anthropic-oauth) _provider=anthropic ;;
        *) _provider=deepseek ;;
    esac
    _config="$RUN_DIR/config.toml"
    cat >"$_config" <<EOF
schema_version = 1
[forge]
type = "forgejo"
url = "$BASE_URL"
admin = "bot"
ci_user = "bot"
[engine]
bind = "$DAEMON_BIND"
repos = $_repos_toml
roles = $_roles_toml
workflow = "$WORKFLOW_PATH"
webhook_secret_file = "$WEBHOOK_SECRET_FILE"
poll_cadence_secs = $DAEMON_POLL_CADENCE_SECS
mechanical_cadence_secs = $DAEMON_MECHANICAL_CADENCE_SECS
lease_ttl_secs = $DAEMON_LEASE_TTL_SECS
daemon_id = "reference-delivery-daemon"
[worker]
worker_id = "reference-delivery-1"
workspace = "$RUN_DIR/workspaces"
git_base_url = "$BASE_URL"
[agent]
provider = "$_provider"
max_iterations = $RUN_MAX_ITERATIONS
EOF

    log "starting temper daemon at $DAEMON_BIND (repos: $CONFIGURED_REPOS; roles: $SERVED_ROLES; poll=${DAEMON_POLL_CADENCE_SECS}s mechanical=${DAEMON_MECHANICAL_CADENCE_SECS}s provider=$_provider) ..."
    : >"$LOG_DIR/run.log"
    (
        FORGEJO_ACCESS_TOKEN="$BOT_TOKEN" \
        FORGEJO_USERNAME="$BOT_USER" \
        FORGEJO_PASSWORD="$BOT_PASSWORD" \
            "$RUN_BIN" daemon --config "$_config" --secrets "$CREDENTIALS_FILE"
    ) >"$LOG_DIR/run.log" 2>&1 &
    RUN_PID=$!
    echo "$RUN_PID" >"$RUN_PID_FILE"
    # Readiness: the webhook listener must be up before the seed-last webhook can
    # be delivered, the in-process worker capacity must be initialized before any
    # job can be assigned, and the daemon's ready banner is the final startup
    # signal.
    wait_for_log_line "$LOG_DIR/run.log" 'webhook listener up' "$RUN_PID" 'temper daemon'
    wait_for_log_line "$LOG_DIR/run.log" 'worker:  capacity:' "$RUN_PID" 'temper daemon'
    wait_for_log_line "$LOG_DIR/run.log" 'ready -- watching' "$RUN_PID" 'temper daemon'
    log "temper daemon up (pid $RUN_PID; logs/run.log)"
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

validate_line_with_literals() {
    _file=$1
    _literal_one=$2
    _literal_two=$3
    _description=$4
    if grep -F "$_literal_one" "$_file" 2>/dev/null | grep -F -q "$_literal_two"; then
        log "ok: $_description"
        return 0
    fi
    log "missing: $_description (looked in $_file)"
    return 1
}

# Confirms `temper run` has the bot automation credentials it needs to merge
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
        log 'diagnosis: provision the bot user and launch temper run with its REST token plus FORGEJO_USERNAME/FORGEJO_PASSWORD for landing and the ADR-0019 CI read fallback'
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
        log 'missing: temper run reported missing Forgejo web-UI credentials for CI reads'
        log 'diagnosis: the landing queue needs native CI; pass the bot FORGEJO_USERNAME/FORGEJO_PASSWORD to temper run (ADR 0019)'
        _ok=1
    fi
    if grep -F -q "$CI_FALLBACK_LOGIN_FAILED" "$_run_log" 2>/dev/null; then
        log 'missing: temper run could not log in to Forgejo web UI for CI reads'
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
    log "configured repos: $CONFIGURED_REPOS"
    log "configured DAEMON_POLL_CADENCE_SECS=$DAEMON_POLL_CADENCE_SECS DAEMON_MECHANICAL_CADENCE_SECS=$DAEMON_MECHANICAL_CADENCE_SECS; long-poll smoke expects DAEMON_POLL_CADENCE_SECS=120"

    validate_mechanical_bot_config || _ok=1
    validate_mechanical_ci_log || _ok=1

    validate_contains "$_provision_log" 'webhook registered url=' \
        'repo webhook registration recorded' || _ok=1
    validate_contains "$_run_log" 'webhook listener up' \
        'temper daemon webhook listener reached readiness' || _ok=1
    validate_contains "$_run_log" 'worker:  capacity:' \
        'in-process worker capacity reported' || _ok=1
    validate_contains "$_run_log" 'ready -- watching' \
        'temper daemon reached watching readiness' || _ok=1
    validate_contains "$_run_log" 'event="wake.received"' \
        'Forgejo delivered at least one accepted webhook wake' || _ok=1
    validate_contains "$_run_log" 'mark_untriaged applied' \
        'seed-last wake advanced raw intake into triage' || _ok=1
    validate_contains "$_run_log" 'event="agent.started"' \
        'in-process agent accepted at least one assignment' || _ok=1
    validate_contains "$_run_log" 'event="agent.finished"' \
        'in-process agent finished at least one assignment' || _ok=1
    validate_contains "$_run_log" 'event="lease.claimed"' \
        'daemon apply path claimed at least one result lease' || _ok=1
    validate_contains "$_run_log" 'event="lease.released"' \
        'daemon apply path released at least one result lease' || _ok=1

    _wakes=$(count_matches 'event="wake.received"' "$_run_log")
    _advanced=$(count_matches 'mark_untriaged applied' "$_run_log")
    _agent_started=$(count_matches 'event="agent.started"' "$_run_log")
    _agent_finished=$(count_matches 'event="agent.finished"' "$_run_log")
    log "daemon summary: wakes=$_wakes raw_intake_advanced=$_advanced agent_started=$_agent_started agent_finished=$_agent_finished"

    _registered=$(count_matches 'worker:  capacity:' "$_run_log")
    _leases_claimed=$(count_matches 'event="lease.claimed"' "$_run_log")
    _leases_released=$(count_matches 'event="lease.released"' "$_run_log")
    log "worker summary: registered=$_registered leases_claimed=$_leases_claimed leases_released=$_leases_released"

    if [ "$_ok" -eq 0 ]; then
        log 'webhook validation passed'
    else
        log 'webhook validation failed; inspect logs/provision.log and logs/run.log'
    fi
    return "$_ok"
}

validate_repo_specific_logs() {
    _ok=0
    _provision_log="$LOG_DIR/provision.log"
    _run_log="$LOG_DIR/run.log"
    for _repo in $CONFIGURED_REPOS; do
        validate_contains "$_provision_log" "repo=$_repo webhook registered" \
            "webhook registration recorded for $_repo" || _ok=1
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
        validate_line_with_literals "$_run_log" 'event="agent.started"' "repo=\"$_repo\"" \
            "in-process agent accepted at least one assignment for $_repo" || _ok=1
        validate_line_with_literals "$_run_log" 'event="agent.finished"' "repo=\"$_repo\"" \
            "in-process agent finished at least one assignment for $_repo" || _ok=1
    done
    return "$_ok"
}

validator_token() {
    [ -f "$CREDENTIALS_FILE" ] || return 1
    _token=$(toml_forge_user_field architect token)
    [ -n "$_token" ] || return 1
    printf '%s\n' "$_token"
}

cross_repo_parent_number() {
    sed -n "s|^repo=$FIRST_CONFIGURED_REPO cross_repo_parent_url=.*/issues/\([0-9][0-9]*\).*|\1|p" \
        "$LOG_DIR/provision.log" 2>/dev/null | sed -n '1p'
}

cmd_validate_reference_delivery_state() {
    [ "$CROSS_REPO_ENABLED" = "1" ] || return 0
    _ok=0
    RUN_BIN=${TEMPER_RUN_BIN:-$WORKSPACE_ROOT/target/debug/temper}
    if [ ! -x "$RUN_BIN" ]; then
        log "missing: temper binary not found at $RUN_BIN for Forge-state validation"
        return 1
    fi
    _parent=$(cross_repo_parent_number)
    if [ -z "$_parent" ]; then
        log "missing: could not derive cross-repo parent issue number from logs/provision.log"
        return 1
    fi
    _token=$(validator_token) || {
        log "missing: could not find architect read token in $CREDENTIALS_FILE for Forge-state validation"
        return 1
    }
    _repo_args=
    for _repo in $CONFIGURED_REPOS; do
        _repo_args="$_repo_args --repo $_repo"
    done
    log "validating reference-delivery Forge state for parent $FIRST_CONFIGURED_REPO#$_parent"
    # _repo_args intentionally word-split; repo values are validated owner/name.
    # shellcheck disable=SC2086
    if _output=$(TEMPER_FORGEJO_TOKEN="$_token" "$RUN_BIN" validate-reference-delivery \
        --base-url "$BASE_URL" $_repo_args \
        --source-repo "$FIRST_CONFIGURED_REPO" \
        --parent-number "$_parent" \
        --expected-children "$REPO_COUNT" 2>&1); then
        printf '%s\n' "$_output" | while IFS= read -r _line; do
            [ -n "$_line" ] && log "$_line"
        done
    else
        _ok=1
        printf '%s\n' "$_output" | while IFS= read -r _line; do
            [ -n "$_line" ] && log "$_line"
        done
    fi
    return "$_ok"
}

cmd_validate_multi_repo() {
    _ok=0
    cmd_validate_webhooks || _ok=1
    validate_repo_specific_logs || _ok=1
    cmd_validate_reference_delivery_state || _ok=1
    return "$_ok"
}

# --- Monitor ------------------------------------------------------------------

# Blocks until the stop-file appears, the server dies, or RUN_SECS elapses, so
# the EXIT/INT/TERM trap can tear everything down on Ctrl-C.
monitor() {
    log ''
    log "Forgejo UI:   $BASE_URL  (log in as any provisioned role)"
    log "temper run:   http://$DAEMON_BIND  (webhook + poll/mechanical backstops + in-process coding agent for: $CONFIGURED_REPOS)"
    log "Roles served: [$SERVED_ROLES] across: $CONFIGURED_REPOS"
    log 'Repo issue URLs:'
    for _repo in $CONFIGURED_REPOS; do
        log "  $_repo -> $BASE_URL/$_repo/issues"
    done
    log "Logs:         $LOG_DIR/run.log"
    log 'Single-repo path: human intake -> architect triage rewrite -> engineer PR'
    log '(created with implementation+needs-reviewer) -> reviewer approve -> landing ->'
    log 'mechanical bot-merge -> source issue closed.'
    log 'Cross-repo path: parent intake -> architect needs_breakdown -> one child'
    log 'code issue per repo (parent backrefs + dependency refs) -> each child -> PR'
    log '-> review -> merge; owner/human are idle by design in this demo.'
    if [ "$CROSS_REPO_ENABLED" = "1" ]; then
        log "Cross-repo intake: one parent in $FIRST_CONFIGURED_REPO fans out across $REPO_COUNT repos."
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
    boot_run
    seed_intake
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
