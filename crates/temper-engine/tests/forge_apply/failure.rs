// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_protocol_worker::JobProgress;

#[test]
fn peer_owned_lease_prevents_forge_apply_and_preserves_peer_metadata() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let target = ArtifactSource::Issue { number: issue };
        let manager = LeaseManager::new(forge.as_ref(), policy());
        let peer_lease = manager
            .acquire(
                &repo,
                target,
                RoleId::new("engineer"),
                "peer-daemon",
                chrono::Utc::now(),
            )
            .await
            .expect("peer lease is acquired");
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(ForgeApplier::new(forge.clone(), workflow)),
            temper_engine::system_clock(),
        );
        let job = in_flight_job("acme/service", issue);
        let result = success_result(
            "worker-a",
            &job.job_id,
            "acme/service",
            &format!("agent/pr-for-code-{}", issue.get()),
            "done",
        );

        applier.apply(job, result).await;

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("list pull requests succeeds");
        assert!(pulls.is_empty());
        let issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("issue exists after apply");
        let lease = parse_metadata_block(&issue.body)
            .expect("issue metadata parses")
            .expect("issue has metadata")
            .lease
            .expect("peer lease is still present");
        assert_eq!(lease, peer_lease);
    })
}

#[test]
fn success_without_branch_does_not_create_pull_request_or_mark_issue() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);

        applier
            .apply(job.clone(), success_without_branch("worker-a", &job.job_id))
            .await;

        assert_no_pull_requests(&forge, &repo).await;
        assert_no_attention_mark(&forge, &repo, issue).await;
    })
}

#[test]
fn permanent_failure_marks_issue_for_human_attention_and_audit() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);

        applier
            .apply(
                job.clone(),
                permanent_failure_result("worker-a", &job.job_id),
            )
            .await;

        assert_no_pull_requests(&forge, &repo).await;
        let labels = issue_labels(&forge, &repo, issue).await;
        assert!(labels.iter().any(|label| label == "needs-human"));
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        let comment = &comments[0];
        assert!(comment.contains("not implemented"));
        assert!(comment.contains("failure class: permanent"));
        assert!(comment.contains(&format!("job_id: `{}`", job.job_id)));
        assert!(comment.contains("worker: `worker-a`"));
        assert!(comment.contains(&format!(
            "<!-- temper:comment-key=daemon_failure_audit:{} -->",
            job.job_id
        )));
    })
}

#[test]
fn failure_marking_applies_for_human_audit_classes() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        for (failure_class, expected_class, message) in [
            (
                Some(FailureClass::Permanent),
                "permanent",
                "permanent worker failure",
            ),
            (
                Some(FailureClass::Protocol),
                "protocol",
                "protocol worker failure",
            ),
            (None, "unknown", "missing failure details"),
        ] {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge, "stable").await;
            let issue = create_ready_issue(&forge, &repo).await;
            let workflow = Arc::new(workflow());
            let applier = ForgeApplier::new(forge.clone(), workflow);
            let job = in_flight_job("acme/service", issue);

            applier
                .apply(
                    job.clone(),
                    failure_result("worker-a", &job.job_id, failure_class, message),
                )
                .await;

            assert_no_pull_requests(&forge, &repo).await;
            let labels = issue_labels(&forge, &repo, issue).await;
            assert!(labels.iter().any(|label| label == "needs-human"));
            let comments = issue_comment_bodies(&forge, &repo, issue).await;
            assert_eq!(comments.len(), 1);
            let comment = &comments[0];
            assert!(comment.contains(&format!("failure class: {expected_class}")));
            assert!(comment.contains(&format!("job_id: `{}`", job.job_id)));
            assert!(comment.contains("worker: `worker-a`"));
            if failure_class.is_some() {
                assert!(comment.contains(message));
            } else {
                assert!(!comment.contains(message));
            }
        }
    })
}

#[test]
fn transient_failure_does_not_create_pull_request_or_mark_issue() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);

        applier
            .apply(
                job.clone(),
                failure_result(
                    "worker-a",
                    &job.job_id,
                    Some(FailureClass::Transient),
                    "try again later",
                ),
            )
            .await;

        assert_no_pull_requests(&forge, &repo).await;
        assert_no_attention_mark(&forge, &repo, issue).await;
    })
}

