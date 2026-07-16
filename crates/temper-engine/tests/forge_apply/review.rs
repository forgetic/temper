// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

#[test]
fn review_verdict_approve_submits_native_review_and_routes_landing_label() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;

        let result = verdict_result("worker-a", &assignment.job_id, "approve", None);
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let (labels, reviews) =
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
                !has_label(labels, "needs-reviewer")
                    && has_label(labels, "landing")
                    && reviews.len() == 1
            })
            .await;

        assert!(has_label(&labels, "implementation"));
        assert!(!has_label(&labels, "needs-reviewer"));
        assert!(has_label(&labels, "landing"));
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].decision, ReviewDecision::Approved);
    })
}

#[test]
fn review_verdict_changes_attaches_changes_requested_review_with_body() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;
        let authored = "please add error handling";

        let result = verdict_result("worker-a", &assignment.job_id, "changes", Some(authored));
        assert_release(
            post_json(
                &client,
                &url,
                &WorkerProtocolMessage::Result(result.clone()),
            )
            .await,
            "worker-a",
            &assignment.job_id,
        );

        let (labels, reviews) =
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
                !has_label(labels, "needs-reviewer") && reviews.len() == 1
            })
            .await;

        assert!(has_label(&labels, "implementation"));
        assert!(!has_label(&labels, "needs-reviewer"));
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].decision, ReviewDecision::ChangesRequested);
        let body = reviews[0].body.as_deref().expect("review carries a body");
        assert!(
            body.contains(authored),
            "review body should carry authored text, got `{body}`"
        );

        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let replay_job = review_in_flight_job("acme/service", pull_request);
        let replay_result =
            verdict_result("worker-a", &replay_job.job_id, "changes", Some(authored));
        ForgeApplier::new(forge.clone(), Arc::new(workflow()))
            .apply(replay_job, replay_result)
            .await;

        assert_pull_request_state_stays(&cx, &forge, &repo, pull_request, labels, 1).await;
    })
}

#[test]
fn review_verdict_escalate_adds_needs_architect_label() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;

        let result = verdict_result("worker-a", &assignment.job_id, "escalate", None);
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let (labels, reviews) =
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
                has_label(labels, "needs-architect") && reviews.is_empty()
            })
            .await;

        assert!(has_label(&labels, "implementation"));
        assert!(has_label(&labels, "needs-architect"));
        assert!(reviews.is_empty());
    })
}

#[test]
fn undeclared_review_verdict_quarantines_pull_request() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let pull_request = create_pull_request_needing_review(&forge, &repo).await;
        let (client, url, assignment) =
            assign_review_job(&handle, forge.clone(), &repo, pull_request).await;

        let result = verdict_result("worker-a", &assignment.job_id, "merge_now", None);
        let response = post(&client, &url, &WorkerProtocolMessage::Result(result)).await;
        assert_eq!(response.status, 422);

        let (labels, reviews) =
            wait_for_review_apply(&cx, &forge, &repo, pull_request, |labels, reviews| {
                has_label(labels, "needs-human") && reviews.is_empty()
            })
            .await;
        assert!(has_label(&labels, "implementation"));
        assert!(has_label(&labels, "needs-reviewer"));
        assert!(has_label(&labels, "needs-human"));
        assert!(reviews.is_empty());
    })
}
