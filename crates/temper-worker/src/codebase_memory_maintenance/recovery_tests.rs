// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn logical_repository_key_matches_the_runtime_stable_identity_contract() {
    let target = codebase_memory_recovery_target("ai/temper", Some(PathBuf::from("/elsewhere")))
        .expect("logical target resolves");
    assert_eq!(
        target.provider_key,
        "temper-v1-c64512cdee6aab050daf4ddccd4fb911f1fbca74fdc052c1c79ab68b209240ad"
    );
    assert_eq!(target.logical_repository, "ai/temper");
    assert_eq!(target.rebuild_from, Some(PathBuf::from("/elsewhere")));
}

#[test]
fn malformed_logical_repository_is_rejected_without_using_a_path() {
    for invalid in ["temper", "/temper", "ai/", "ai/temper/extra"] {
        assert!(codebase_memory_recovery_target(invalid, None).is_err());
    }
}

#[test]
fn changed_preflight_or_provider_identity_changes_the_review_binding() {
    let proposed = crate::CodebaseMemoryRetentionRecordResult {
        project: "obsolete".to_string(),
        repo_path: Some(PathBuf::from("/workspace/engineer/old/temper")),
        reason: "exceeds configured age bound".to_string(),
        estimated_bytes: Some(42),
    };
    let review = CodebaseMemoryRetentionReport {
        cache_instance_id: Some("cache-a".to_string()),
        inventory_complete: true,
        inventory_record_count: 1,
        proposed: vec![proposed],
        ..Default::default()
    };
    let mut preflight = review.clone();
    preflight.proposed[0].estimated_bytes = Some(43);
    let provider = CodebaseMemoryProviderIdentity {
        name: "codebase-memory-mcp".to_string(),
        version: "0.9.0".to_string(),
        cache_instance_id: None,
    };
    let plan = retention_plan_id(
        CodebaseMemoryRetentionPolicy::default(),
        &provider,
        None,
        &review,
    );
    let mut observed = review.clone();
    observed.inventory_duration_ms = 27;
    observed.duration_ms = 31;
    assert_eq!(
        plan,
        retention_plan_id(
            CodebaseMemoryRetentionPolicy::default(),
            &provider,
            None,
            &observed
        ),
        "latency evidence must not invalidate a reviewed provider plan"
    );
    let (unchanged, observed) = verify_unchanged_preflight(&review, observed);
    assert!(unchanged);
    assert_eq!(observed.inventory_duration_ms, 27);
    assert_ne!(
        plan,
        retention_plan_id(
            CodebaseMemoryRetentionPolicy::default(),
            &provider,
            None,
            &preflight
        )
    );
    assert_ne!(
        plan,
        retention_plan_id(
            CodebaseMemoryRetentionPolicy::default(),
            &CodebaseMemoryProviderIdentity {
                version: "0.10.0".to_string(),
                ..provider
            },
            None,
            &review
        )
    );

    let (unchanged, refused) = verify_unchanged_preflight(&review, preflight);
    assert!(!unchanged, "changed preflight fails closed");
    assert!(refused.proposed.is_empty());
    assert!(refused.no_op_reason.as_deref().unwrap().contains("changed"));
}
