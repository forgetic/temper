#!/bin/sh
# Idempotent local installer for the temper engine-service deployment assets. It
# installs the unified `temper` binary, the ExecStart launcher, the systemd unit,
# and a config template only; provisioning secrets and starting the unit remain
# operator actions.
set -eu

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
repo_root=$(CDPATH= cd "$script_dir/.." && pwd)

local_bin=${HOME:?HOME not set}/.local/bin
systemd_user_dir=$HOME/.config/systemd/user
temper_config_dir=$HOME/.config/temper

config_file=$temper_config_dir/config.toml

mkdir -p "$local_bin" "$systemd_user_dir" "$temper_config_dir"

if [ "${TEMPER_SKIP_BUILD:-}" = 1 ]; then
    echo 'temper install: TEMPER_SKIP_BUILD=1; skipping cargo build'
else
    (cd "$repo_root" && cargo build -j2 --bin temper)
fi

install -m 0755 "$repo_root/target/debug/temper" "$local_bin/"
install -m 0755 "$repo_root/deploy/bin/temper-daemon-launcher" "$local_bin/"
install -m 0644 "$repo_root/deploy/systemd/temper-daemon.service" "$systemd_user_dir/"

if [ ! -e "$config_file" ]; then
    install -m 0644 "$repo_root/deploy/config/config.toml" "$config_file"
    echo "temper install: installed template $config_file"
else
    echo "temper install: leaving existing $config_file unchanged"
fi

cat <<'EOF'
temper install: next steps:
  1. Ensure ~/.config/temper/secrets/roles.env is provisioned (not by this installer).
  2. Review ~/.config/temper/config.toml for repositories, roles, and webhook settings
     (run `temper config validate` to check it).
  3. Run: systemctl --user daemon-reload && systemctl --user start temper-daemon.service
EOF
