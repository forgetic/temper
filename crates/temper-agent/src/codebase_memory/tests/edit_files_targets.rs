//! Batch edits expose every explicit target to ordinary-read admission.

use std::sync::Arc;

use serde_json::json;
use temper_agent_core::{
    InvocationTargetAdmission, LineageAdmissionResolver, TargetAdmissionOutcome,
    TargetAdmissionStatus,
};

use crate::codebase_memory::lineage::DecisionAnchorLineageRegistry;
use crate::codebase_memory::scope::WorkspaceScope;
use crate::workspace_edits::EditFilesInput;

#[test]
fn edit_files_admission_uses_shared_parser_and_requires_every_exact_read() {
    let root = tempfile::tempdir().unwrap();
    let context = super::test_support::workspace_context(root.path(), &[("acme", "demo", "demo")]);
    std::fs::create_dir_all(root.path().join("demo/src")).unwrap();
    for name in ["one.rs", "two.rs"] {
        std::fs::write(root.path().join("demo/src").join(name), "fn example() {}\n").unwrap();
    }
    let scope = Arc::new(WorkspaceScope::from_context(&context, root.path()).unwrap());
    let registry = DecisionAnchorLineageRegistry::new(scope);
    let read = registry.resolve_invocation_targets("read", &json!({"path":"demo/src/one.rs"}));
    let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(identity)) = read else {
        panic!("existing source must have an opaque read identity");
    };
    let arguments = json!({"files":[
        {"path":"demo/src/one.rs","edits":[{"oldText":"example","newText":"first"}]},
        {"path":"demo/src/two.rs","edits":[{"oldText":"example","newText":"second"}]}
    ]});
    assert_eq!(EditFilesInput::parse(&arguments).unwrap().files.len(), 2);
    let InvocationTargetAdmission::Mutation(targets) =
        registry.resolve_invocation_targets("edit_files", &arguments)
    else {
        panic!("batch must expose every target");
    };
    assert!(
        matches!(&targets[0],TargetAdmissionOutcome::Eligible(target) if target.matches(&identity))
    );
    assert_eq!(
        targets[1],
        TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget)
    );
    registry.resolve_invocation_targets("read", &json!({"path":"demo/src/two.rs"}));
    let InvocationTargetAdmission::Mutation(targets) =
        registry.resolve_invocation_targets("edit_files", &arguments)
    else {
        panic!("read companions must remain explicit mutation targets");
    };
    assert!(
        targets
            .iter()
            .all(|target| matches!(target, TargetAdmissionOutcome::Eligible(_)))
    );
    for invalid in [
        json!({"files":[{"path":"../outside","edits":[{"oldText":"x","newText":"y"}]}]}),
        json!({"files":[arguments["files"][0].clone(),arguments["files"][0].clone()]}),
        json!({"files":[arguments["files"][0].clone()],"hidden":"demo/src/two.rs"}),
    ] {
        assert!(EditFilesInput::parse(&invalid).is_err());
        assert_eq!(
            registry.resolve_invocation_targets("edit_files", &invalid),
            InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::MalformedTarget)
        );
    }
}
