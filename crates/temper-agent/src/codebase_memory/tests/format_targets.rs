//! Formatting uses the same complete explicit target set in policy and execution.

use std::sync::Arc;

use temper_agent_core::{
    InvocationTargetAdmission, LineageAdmissionResolver, TargetAdmissionOutcome,
    TargetAdmissionStatus,
};

use crate::codebase_memory::lineage::DecisionAnchorLineageRegistry;
use crate::codebase_memory::scope::WorkspaceScope;
use crate::workspace_format::FormatRustInput;

#[test]
fn format_rust_admission_matches_shared_parser_and_keeps_unread_siblings_ineligible() {
    let root = tempfile::tempdir().unwrap();
    let context = super::test_support::workspace_context(root.path(), &[("acme", "demo", "demo")]);
    std::fs::create_dir_all(root.path().join("demo/src")).unwrap();
    for name in ["one.rs", "two.rs"] {
        std::fs::write(root.path().join("demo/src").join(name), "fn example() {}\n").unwrap();
    }
    let scope = Arc::new(WorkspaceScope::from_context(&context, root.path()).unwrap());
    let registry = DecisionAnchorLineageRegistry::new(scope);
    let read = registry
        .resolve_invocation_targets("read", &serde_json::json!({"path": "demo/src/one.rs"}));
    let InvocationTargetAdmission::Read(TargetAdmissionOutcome::Eligible(identity)) = read else {
        panic!("existing source must have an opaque read identity");
    };
    let arguments =
        serde_json::json!({"paths": ["demo/src/one.rs", "demo/src/two.rs"], "edition": "2021"});
    assert_eq!(FormatRustInput::parse(&arguments).unwrap().paths.len(), 2);
    let InvocationTargetAdmission::Mutation(targets) =
        registry.resolve_invocation_targets("format_rust", &arguments)
    else {
        panic!("formatter must admit every target as a mutation");
    };
    assert!(
        matches!(&targets[0], TargetAdmissionOutcome::Eligible(target) if target.matches(&identity))
    );
    assert_eq!(
        targets[1],
        TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget)
    );
    for invalid in [
        serde_json::json!({"paths": ["demo/src/one.rs", "../outside.rs"], "edition": "2021"}),
        serde_json::json!({"paths": ["demo/src/one.rs", "demo/src/one.rs"], "edition": "2021"}),
        serde_json::json!({"paths": ["demo/src/one.rs"], "edition": "2025"}),
        serde_json::json!({"paths": ["demo/src/one.rs"], "edition": "2021", "hidden": "demo/src/two.rs"}),
    ] {
        assert!(FormatRustInput::parse(&invalid).is_err());
        assert_eq!(
            registry.resolve_invocation_targets("format_rust", &invalid),
            InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::MalformedTarget)
        );
    }
}
