// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn target_sections_resolve_relative_to_config_base() {
    let config = parse_config(
        r#"
schema_version = 1
[deployment]
name = "local-dev"
topology = "standalone"
[workflow]
file = "flows/workflow.json"
[paths]
state_dir = "state"
workspace_dir = "workspace"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("target sections resolve");

    assert_eq!(resolved.deployment.name.as_deref(), Some("local-dev"));
    assert_eq!(
        resolved.deployment.topology,
        Some(DeploymentTopology::Standalone)
    );
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.paths.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.paths.state_dir.as_deref(),
        Some(std::path::Path::new("/bundle/state"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/workspace")
    );
    assert_eq!(
        resolved.paths.workspace_dir,
        std::path::Path::new("/bundle/workspace")
    );
}

#[test]
fn target_state_dir_controls_default_workspace_root() {
    let config = parse_config(
        r#"
schema_version = 1
[paths]
state_dir = "state"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("state dir resolves");

    assert_eq!(
        resolved.paths.state_dir.as_deref(),
        Some(std::path::Path::new("/bundle/state"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/state/workspace")
    );
    assert_eq!(
        resolved.paths.workspace_dir,
        std::path::Path::new("/bundle/state/workspace")
    );
}

#[test]
fn legacy_workflow_and_workspace_remain_supported() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
workflow = "flows/workflow.json"
[worker]
workspace = "workspace"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("legacy path fields resolve");

    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/workspace")
    );
}

#[test]
fn matching_target_and_legacy_paths_are_accepted() {
    let config = parse_config(
        r#"
schema_version = 1
[workflow]
file = "flows/workflow.json"
[paths]
workspace_dir = "workspace"
[engine]
workflow = "flows/workflow.json"
[worker]
workspace = "workspace"
"#,
    );
    let options = ResolveOptions::from_config_base_dir("/bundle");
    let resolved = resolve_with_options(&config, &Credentials::default(), &NoEnv, &options)
        .expect("matching target and legacy values resolve");

    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/bundle/flows/workflow.json"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/bundle/workspace")
    );
}

#[test]
fn conflicting_target_and_legacy_workflow_is_invalid() {
    let config = parse_config(
        r#"
schema_version = 1
[workflow]
file = "target-workflow.json"
[engine]
workflow = "legacy-workflow.json"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("conflicting workflow fields are rejected");
    let message = err.to_string();
    assert!(message.contains("workflow.file"), "{message}");
    assert!(message.contains("engine.workflow"), "{message}");
    assert!(message.contains("conflicting"), "{message}");
}

#[test]
fn conflicting_target_and_legacy_workspace_is_invalid() {
    let config = parse_config(
        r#"
schema_version = 1
[paths]
workspace_dir = "target-workspace"
[worker]
workspace = "legacy-workspace"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("conflicting workspace fields are rejected");
    let message = err.to_string();
    assert!(message.contains("paths.workspace_dir"), "{message}");
    assert!(message.contains("worker.workspace"), "{message}");
    assert!(message.contains("conflicting"), "{message}");
}

#[test]
fn invalid_deployment_topology_is_rejected() {
    let config = parse_config(
        r#"
schema_version = 1
[deployment]
topology = "clustered"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("invalid topology is rejected");
    assert!(
        err.to_string().contains("deployment.topology"),
        "error should name the invalid field: {err}"
    );
}
