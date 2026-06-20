// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_protocol_worker::JobProgress;

#[test]
fn started_progress_checkpoints_do_not_create_issue_comments() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let number = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = progress_job(number);
        let correlation = correlation_key(number);

        applier
            .apply_progress(
                job.clone(),
                progress(&correlation, 1, "started", "start engineer run", None, None),
            )
            .await;
        applier
            .apply_progress(
                job,
                progress(
                    &correlation,
                    2,
                    "started",
                    "resume engineer run from pushed checkpoints",
                    Some("fedcba98765432100123456789abcdef01234567"),
                    None,
                ),
            )
            .await;

        let comments = issue_comments(&forge, &repo, number).await;
        let progress_comments: Vec<_> = comments
            .iter()
            .filter(|comment| comment.body.contains("temper-progress"))
            .collect();
        assert!(
            progress_comments.is_empty(),
            "started progress must stay off issue comments: {progress_comments:?}"
        );
    })
}

#[test]
fn started_open_pr_progress_claims_source_issue() {
    temper_engine_io::block_on(async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue = create_ready_issue(&root, &repo).await;
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let applier = ForgeApplier::new(forge, Arc::new(workflow()));
        let job = open_pr_in_flight_job("acme/service", issue);
        let correlation = correlation_key(issue);

        applier
            .apply_progress(
                job,
                progress(&correlation, 1, "started", "start engineer run", None, None),
            )
            .await;

        let issue = root
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists");
        assert_eq!(
            issue.labels,
            vec!["code".to_string(), "in-progress".to_string()]
        );
        assert_eq!(issue.assignees, vec![UserId::new("engineer")]);
        assert!(issue_comments(&root, &repo, issue.number).await.is_empty());
    })
}

#[test]
fn checkpoint_done_progress_does_not_create_checklist_or_issue_chatter() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let correlation = correlation_key(issue);
        let branch_name = format!("agent/{correlation}");
        let mut result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch_name,
            "implemented with checkpoints",
        );
        result.details = Some(json!({
            "plan": {"phases": ["Write failing test", "Implement fix"]}
        }));

        applier.apply(job.clone(), result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull_number = pulls[0].number;
        let body = pulls[0].body.clone();
        assert!(!body.contains("Implementation plan"));
        assert!(!body.contains("- [ ]"));

        applier
            .apply_progress(
                job,
                progress(
                    &correlation,
                    2,
                    "done",
                    "Implement fix",
                    Some("abc123456789"),
                    Some("checkpoint pushed"),
                ),
            )
            .await;

        assert_eq!(pull_request_body(&forge, &repo, pull_number).await, body);
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());
    })
}

#[test]
fn trivial_pr_receives_no_checklist_progress_chatter() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let correlation = correlation_key(issue);
        let branch_name = format!("agent/{correlation}");
        let result = success_result(
            "worker-a",
            &job.job_id,
            &job.repo,
            &branch_name,
            "implemented one obvious edit",
        );

        applier.apply(job.clone(), result).await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull_number = pulls[0].number;
        let body = pulls[0].body.clone();
        assert!(!body.contains("Implementation plan"));
        assert!(!body.contains("- [ ]"));

        applier
            .apply_progress(
                job,
                progress(
                    &correlation,
                    1,
                    "done",
                    "Apply obvious edit",
                    Some("abc123456789"),
                    None,
                ),
            )
            .await;

        assert_eq!(pull_request_body(&forge, &repo, pull_number).await, body);
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());
    })
}

#[test]
fn final_engineer_open_pr_progress_uses_pr_body_not_issue_comment() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = open_pr_in_flight_job("acme/service", issue);
        let correlation = correlation_key(issue);
        let branch_name = format!("agent/{correlation}");
        let summary = "Implemented the API and tests.";

        // The terminal progress checkpoint can arrive immediately before the
        // success result that opens the PR. It should not leave a duplicate
        // final-summary comment on the source issue.
        applier
            .apply_progress(
                job.clone(),
                progress(
                    &correlation,
                    3,
                    "done",
                    "finish engineer run",
                    None,
                    Some(summary),
                ),
            )
            .await;
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());

        applier
            .apply(
                job.clone(),
                success_result("worker-a", &job.job_id, &job.repo, &branch_name, summary),
            )
            .await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        assert!(pulls[0].body.contains(summary));
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());

        // Re-delivery after the PR exists is also quiet.
        applier
            .apply_progress(
                job,
                progress(
                    &correlation,
                    3,
                    "done",
                    "finish engineer run",
                    None,
                    Some(summary),
                ),
            )
            .await;
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());
    })
}

#[test]
fn terminal_progress_comment_requires_useful_final_note_and_is_idempotent() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let number = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = progress_job(number);
        let correlation = correlation_key(number);

        applier
            .apply_progress(
                job.clone(),
                progress(
                    &correlation,
                    3,
                    "done",
                    "finish engineer run",
                    None,
                    Some("Implemented the API and tests."),
                ),
            )
            .await;
        applier
            .apply_progress(
                job,
                progress(
                    &correlation,
                    3,
                    "done",
                    "finish engineer run",
                    None,
                    Some("Implemented the API and tests."),
                ),
            )
            .await;

        let comments = issue_comments(&forge, &repo, number).await;
        let progress_comments: Vec<_> = comments
            .iter()
            .filter(|comment| comment.body.contains("temper-progress"))
            .collect();
        assert_eq!(
            progress_comments.len(),
            1,
            "duplicate final-summary progress must not duplicate forge state: {progress_comments:?}"
        );
        assert!(progress_comments[0].body.contains(&format!(
            "<!-- temper-progress correlation_key={} step=3 state=done -->",
            correlation
        )));
        assert!(
            progress_comments[0]
                .body
                .contains("Implemented the API and tests."),
            "useful final note should be preserved: {}",
            progress_comments[0].body
        );
    })
}

fn progress_job(number: ItemNumber) -> InFlightJob {
    InFlightJob {
        job_id: "acme/service/issue-1/engineer/code_ready".to_string(),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(number.get()),
            kind: "issue".to_string(),
        },
        job_payload: json!({ "correlation_key": correlation_key(number) }),
    }
}

fn correlation_key(number: ItemNumber) -> String {
    format!("pr-for-code-{}", number.get())
}

fn progress(
    correlation_key: &str,
    step: u32,
    state: &str,
    status: &str,
    pushed_sha: Option<&str>,
    note: Option<&str>,
) -> JobProgress {
    JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        correlation_key: correlation_key.to_string(),
        step,
        status: status.to_string(),
        state: state.to_string(),
        pushed_sha: pushed_sha.map(str::to_string),
        note: note.map(str::to_string),
    }
}

async fn pull_request_body(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> String {
    forge
        .get_pull_request_by_number(repo, number)
        .await
        .expect("pull request lookup succeeds")
        .expect("pull request exists")
        .body
}

async fn issue_comments(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<temper_forge::Comment> {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue lookup succeeds")
        .expect("issue exists");
    forge
        .list_issue_comments(&issue.id)
        .await
        .expect("comments list")
}
