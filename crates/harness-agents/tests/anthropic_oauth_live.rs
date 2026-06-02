//! Live validation for the Anthropic OAuth auth mode.
//!
//! This reads the real shared `~/.pi/agent/auth.json` and uses the `anthropic`
//! entry written by `pi /login anthropic`. It is `#[ignore]`d and gated on
//! `HARNESS_ANTHROPIC_OAUTH=1`, so the default test suite never touches the
//! network or real credentials.
//!
//! ```sh
//! HARNESS_ANTHROPIC_OAUTH=1 \
//!   cargo test -p harness-agents --test anthropic_oauth_live -- --ignored --nocapture
//! ```

use harness_agents::{AuthChoice, ProviderConfig, ProviderRoleDecisionEngine, RoleDecisionEngine};
use harness_workflow::{RawWorkflowSpec, RoleId, RoleManifest};
use std::time::Instant;

#[test]
#[ignore = "makes real Anthropic OAuth calls; run with HARNESS_ANTHROPIC_OAUTH=1"]
fn anthropic_oauth_validation() {
    if std::env::var("HARNESS_ANTHROPIC_OAUTH").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping Anthropic OAuth live validation: set HARNESS_ANTHROPIC_OAUTH=1 \
             (reads the real ~/.pi/agent/auth.json and makes real Anthropic calls). \
             Run `pi /login anthropic` first."
        );
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let provider = ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, None)
        .expect("Anthropic OAuth provider builds (run `pi /login anthropic` first)");
    eprintln!("[anthropic] model id: {}", provider.model_id());

    let role = fixture_role_manifest();
    let context = fixture_role_context(&role);
    let start = Instant::now();
    let decision_engine = ProviderRoleDecisionEngine::new(provider);
    let decision = runtime
        .block_on(decision_engine.decide(&role.prompt.render(), &context))
        .expect("Anthropic OAuth generic role decision succeeds and parses");
    assert_eq!(decision.action, "advance");
    eprintln!(
        "[anthropic] generic role decision: {decision:?} latency={:?}",
        start.elapsed()
    );
}

fn fixture_role_manifest() -> RoleManifest {
    let json = r#"{
        "name": "anthropic-generic-role-smoke",
        "roles": [{
            "id": "banana",
            "prompt": {
                "guidance": "When the work item is a task in the todo queue with the todo label, choose the advance action."
            },
            "queues": ["todo"]
        }],
        "labels": [{"id": "task"}, {"id": "todo"}, {"id": "done"}],
        "artifact_kinds": [{
            "id": "task",
            "target": "issue",
            "identifying_labels": ["task"]
        }],
        "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
        "transitions": [{
            "id": "advance",
            "artifact": "task",
            "roles": ["banana"],
            "effects": [
                {"kind": "remove_label", "label": "todo"},
                {"kind": "add_label", "label": "done"}
            ]
        }]
    }"#;
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow json parses");
    spec.validate()
        .expect("workflow validates")
        .compile()
        .role(&RoleId::new("banana"))
        .expect("banana role manifest exists")
        .clone()
}

fn fixture_role_context(role: &RoleManifest) -> String {
    let context = serde_json::json!({
        "work_item": {
            "repository": "forgejo:acme/service",
            "queue": "todo",
            "role": role.id.as_str(),
            "kind": "task",
            "artifact": {
                "type": "issue",
                "number": 1,
                "title": "Advance a generic task",
                "body": "This synthetic task is ready for the generic action.",
                "labels": ["task", "todo"],
                "state": "Open"
            }
        },
        "allowed_actions": ["no_action", "advance"],
        "authorized_actions": [{
            "action": "advance",
            "transition": "advance",
            "artifact": "task",
            "requires_gates": []
        }]
    });
    serde_json::to_string_pretty(&context).expect("context serializes")
}
