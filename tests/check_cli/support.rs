// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const WORKFLOW_JSON: &str =
    include_str!("../../crates/temper-workflow/fixtures/reference-delivery.json");

pub fn temper(args: &[&str], env_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_temper"))
        .args(args)
        .env("XDG_CONFIG_HOME", env_root.join("xdg-config"))
        .env("XDG_STATE_HOME", env_root.join("xdg-state"))
        .env("HOME", env_root.join("home"))
        .output()
        .expect("run temper")
}

pub fn write_valid_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [deployment]\n\
         name = \"local-dev\"\n\
         topology = \"standalone\"\n\
         [workflow]\n\
         file = \"workflow.json\"\n\
         [paths]\n\
         state_dir = \"state\"\n\
         workspace_dir = \"workspace\"\n\
         [forge]\n\
         url = \"http://localhost:3000\"\n\
         admin = \"engineer\"\n\
         ci_user = \"engineer\"\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
    std::fs::write(bundle.join("workflow.json"), WORKFLOW_JSON).expect("write workflow");
    std::fs::create_dir_all(bundle.join("state")).expect("create state dir");
    std::fs::create_dir_all(bundle.join("workspace")).expect("create workspace dir");
    write_valid_credentials(&bundle);
    bundle
}

pub fn write_valid_credentials(bundle: &Path) {
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.engineer]\n\
         token = \"forge-token\"\n\
         password = \"forge-password\"\n\
         [agent.providers.anthropic]\n\
         type = \"api-key\"\n\
         key = \"provider-key\"\n",
    )
    .expect("write credentials");
}