#[test]
fn transient_failure_after_started_releases_claimed_source_issue() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let applier = ForgeApplier::new(forge, Arc::new(workflow()));
        let job = open_pr_in_flight_job("acme/service", issue_number);
        let correlation = local_correlation_key(issue_number);

        applier
            .apply_progress(
                job.clone(),
                transient_progress(&correlation, 1, "started", "start engineer run"),
            )
            .await;

        let claimed = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists after started progress");
        assert_eq!(
            claimed.labels,
            vec!["code".to_string(), "in-progress".to_string()]
        );
        assert_eq!(claimed.assignees, vec![UserId::new("engineer")]);
        assert_one_run_ledger(&claimed.body, &correlation);
        assert!(claimed.body.contains("Current status: editing"));
        assert!(claimed.body.contains("Worker: `worker-a`"));

        let result = failure_result(
            "worker-a",
            &job.job_id,
            Some(FailureClass::Transient),
            "OpenAI API error: server_error: upstream overloaded",
        );
        applier.apply(job.clone(), result.clone()).await;

        let released = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists after transient failure");
        assert_eq!(
            released.labels,
            vec!["code".to_string(), "ready".to_string()],
            "transient failure must make the source issue queue-visible again"
        );
        assert!(
            released.assignees.is_empty(),
            "engineer claim assignee should be removed on retry release"
        );
        assert_one_run_ledger(&released.body, &correlation);
        assert!(released.body.contains("Current status: queued for retry"));
        assert!(
            released
                .body
                .contains("Retry: released back to the ready queue after a transient failure")
        );
        assert!(released.body.contains("Latest progress: step 1"));
        assert!(released.body.contains("Worker: `worker-a`"));
        assert_no_pull_requests(&root, &repo).await;
        assert_no_attention_mark(&root, &repo, issue_number).await;

        let body_after_first_apply = released.body.clone();
        let labels_after_first_apply = released.labels.clone();
        applier.apply(job, result).await;

        let replayed = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists after replay");
        assert_eq!(replayed.labels, labels_after_first_apply);
        assert!(replayed.assignees.is_empty());
        assert_eq!(
            replayed.body, body_after_first_apply,
            "replaying the transient result should not duplicate retry ledger state"
        );
        assert_no_pull_requests(&root, &repo).await;
        assert_no_attention_mark(&root, &repo, issue_number).await;
    })
}

#[test]
fn canceled_failure_does_not_create_pull_request_or_mark_issue() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);

        applier
            .apply(
                job.clone(),
                failure_result(
                    "worker-a",
                    &job.job_id,
                    Some(FailureClass::Canceled),
                    "worker stopped",
                ),
            )
            .await;

        assert_no_pull_requests(&forge, &repo).await;
        assert_no_attention_mark(&forge, &repo, issue).await;
    })
}

#[test]
fn permanent_failure_replay_is_idempotent() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);
        let result = permanent_failure_result("worker-a", &job.job_id);

        applier.apply(job.clone(), result.clone()).await;
        applier.apply(job.clone(), result).await;

        assert_no_pull_requests(&forge, &repo).await;
        let labels = issue_labels(&forge, &repo, issue).await;
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.as_str() == "needs-human")
                .count(),
            1
        );
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("not implemented"));
        assert!(comments[0].contains(&format!(
            "<!-- temper:comment-key=daemon_failure_audit:{} -->",
            job.job_id
        )));
    })
}

#[test]
fn permanent_failure_replay_dedupes_by_comment_marker_when_label_is_missing() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = in_flight_job("acme/service", issue);
        let result = permanent_failure_result("worker-a", &job.job_id);

        applier.apply(job.clone(), result.clone()).await;
        drop_issue_label(&forge, &repo, issue, "needs-human").await;
        applier.apply(job.clone(), result).await;

        assert_no_pull_requests(&forge, &repo).await;
        assert!(
            !issue_labels(&forge, &repo, issue)
                .await
                .iter()
                .any(|label| label == "needs-human")
        );
        let comments = issue_comment_bodies(&forge, &repo, issue).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("not implemented"));
        assert!(comments[0].contains(&format!(
            "<!-- temper:comment-key=daemon_failure_audit:{} -->",
            job.job_id
        )));
    })
}

fn local_correlation_key(number: ItemNumber) -> String {
    format!("pr-for-code-{}", number.get())
}

fn transient_progress(correlation_key: &str, step: u32, state: &str, status: &str) -> JobProgress {
    JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        correlation_key: correlation_key.to_string(),
        step,
        status: status.to_string(),
        state: state.to_string(),
        pushed_sha: None,
        note: None,
    }
}

fn assert_one_run_ledger(body: &str, correlation: &str) {
    let marker = format!("<!-- temper-run-ledger correlation_key={correlation} -->");
    assert_eq!(
        body.matches(&marker).count(),
        1,
        "expected exactly one run ledger marker in body: {body}"
    );
    assert_eq!(
        body.matches("<!-- /temper-run-ledger -->").count(),
        1,
        "expected exactly one run ledger end marker in body: {body}"
    );
}

// ---------------------------------------------------------------------------
// Step-progress checkpoints (worker → daemon → forge relay, phase 6a).
// ---------------------------------------------------------------------------
