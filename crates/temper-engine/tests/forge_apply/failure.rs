// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

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

// ---------------------------------------------------------------------------
// Step-progress checkpoints (worker → daemon → forge relay, phase 6a).
// ---------------------------------------------------------------------------
