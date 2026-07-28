//! Finalized-registry guidance and stable effective action prompt snapshots.

use super::common::*;
use crate::coding_agent::*;
use async_trait::async_trait;
use tongs::error::Result as ToolResult;
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

struct NamedTool(String);

#[async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        &self.0
    }

    fn label(&self) -> &str {
        &self.0
    }

    fn description(&self) -> &str {
        "test-only named tool"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::read()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> ToolResult<ToolOutput> {
        panic!("test-only tool is never executed")
    }
}

fn named_registry(names: &[&str]) -> ToolRegistry {
    ToolRegistry::from_tools(
        names
            .iter()
            .map(|name| Box::new(NamedTool((*name).to_string())) as Box<dyn Tool>)
            .collect(),
    )
}

#[test]
fn registry_aware_guidance_names_only_finalized_tools() {
    let mut context: WorkspaceContext = serde_json::from_str(include_str!(
        "../../../../temper-protocol-agent/fixtures/workspace-context-artifact-context.json"
    ))
    .expect("artifact context fixture");
    context.allowed_verdicts.clear();

    let optional_names = [
        "submit_for_pr",
        "forge_get_item",
        "forge_list_related",
        "investigate",
        "delegate",
    ];
    let registry = named_registry(&optional_names);
    let role = system_prompt_with_registry(
        Capability::CodingWorkspace,
        &context.work_item.role,
        &context.allowed_verdicts,
        &context.verdict_contracts,
        &registry,
    );
    let user = user_context_with_registry(&context, &registry);
    let effective = format!("{role}\n{user}");
    let manifest = registry
        .tools()
        .iter()
        .map(|tool| tool.name())
        .collect::<std::collections::BTreeSet<_>>();
    for name in optional_names {
        assert!(
            effective.contains(name),
            "guidance omitted registered {name}"
        );
        assert!(
            manifest.contains(name),
            "guidance named unregistered {name}"
        );
    }

    let empty = named_registry(&[]);
    let role = system_prompt_with_registry(
        Capability::CodingWorkspace,
        &context.work_item.role,
        &context.allowed_verdicts,
        &context.verdict_contracts,
        &empty,
    );
    let user = user_context_with_registry(&context, &empty);
    for name in optional_names {
        assert!(
            !format!("{role}\n{user}").contains(name),
            "guidance retained unavailable {name}"
        );
    }

    let investigate_only = named_registry(&["investigate"]);
    let role = system_prompt_with_registry(
        Capability::CodingWorkspace,
        "engineer",
        &[],
        &Default::default(),
        &investigate_only,
    );
    assert!(role.contains("investigate"));
    assert!(!role.contains("delegate"));
}

#[test]
fn host_guidance_follows_filtered_submit_and_forge_registry() {
    let mut context: WorkspaceContext = serde_json::from_str(include_str!(
        "../../../../temper-protocol-agent/fixtures/workspace-context-artifact-context.json"
    ))
    .expect("artifact context fixture");
    context.work_item.role = "engineer".to_string();
    context.checkout = Some("writable".to_string());
    context.allowed_verdicts.clear();

    let submit: SubmitForPrCallback = std::sync::Arc::new(|_| {
        Box::pin(async { temper_protocol_agent::SubmitForPrResponse::accepted("ok") })
    });
    let forge: ForgeContextHost = std::sync::Arc::new(|_| {
        Box::pin(async { Err(temper_protocol_agent::ForgeContextErrorCode::NotFound) })
    });
    let registry = tool_registry_for_context(
        Capability::CodingWorkspace,
        &context,
        std::path::Path::new("."),
        Some(submit.clone()),
        Some(forge),
    );
    let role = system_prompt_with_registry(
        Capability::CodingWorkspace,
        &context.work_item.role,
        &[],
        &Default::default(),
        &registry,
    );
    let user = user_context_with_registry(&context, &registry);
    assert!(role.contains("`submit_for_pr`"));
    assert!(user.contains("`forge_get_item`"));
    assert!(user.contains("`forge_list_related`"));

    let without_hosts = tool_registry_for_context(
        Capability::CodingWorkspace,
        &context,
        std::path::Path::new("."),
        None,
        None,
    );
    let role = system_prompt_with_registry(
        Capability::CodingWorkspace,
        &context.work_item.role,
        &[],
        &Default::default(),
        &without_hosts,
    );
    let user = user_context_with_registry(&context, &without_hosts);
    assert!(!role.contains("submit_for_pr"));
    assert!(!user.contains("forge_get_item"));
    assert!(!user.contains("forge_list_related"));

    context.checkout = Some("read_only".to_string());
    let read_only = tool_registry_for_context(
        Capability::CodingWorkspace,
        &context,
        std::path::Path::new("."),
        Some(submit),
        None,
    );
    let role = system_prompt_with_registry(
        Capability::CodingWorkspace,
        &context.work_item.role,
        &[],
        &Default::default(),
        &read_only,
    );
    assert!(!role.contains("submit_for_pr"));
}

