#!/bin/sh
# Smith consolidated worker deployment installer (idempotent).
#
# Installs the Smith worker tier for the daemon/worker topology:
#   - builds + installs the smith-worker binary (orchestration only) and the
#     anvil-agent binary from the sibling anvil checkout (the out-of-process
#     coding agent the worker spawns via --agent-command anvil-native),
#   - installs the smith-worker-launcher ExecStart shim,
#   - copies the smith-worker systemd user unit template,
#   - copies worker config templates into ~/.config/smith/ and agent prompt
#     templates into ~/.config/anvil/ WITHOUT clobbering files you have
#     already edited,
#   - creates the worker workspace parent under ~/.local/state/smith/.
#
# It does NOT deploy the Temper daemon, provision Forgejo, write roles.env, create
# webhook secrets, start services, or touch already-installed legacy units. Deploy
# the daemon from temper/deploy/install.sh and perform the live cutover manually.
#
# Re-running is safe: existing config is preserved, templates and binaries are
# refreshed, and no live secrets are generated or overwritten.
#
# POSIX sh only. Validate with `sh -n deploy/install.sh`.
# Secrets are read by systemd EnvironmentFile= at runtime, never echoed and never
# placed on argv by this installer.

set -eu

# --- Locations ----------------------------------------------------------------
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
ANVIL_REPO_ROOT=${ANVIL_REPO_ROOT:-$(CDPATH= cd -- "$REPO_ROOT/../anvil" && pwd)}
DEPLOY_SYSTEMD="$SCRIPT_DIR/systemd"
DEPLOY_CONFIG="$SCRIPT_DIR/config"
DEPLOY_BIN="$SCRIPT_DIR/bin"

# Locations are pinned under $HOME, matching the units' %h-based directives and
# the existing local dogfood layout. Do not switch these to XDG_CONFIG_HOME /
# XDG_STATE_HOME without also updating the unit templates and launcher.
BIN_DIR="$HOME/.local/bin"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
SMITH_CONFIG_DIR="$HOME/.config/smith"
SMITH_SECRETS_DIR="$SMITH_CONFIG_DIR/secrets"
SMITH_STATE_DIR="$HOME/.local/state/smith"
SMITH_WORKER_STATE_DIR="$SMITH_STATE_DIR/worker"
# The agent reads its prompt overlays from its own config dir (ANVIL_CONFIG_DIR
# or ~/.config/anvil), not from the worker's ~/.config/smith.
ANVIL_CONFIG_DIR="$HOME/.config/anvil"

# Build profile dir name (Smith builds in the dev profile here).
CARGO_PROFILE_DIR=debug

log() { printf '[install] %s\n' "$*"; }
die() { printf '[install] error: %s\n' "$*" >&2; exit 1; }

# --- Binaries -----------------------------------------------------------------
# Build and install the two binaries the worker tier invokes:
#   - smith-worker (the daemon long-poll worker; orchestration only)
#   - anvil-agent (the out-of-process coding agent, from the sibling anvil
#     checkout; the worker spawns it via `--agent-command anvil-native`)
# Installation is a copy into ~/.local/bin so the unit has stable absolute paths
# independent of the checkouts' target dirs.
build_smith_binaries() {
    if [ "${SMITH_SKIP_BUILD:-0}" != "1" ]; then
        log "building Smith worker binary in $REPO_ROOT ..."
        ( cd "$REPO_ROOT" \
            && cargo build -j2 -p smith-worker --bin smith-worker ) \
            || die 'Smith cargo build failed'
        log "building anvil-agent binary in $ANVIL_REPO_ROOT ..."
        ( cd "$ANVIL_REPO_ROOT" \
            && cargo build -j2 --bin anvil-agent ) \
            || die 'anvil cargo build failed'
    else
        log 'skipping cargo build because SMITH_SKIP_BUILD=1'
    fi

    install_binary "$REPO_ROOT/target/$CARGO_PROFILE_DIR/smith-worker"
    install_binary "$ANVIL_REPO_ROOT/target/$CARGO_PROFILE_DIR/anvil-agent"
}

