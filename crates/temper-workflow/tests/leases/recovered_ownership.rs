// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_forge::RepositoryId;
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::{
    RecoveredHeartbeatOutcome, RecoveredOwnershipLossReason, replace_metadata_block,
};

fn rewrite_assignment_metadata(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    mutate: impl FnOnce(&mut temper_workflow::WorkflowMetadata),
) {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("issue lookup succeeds")
        .expect("issue exists");
    let mut metadata = parse_metadata_block(&issue.body)
        .expect("metadata parses")
        .expect("metadata exists");
    mutate(&mut metadata);
    let body = replace_metadata_block(&issue.body, &metadata).expect("metadata renders");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            body: Some(body),
            ..UpdateIssue::default()
        },
    ))
    .expect("metadata rewrite succeeds");
}

fn recovered_assignment() -> DurableAssignment {
    DurableAssignment {
        job_id: Some("job-258".to_string()),
        attempt_id: Some("attempt-1".to_string()),
        role: Some(RoleId::new("engineer")),
        queue: Some("code_ready".to_string()),
        action: Some("open_pr".to_string()),
        worker_id: Some("worker-a".to_string()),
        coordination_key: Some("pr-for-code-258".to_string()),
        daemon_boot_id: Some("prior-boot".to_string()),
        assignment_pr_head: Some("abc123".to_string()),
        ..DurableAssignment::default()
    }
}

fn claim_recovered_assignment(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    expected: &DurableAssignment,
) {
    let manager = LeaseManager::new(forge, policy());
    block_on(manager.claim_assignment(
        repo,
        ArtifactSource::Issue { number },
        AssignmentClaimRequest {
            assignment: expected.clone(),
            mutation: AssignmentMutation::default(),
        },
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("recovered assignment fixture is claimed");
}

#[test]
fn recovered_assignment_heartbeat_requires_every_identity_field() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);

    for field in [
        "job_id",
        "attempt_id",
        "role",
        "queue",
        "action",
        "worker_id",
        "coordination_key",
        "daemon_boot_id",
        "assignment_pr_head",
    ] {
        let number = create_issue(&forge, &repo, &["code", "ready"], "Recover me.");
        let target = ArtifactSource::Issue { number };
        let expected = recovered_assignment();
        claim_recovered_assignment(&forge, &repo, number, &expected);
        rewrite_assignment_metadata(&forge, &repo, number, |metadata| {
            let assignment = metadata.assignment.as_mut().expect("assignment exists");
            match field {
                "job_id" => assignment.job_id = Some("replacement-job".to_string()),
                "attempt_id" => assignment.attempt_id = Some("attempt-2".to_string()),
                "role" => assignment.role = Some(RoleId::new("reviewer")),
                "queue" => assignment.queue = Some("other_queue".to_string()),
                "action" => assignment.action = Some("review".to_string()),
                "worker_id" => assignment.worker_id = Some("worker-b".to_string()),
                "coordination_key" => {
                    assignment.coordination_key = Some("replacement-key".to_string());
                }
                "daemon_boot_id" => assignment.daemon_boot_id = Some("new-boot".to_string()),
                "assignment_pr_head" => {
                    assignment.assignment_pr_head = Some("def456".to_string());
                }
                _ => unreachable!(),
            }
        });

        assert_eq!(
            block_on(LeaseManager::new(&forge, policy()).heartbeat_assignment(
                &repo,
                target,
                &expected,
                ts("2026-05-29T00:05:00Z"),
            )),
            RecoveredHeartbeatOutcome::OwnershipLost {
                reason: RecoveredOwnershipLossReason::AssignmentReplaced,
            },
            "field {field} must fence the recovered assignment",
        );
    }
}

#[test]
fn recovered_assignment_legacy_optional_fields_and_attempt_fence_are_explicit() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let manager = LeaseManager::new(&forge, policy());

    let number = create_issue(&forge, &repo, &["code", "ready"], "Legacy optionals.");
    let target = ArtifactSource::Issue { number };
    let current = recovered_assignment();
    claim_recovered_assignment(&forge, &repo, number, &current);
    let expected_without_later_optional_fields = DurableAssignment {
        queue: None,
        action: None,
        coordination_key: None,
        assignment_pr_head: None,
        ..current.clone()
    };
    assert_eq!(
        block_on(manager.heartbeat_assignment(
            &repo,
            target,
            &expected_without_later_optional_fields,
            ts("2026-05-29T00:05:00Z"),
        )),
        RecoveredHeartbeatOutcome::Owned,
        "omitted legacy optional fields do not become mismatches",
    );

    let legacy_number = create_issue(&forge, &repo, &["code", "ready"], "Legacy attempt.");
    let legacy_target = ArtifactSource::Issue {
        number: legacy_number,
    };
    let legacy = DurableAssignment {
        attempt_id: None,
        ..recovered_assignment()
    };
    claim_recovered_assignment(&forge, &repo, legacy_number, &legacy);
    let newer_attempt = DurableAssignment {
        attempt_id: Some("attempt-new".to_string()),
        ..legacy
    };
    assert_eq!(
        block_on(manager.heartbeat_assignment(
            &repo,
            legacy_target,
            &newer_attempt,
            ts("2026-05-29T00:05:00Z"),
        )),
        RecoveredHeartbeatOutcome::OwnershipLost {
            reason: RecoveredOwnershipLossReason::AssignmentReplaced,
        },
        "legacy None is not a wildcard for a fenced attempt",
    );
}

