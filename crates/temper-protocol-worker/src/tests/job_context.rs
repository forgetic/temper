// SPDX-License-Identifier: MPL-2.0

use crate::{JobArtifactSnapshot, JobContext, WorkspaceManifest};
use temper_verdict::{VerdictContract, VerdictContracts};

use super::sample_manifest;

#[test]
fn full_job_context_round_trips_without_loss() {
    let context = JobContext {
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        queue: "code_ready".to_string(),
        artifact_kind: "code".to_string(),
        artifact: Some(JobArtifactSnapshot {
            number: 42,
            title: "Cross-repo worker protocol change".to_string(),
            body: "Change the protocol in temper and consume it in smith.".to_string(),
            labels: vec!["code".to_string(), "ready".to_string()],
            state: "Open".to_string(),
        }),
        workspace: Some(sample_manifest()),
        action: Some("open_pr".to_string()),
        checkout_capability: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string(), "needs_human".to_string()],
        verdict_contracts: VerdictContracts::from([(
            "needs_architect".to_string(),
            VerdictContract {
                min_children: 1,
                max_children: Some(1),
                allowed_child_kinds: vec!["plan".to_string()],
                required_child_metadata: vec!["target_branch".to_string()],
                ..VerdictContract::default()
            },
        )]),
        source_metadata: [("target_branch".to_string(), "feature/x".to_string())]
            .into_iter()
            .collect(),
        guidance: Some("fix CI".to_string()),
        pull_request_freshness: None,
    };

    let value = serde_json::to_value(&context).expect("job context serializes");
    let decoded: JobContext = serde_json::from_value(value).expect("serialized job context parses");
    assert_eq!(decoded, context);
}

#[test]
fn job_context_omits_empty_optional_fields() {
    let context = JobContext {
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        queue: "code_ready".to_string(),
        artifact_kind: "code".to_string(),
        artifact: Some(JobArtifactSnapshot {
            number: 42,
            title: "t".to_string(),
            body: "b".to_string(),
            labels: Vec::new(),
            state: "Open".to_string(),
        }),
        workspace: Some(WorkspaceManifest::single(
            "ai/temper",
            "temper",
            "main",
            "main",
            "agent/pr-for-code-42",
            "pr-for-code-42",
        )),
        action: None,
        checkout_capability: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: None,
        pull_request_freshness: None,
    };

    let value = serde_json::to_value(&context).expect("job context serializes");
    assert_eq!(value.get("action"), None);
    assert_eq!(value.get("checkout_capability"), None);
    assert_eq!(value.get("allowed_verdicts"), None);
    assert_eq!(value.get("verdict_contracts"), None);
    assert_eq!(value.get("source_metadata"), None);
    assert_eq!(value.get("guidance"), None);
    assert_eq!(value.get("pull_request_freshness"), None);
}

#[test]
fn thin_pre_enrichment_job_context_omits_artifact_and_workspace() {
    // The daemon's pure work-item mapping has no Forge access, so it emits a
    // thin context; enrichment fills artifact + workspace before dispatch.
    let context = JobContext {
        role: "engineer".to_string(),
        repo: "ai/temper".to_string(),
        queue: "code_ready".to_string(),
        artifact_kind: "code".to_string(),
        artifact: None,
        workspace: None,
        action: None,
        checkout_capability: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: None,
        pull_request_freshness: None,
    };

    let value = serde_json::to_value(&context).expect("job context serializes");
    assert_eq!(
        value,
        serde_json::json!({
            "role": "engineer",
            "repo": "ai/temper",
            "queue": "code_ready",
            "artifact_kind": "code"
        })
    );
    let decoded: JobContext = serde_json::from_value(value).expect("thin context parses");
    assert_eq!(decoded, context);
}

#[test]
fn job_context_unknown_fields_are_ignored() {
    let context: JobContext = serde_json::from_value(serde_json::json!({
        "role": "engineer",
        "repo": "ai/temper",
        "queue": "code_ready",
        "artifact_kind": "code",
        "artifact": {
            "number": 42, "title": "t", "body": "b", "labels": [], "state": "Open"
        },
        "workspace": {
            "coordination_key": "pr-for-code-42",
            "repos": [{
                "repo": "ai/temper", "dir": "temper", "access": "writable",
                "default_branch": "main", "base_branch": "main",
                "branch_hint": "agent/pr-for-code-42"
            }]
        },
        "future_field": "ignored"
    }))
    .expect("unknown job context fields must be accepted");

    assert_eq!(context.role, "engineer");
    assert!(context.verdict_contracts.is_empty());
    assert!(context.source_metadata.is_empty());
    assert_eq!(context.workspace.expect("workspace present").repos.len(), 1);
}