#[test]
fn scenario_author_gets_writable_role_identity_and_submit_guidance() {
    let mut context = parsed_fixture();
    context.work_item.role = "scenario_author".to_string();
    context.checkout = Some("writable".to_string());
    context.allowed_verdicts.clear();
    let submit: SubmitForPrCallback = std::sync::Arc::new(|_| {
        Box::pin(async { temper_protocol_agent::SubmitForPrResponse::accepted("ok") })
    });
    let capability = Capability::for_role(&context.work_item.role);
    let registry = tool_registry_for_context(
        capability,
        &context,
        std::path::Path::new("."),
        Some(submit),
        None,
    );

    let role = system_prompt_with_registry(
        capability,
        &context.work_item.role,
        &[],
        &Default::default(),
        &registry,
    );
    let names = registry
        .tools()
        .iter()
        .map(|tool| tool.name())
        .collect::<Vec<_>>();
    assert!(role.contains("ROLE: scenario_author (coding_workspace capability)"));
    assert!(!role.contains("ROLE: engineer"));
    assert!(role.contains("`submit_for_pr`"));
    assert!(names.contains(&"write"));
    assert!(names.contains(&"submit_for_pr"));
}

#[test]
fn stable_effective_action_prompt_snapshots() {
    let cases = [
        action_snapshot_case(
            "plan_feature",
            "architect",
            "feature",
            "read_only",
            &["needs_plan", "config_only"],
            temper_verdict::VerdictContracts::from([
                (
                    "needs_plan".to_string(),
                    temper_verdict::VerdictContract {
                        min_children: 1,
                        max_children: Some(1),
                        allowed_child_kinds: vec!["plan".to_string()],
                        required_child_metadata: vec!["target_branch".to_string()],
                        ..Default::default()
                    },
                ),
                (
                    "config_only".to_string(),
                    temper_verdict::VerdictContract {
                        max_children: Some(0),
                        ..Default::default()
                    },
                ),
            ]),
        ),
        action_snapshot_case(
            "decompose_plan",
            "architect",
            "plan",
            "read_only",
            &["children_ready", "config_only"],
            temper_verdict::VerdictContracts::from([
                (
                    "children_ready".to_string(),
                    temper_verdict::VerdictContract {
                        min_children: 1,
                        allowed_child_kinds: vec!["code".to_string()],
                        ..Default::default()
                    },
                ),
                (
                    "config_only".to_string(),
                    temper_verdict::VerdictContract {
                        max_children: Some(0),
                        ..Default::default()
                    },
                ),
            ]),
        ),
        action_snapshot_case(
            "open_pr",
            "engineer",
            "code",
            "writable",
            &[],
            Default::default(),
        ),
    ];

    for (name, context, capability) in cases {
        let submit = (name == "open_pr").then(|| {
            std::sync::Arc::new(|_| {
                Box::pin(async { temper_protocol_agent::SubmitForPrResponse::accepted("ok") })
                    as SubmitForPrFuture
            }) as SubmitForPrCallback
        });
        let registry = tool_registry_for_context(
            capability,
            &context,
            std::path::Path::new("."),
            submit,
            None,
        );
        let role = system_prompt_with_registry(
            capability,
            &context.work_item.role,
            &context.allowed_verdicts,
            &context.verdict_contracts,
            &registry,
        );
        let user = user_context_with_registry(&context, &registry);
        let names = registry
            .tools()
            .iter()
            .map(|tool| tool.name())
            .collect::<Vec<_>>()
            .join(", ");
        let actual = format!("SYSTEM PROMPT\n{role}\n\nUSER PROMPT\n{user}\nTOOL NAMES\n{names}\n");
        assert_action_snapshot(name, &actual);

        match name {
            "plan_feature" => {
                assert!(actual.contains("`needs_plan`"));
                assert!(actual.contains("`config_only`"));
            }
            "decompose_plan" => {
                assert!(actual.contains("`children_ready`"));
                assert!(actual.contains("`config_only`"));
            }
            "open_pr" => {
                assert!(actual.contains("LEGACY FALLBACK OUTCOMES"));
                assert!(actual.contains("no-verdict success path"));
                assert!(actual.contains("`submit_for_pr`"));
            }
            _ => unreachable!(),
        }
        if name != "open_pr" {
            for forbidden in ["ready_code", "needs_design", "needs_breakdown"] {
                assert!(
                    !actual.contains(forbidden),
                    "{name} leaked fallback outcome {forbidden}"
                );
            }
        }
    }
}

fn action_snapshot_case<'a>(
    action: &'a str,
    role: &str,
    kind: &str,
    checkout: &str,
    allowed: &[&str],
    contracts: temper_verdict::VerdictContracts,
) -> (&'a str, WorkspaceContext, Capability) {
    let mut context = parsed_fixture();
    context.action = action.to_string();
    context.work_item.role = role.to_string();
    context.work_item.kind = kind.to_string();
    context.work_item.context = "{}".to_string();
    context.checkout = Some(checkout.to_string());
    context.allowed_verdicts = allowed.iter().map(|name| (*name).to_string()).collect();
    context.verdict_contracts = contracts;
    context.guidance = WorkspaceGuidance::default();
    (action, context, Capability::for_role(role))
}

fn assert_action_snapshot(name: &str, actual: &str) {
    let expected = match name {
        "plan_feature" => include_str!("snapshots/plan_feature.txt"),
        "decompose_plan" => include_str!("snapshots/decompose_plan.txt"),
        "open_pr" => include_str!("snapshots/open_pr.txt"),
        _ => panic!("unknown action snapshot {name}"),
    };
    if std::env::var_os("UPDATE_EXPECT").is_some() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/coding_agent/tests/snapshots")
            .join(format!("{name}.txt"));
        std::fs::write(path, actual).expect("update action prompt snapshot");
        return;
    }
    assert_eq!(actual, expected, "effective {name} prompt changed");
}