#[test]
fn recovered_assignment_heartbeat_classifies_missing_and_replaced_durable_state() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let manager = LeaseManager::new(&forge, policy());

    for (case, expected_reason) in [
        (
            "assignment_absent",
            RecoveredOwnershipLossReason::AssignmentAbsent,
        ),
        (
            "assignment_replaced",
            RecoveredOwnershipLossReason::AssignmentReplaced,
        ),
        ("lease_absent", RecoveredOwnershipLossReason::LeaseAbsent),
        (
            "lease_replaced",
            RecoveredOwnershipLossReason::LeaseReplaced,
        ),
    ] {
        let number = create_issue(&forge, &repo, &["code", "ready"], case);
        let target = ArtifactSource::Issue { number };
        let expected = recovered_assignment();
        claim_recovered_assignment(&forge, &repo, number, &expected);
        rewrite_assignment_metadata(&forge, &repo, number, |metadata| match case {
            "assignment_absent" => metadata.assignment = None,
            "assignment_replaced" => {
                metadata
                    .assignment
                    .as_mut()
                    .expect("assignment exists")
                    .attempt_id = Some("attempt-new".to_string());
            }
            "lease_absent" => metadata.lease = None,
            "lease_replaced" => {
                metadata.lease.as_mut().expect("lease exists").worker = "other-boot".to_string();
            }
            _ => unreachable!(),
        });

        assert_eq!(
            block_on(manager.heartbeat_assignment(
                &repo,
                target,
                &expected,
                ts("2026-05-29T00:05:00Z"),
            )),
            RecoveredHeartbeatOutcome::OwnershipLost {
                reason: expected_reason,
            },
            "case {case}",
        );
    }
}

#[test]
fn recovered_assignment_heartbeat_classifies_target_metadata_backend_and_contention() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let manager = LeaseManager::new(&forge, policy());
    let expected = recovered_assignment();

    assert_eq!(
        block_on(manager.heartbeat_assignment(
            &repo,
            ArtifactSource::Issue {
                number: ItemNumber::new(999),
            },
            &expected,
            ts("2026-05-29T00:05:00Z"),
        )),
        RecoveredHeartbeatOutcome::OwnershipLost {
            reason: RecoveredOwnershipLossReason::TargetRemoved,
        },
    );

    let malformed_number = create_issue(&forge, &repo, &["code", "ready"], "Malformed.");
    let malformed_issue = block_on(forge.get_issue_by_number(&repo, malformed_number))
        .unwrap()
        .unwrap();
    block_on(forge.update_issue(
        &malformed_issue.id,
        UpdateIssue {
            body: Some("<!-- temper:workflow\nnot-json\n-->".to_string()),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert!(matches!(
        block_on(manager.heartbeat_assignment(
            &repo,
            ArtifactSource::Issue {
                number: malformed_number,
            },
            &expected,
            ts("2026-05-29T00:05:00Z"),
        )),
        RecoveredHeartbeatOutcome::OwnershipLost {
            reason: RecoveredOwnershipLossReason::MalformedClaim { .. }
        }
    ));

    let number = create_issue(&forge, &repo, &["code", "ready"], "Transient.");
    let target = ArtifactSource::Issue { number };
    claim_recovered_assignment(&forge, &repo, number, &expected);
    forge.fail_next(FaultOp::GetIssueByNumber, "temporary transport failure");
    assert!(matches!(
        block_on(manager.heartbeat_assignment(
            &repo,
            target,
            &expected,
            ts("2026-05-29T00:05:00Z"),
        )),
        RecoveredHeartbeatOutcome::TransientlyUnavailable { reason }
            if reason.contains("temporary transport failure")
    ));

    forge.conflict_next(FaultOp::UpdateIssue, "simulated heartbeat CAS race");
    assert!(matches!(
        block_on(manager.heartbeat_assignment(
            &repo,
            target,
            &expected,
            ts("2026-05-29T00:10:00Z"),
        )),
        RecoveredHeartbeatOutcome::TransientlyUnavailable { reason }
            if reason.contains("fresh durable ownership still matches")
    ));

    assert_eq!(
        block_on(manager.heartbeat_assignment(
            &repo,
            target,
            &expected,
            ts("2026-05-29T00:15:00Z"),
        )),
        RecoveredHeartbeatOutcome::Owned,
    );
    let metadata = parse_metadata_block(&issue_body(&forge, &repo, number))
        .unwrap()
        .unwrap();
    assert_eq!(
        metadata.lease.unwrap().heartbeat_at,
        ts("2026-05-29T00:15:00Z")
    );
}
