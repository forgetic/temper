// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::collections::BTreeSet;

#[test]
fn repeated_negative_validation_rounds_use_attempt_bound_audit_markers() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::with_current_user(actor()));
        let repo = new_repo(&forge, "main").await;
        let plan = create_plan(&forge, &repo).await;
        let workflow = Arc::new(validation_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);

        let mut first_job = validation_job(plan, Vec::new());
        first_job.attempt_id = Some("validation-attempt-1".to_string());
        let first_result = round_followup_result(
            &first_job,
            "First round found an API gap.",
            "repair-first-round",
            "Repair the first-round API gap",
        );
        assert_eq!(
            applier.apply(first_job.clone(), first_result.clone()).await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(
            applier.apply(first_job.clone(), first_result.clone()).await,
            temper_engine::ApplyOutcome::Stale,
            "exact first-attempt replay is idempotent"
        );
        assert_eq!(issue_comments(&forge, &repo, plan).await.len(), 1);

        // A completed follow-up round returns the plan to in-progress. Simulate
        // the follow-up landing and the workflow's later validation handoff.
        let in_progress = forge
            .get_issue_by_number(&repo, plan)
            .await
            .unwrap()
            .unwrap();
        forge
            .update_issue(
                &in_progress.id,
                UpdateIssue {
                    add_labels: vec!["needs-validation".to_string()],
                    remove_labels: vec!["in-progress".to_string()],
                    expected_version: Some(in_progress.version),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("plan returns to validation");

        let mut second_job = first_job.clone();
        second_job.attempt_id = Some("validation-attempt-2".to_string());
        assert_eq!(first_job.job_id, second_job.job_id);
        let second_result = round_followup_result(
            &second_job,
            "Second round found a client gap.",
            "repair-second-round",
            "Repair the second-round client gap",
        );
        assert_eq!(
            applier
                .apply(second_job.clone(), second_result.clone())
                .await,
            temper_engine::ApplyOutcome::Applied
        );

        let comments = issue_comments(&forge, &repo, plan).await;
        assert_eq!(comments.len(), 2, "each validation round is audited");
        let markers = comments
            .iter()
            .map(|comment| {
                comment
                    .body
                    .lines()
                    .find(|line| line.starts_with("<!-- temper:comment-key=plan-validation:"))
                    .expect("audit contains its marker")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            markers.len(),
            2,
            "attempt identities produce distinct markers"
        );
        assert!(
            markers
                .iter()
                .all(|marker| marker.contains("plan-validation:assignment-sha256:"))
        );
        let first_audit = comments
            .iter()
            .find(|comment| comment.body.contains("First round found an API gap."))
            .expect("first-round summary is retained");
        assert!(
            first_audit
                .body
                .contains("Attempt ID: `validation-attempt-1`")
        );
        assert!(first_audit.body.contains("Repair the first-round API gap"));
        assert!(
            !first_audit
                .body
                .contains("Repair the second-round client gap")
        );
        let second_audit = comments
            .iter()
            .find(|comment| comment.body.contains("Second round found a client gap."))
            .expect("second-round summary is retained");
        assert!(
            second_audit
                .body
                .contains("Attempt ID: `validation-attempt-2`")
        );
        assert!(
            second_audit
                .body
                .contains("Repair the second-round client gap")
        );
        assert!(!second_audit.body.contains("Repair the first-round API gap"));

        let completed = forge
            .get_issue_by_number(&repo, plan)
            .await
            .unwrap()
            .unwrap();
        let metadata = parse_metadata_block(&completed.body)
            .unwrap()
            .expect("plan retains durable create intents");
        let persisted_markers = metadata
            .create_issue_intents
            .values()
            .filter_map(|intent| intent.completion.as_ref())
            .filter_map(|completion| completion.completion_audit.as_ref())
            .map(|audit| audit.marker.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(persisted_markers, markers);
        assert_eq!(list_issues(&forge, &repo).await.len(), 3);

        for (job, result) in [(first_job, first_result), (second_job, second_result)] {
            assert_eq!(
                applier.apply(job, result).await,
                temper_engine::ApplyOutcome::Stale,
                "replay of either exact attempt stays idempotent"
            );
        }
        assert_eq!(issue_comments(&forge, &repo, plan).await.len(), 2);
        assert_eq!(list_issues(&forge, &repo).await.len(), 3);
    });
}
