// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_forge::{CreateIssue, CreateRepository};
use temper_forge_memory::MemoryForge;

#[test]
fn startup_quarantine_records_one_idempotent_audit_comment() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let issue = forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "Malformed recovery assignment".to_string(),
                    body: "<!-- temper:workflow\n{not-json}\n-->".to_string(),
                    labels: vec!["code".to_string(), "in-progress".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let target = ArtifactSource::Issue {
            number: issue.number,
        };

        let workflow = temper_workflow::parse_workflow_spec(
            "reference-delivery.json",
            include_str!("../../../temper-workflow/fixtures/reference-delivery.json"),
        )
        .expect("workflow parses");
        let workflow = workflow.validate().expect("workflow validates");
        let converger = AssignmentConverger::new(
            &workflow,
            &forge,
            LeasePolicy::new(chrono::Duration::minutes(5)),
        );
        converger
            .quarantine_target(&repo, target, "malformed assignment")
            .await
            .expect("first quarantine succeeds");
        converger
            .quarantine_target(&repo, target, "malformed assignment")
            .await
            .expect("replayed quarantine succeeds");

        let issue = forge
            .get_issue_by_number(&repo, issue.number)
            .await
            .unwrap()
            .unwrap();
        assert!(issue.labels.contains(&"needs-human".to_string()));
        let comments = forge.list_issue_comments(&issue.id).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert!(
            comments[0]
                .body
                .contains(temper_workflow::ASSIGNMENT_RECOVERY_AUDIT_MARKER)
        );
    });
}

#[test]
fn missing_pull_collection_is_empty_during_startup_inventory() {
    let repo = RepositoryId::new("forgejo:acme/empty");
    let result = startup_pull_inventory(
        &repo,
        Err(ForgeError::NotFound(
            "pull collection unavailable".to_string(),
        )),
    )
    .expect("an absent PR collection is empty for an existing repository");

    assert!(result.is_empty());
}
