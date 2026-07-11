// SPDX-License-Identifier: MPL-2.0

//! Responsibility-based integration tests for `run_init`.

use std::path::Path;
use temper_cli_common::LoadOptions;
use temper_cli_init::{InitOptions, InitOverrides, RepoSelection};
use temper_workflow::RawWorkflowSpec;

#[path = "run_init/answers.rs"]
mod answers;
#[path = "run_init/apply.rs"]
mod apply;
#[path = "run_init/local_generation.rs"]
mod local_generation;
mod support;

fn options(dir: &Path, repos: &[&str]) -> InitOptions {
    InitOptions {
        options: LoadOptions {
            config: Some(dir.join("config.toml")),
            credentials: Some(dir.join("credentials.toml")),
        },
        non_interactive: true,
        overrides: InitOverrides {
            forge_url: Some("http://forge.local:3000".into()),
            repos: repos
                .iter()
                .map(|repo| {
                    let (owner, name) = repo.split_once('/').expect("owner/name");
                    RepoSelection {
                        owner: owner.into(),
                        name: name.into(),
                    }
                })
                .collect(),
            admin_user: Some("root".into()),
            admin_password: Some("admin-pass".into()),
            provider: Some("deepseek".into()),
            provider_key: Some("sk-key".into()),
            bind: Some("127.0.0.1:38100".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn basic_delivery_spec() -> RawWorkflowSpec {
    serde_json::from_str(temper_reference_delivery::basic_delivery_workflow_json())
        .expect("workflow")
}

fn assert_workflow_yaml(path: &Path) {
    let text = std::fs::read_to_string(path).expect("workflow written");
    let generated: RawWorkflowSpec = serde_yaml::from_str(&text).expect("YAML");
    assert_eq!(generated, basic_delivery_spec());
    generated.validate().expect("valid workflow");
    assert!(!text.trim_start().starts_with('{'));
}