install_binary() {
    _src=$1
    [ -x "$_src" ] || die "expected binary not found (build did not produce it): $_src"
    mkdir -p "$BIN_DIR"
    install -m 0755 "$_src" "$BIN_DIR/$(basename "$_src")"
    log "  installed $(basename "$_src") -> $BIN_DIR/$(basename "$_src")"
}

# Install the systemd ExecStart shim. It contains no secrets; it only translates
# smith.env knobs into smith-worker argv and leaves roles.env variables untouched.
install_shims() {
    mkdir -p "$BIN_DIR"
    install -m 0755 "$DEPLOY_BIN/smith-worker-launcher" "$BIN_DIR/smith-worker-launcher"
    log "  installed smith-worker-launcher -> $BIN_DIR/smith-worker-launcher"
}

# --- systemd user unit ---------------------------------------------------------
# Unit templates have no machine-specific substitutions, so they are always
# refreshed from the repo. Runtime behavior is controlled through smith.env.
install_units() {
    mkdir -p "$SYSTEMD_USER_DIR"
    install -m 0644 "$DEPLOY_SYSTEMD/smith-worker.service" "$SYSTEMD_USER_DIR/smith-worker.service"
    log "  installed smith-worker.service -> $SYSTEMD_USER_DIR/smith-worker.service"
}

# --- Config templates (never clobber live edits) ------------------------------
# Copies a template into place ONLY if the destination does not already exist, so
# an operator's edited config survives a re-run. Reports skip vs. install.
install_template() {
    _src=$1
    _dst=$2
    _mode=$3
    mkdir -p "$(dirname "$_dst")"
    if [ -e "$_dst" ]; then
        log "  keep   $_dst (already present; not overwritten)"
        return 0
    fi
    install -m "$_mode" "$_src" "$_dst"
    log "  create $_dst"
}

install_prompt_templates() {
    for _prompt in "$DEPLOY_CONFIG"/prompts/*; do
        [ -f "$_prompt" ] || continue
        install_template "$_prompt" "$ANVIL_CONFIG_DIR/prompts/$(basename "$_prompt")" 0644
    done
}

install_config() {
    mkdir -p "$SMITH_CONFIG_DIR" "$SMITH_SECRETS_DIR" "$ANVIL_CONFIG_DIR/prompts"
    install_template "$DEPLOY_CONFIG/smith.env" "$SMITH_CONFIG_DIR/smith.env" 0644
    install_template "$DEPLOY_CONFIG/workflow.json" "$SMITH_CONFIG_DIR/workflow.json" 0644
    install_prompt_templates
    install_template "$DEPLOY_CONFIG/secrets/README.md" "$SMITH_SECRETS_DIR/README.md" 0644
    install_template "$DEPLOY_CONFIG/secrets/.gitignore" "$SMITH_SECRETS_DIR/.gitignore" 0644
}

# --- Workspace + state parents ------------------------------------------------
install_state_dirs() {
    mkdir -p "$SMITH_WORKER_STATE_DIR"
    log "  ensured $SMITH_WORKER_STATE_DIR"
}

# --- Main ---------------------------------------------------------------------
main() {
    log 'installing Smith consolidated worker deployment'
    log "repo: $REPO_ROOT"

    log 'binaries:'
    build_smith_binaries

    log 'execstart shim:'
    install_shims

    log 'systemd user unit:'
    install_units

    log 'config templates:'
    install_config

    log 'state directories:'
    install_state_dirs

    cat <<EOF
[install] done.

Next steps:
  1. Ensure the Temper daemon tier is installed and running (from temper/deploy/install.sh).
  2. Ensure $SMITH_SECRETS_DIR/roles.env contains the provisioned per-role git credentials.
  3. Review $SMITH_CONFIG_DIR/smith.env, especially WORKER_DAEMON_URL and WORKER_CAPABILITIES.
  4. Start the worker after reloading systemd:
       systemctl --user daemon-reload && systemctl --user start smith-worker.service
  5. Watch it:
       journalctl --user -u smith-worker.service -f

For the legacy cutover order, see deploy/README.md.
EOF
}

main "$@"
