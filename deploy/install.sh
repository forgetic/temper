#!/bin/sh
# Idempotent local installer for the consolidated temper-daemon deployment
# assets. It installs binaries/templates only; provisioning secrets and starting
# the systemd unit remain operator actions.
set -eu

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
repo_root=$(CDPATH= cd "$script_dir/.." && pwd)

local_bin=${HOME:?HOME not set}/.local/bin
systemd_user_dir=$HOME/.config/systemd/user
temper_config_dir=$HOME/.config/temper

daemon_env=$temper_config_dir/daemon.env

mkdir -p "$local_bin" "$systemd_user_dir" "$temper_config_dir"

if [ "${TEMPER_SKIP_BUILD:-}" = 1 ]; then
    echo 'temper install: TEMPER_SKIP_BUILD=1; skipping cargo build'
else
    (cd "$repo_root" && cargo build -j2 --bin temper-daemon)
fi

install -m 0755 "$repo_root/target/debug/temper-daemon" "$local_bin/"
install -m 0755 "$repo_root/deploy/bin/temper-daemon-launcher" "$local_bin/"
install -m 0644 "$repo_root/deploy/systemd/temper-daemon.service" "$systemd_user_dir/"

if [ ! -e "$daemon_env" ]; then
    install -m 0644 "$repo_root/deploy/config/daemon.env" "$daemon_env"
    echo "temper install: installed template $daemon_env"
else
    echo "temper install: leaving existing $daemon_env unchanged"
fi

cat <<'EOF'
temper install: next steps:
  1. Ensure ~/.config/temper/secrets/roles.env is provisioned (not by this installer).
  2. Review ~/.config/temper/daemon.env for repositories, roles, and webhook settings.
  3. Run: systemctl --user daemon-reload && systemctl --user start temper-daemon.service
EOF
