// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_protocol_worker::JobProgress;

#[test]
fn started_progress_checkpoints_update_one_run_ledger_without_comments() {
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

        let body = issue_body(&forge, &repo, number).await;
        assert_one_run_ledger(&body, &correlation);
        assert!(body.contains("Current status: editing"));
        assert!(body.contains("Latest progress: step 2"));
        assert!(body.contains("fedcba987654"));
        assert!(issue_comments(&forge, &repo, number).await.is_empty());
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
        let body = issue.body;
        assert_one_run_ledger(&body, &correlation);
        assert!(body.contains("Current status: editing"));
        assert!(body.contains("Work branch: `agent/pr-for-code-"));
        assert!(issue_comments(&root, &repo, issue.number).await.is_empty());
    })
}

#[test]
fn checkpoint_done_progress_updates_same_run_ledger_idempotently_before_pr() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = open_pr_in_flight_job("acme/service", issue);
        let correlation = correlation_key(issue);

        applier
            .apply_progress(
                job.clone(),
                progress(&correlation, 1, "started", "start engineer run", None, None),
            )
            .await;
        let checkpoint = progress(
            &correlation,
            2,
            "done",
            "implement managed ledger",
            Some("abc123456789fedcba98765432100123456789ab"),
            Some("checkpoint pushed"),
        );
        applier
            .apply_progress(job.clone(), checkpoint.clone())
            .await;
        applier.apply_progress(job, checkpoint).await;

        let body = issue_body(&forge, &repo, issue).await;
        assert_one_run_ledger(&body, &correlation);
        assert!(body.contains("Current status: checkpointed"));
        assert!(
            body.contains("Latest checkpoint: step 2 — implement managed ledger (abc123456789)")
        );
        assert!(!body.contains("Implementation plan"));
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());
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
        let issue_body_after_success = issue_body(&forge, &repo, issue).await;
        assert_one_run_ledger(&issue_body_after_success, &correlation);
        assert!(
            issue_body_after_success.contains(&format!("continued in PR #{}", pull_number.get()))
        );

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
        assert_eq!(
            issue_body(&forge, &repo, issue).await,
            issue_body_after_success
        );
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
        let pre_pr_body = issue_body(&forge, &repo, issue).await;
        assert_one_run_ledger(&pre_pr_body, &correlation);
        assert!(pre_pr_body.contains("Current status: finalizing"));
        assert!(
            !pre_pr_body.contains(summary),
            "source issue ledger must not duplicate the final implementation summary: {pre_pr_body}"
        );

        applier
            .apply(
                job.clone(),
                success_result("worker-a", &job.job_id, &job.repo, &branch_name, summary),
            )
            .await;

        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let pull_number = pulls[0].number;
        assert!(pulls[0].body.contains(summary));
        let issue_body_after_success = issue_body(&forge, &repo, issue).await;
        assert_one_run_ledger(&issue_body_after_success, &correlation);
        assert!(
            issue_body_after_success.contains(&format!("continued in PR #{}", pull_number.get()))
        );
        assert!(
            !issue_body_after_success.contains(summary),
            "source issue ledger must not duplicate PR summary after handoff: {issue_body_after_success}"
        );
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());

        // Re-delivery after the PR exists is also quiet and does not replace the
        // finalized ledger with checkpoint details.
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
        assert_eq!(
            issue_body(&forge, &repo, issue).await,
            issue_body_after_success
        );
        assert!(issue_comments(&forge, &repo, issue).await.is_empty());
    })
}

#[test]
fn non_pr_final_progress_is_migrated_to_the_run_ledger_and_is_idempotent() {
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

        let body = issue_body(&forge, &repo, number).await;
        assert_one_run_ledger(&body, &correlation);
        assert!(body.contains("Current status: finalizing"));
        assert!(body.contains("Final note:"));
        assert!(body.contains("Implemented the API and tests."));
        assert!(
            issue_comments(&forge, &repo, number).await.is_empty(),
            "engineer issue final progress should use the managed ledger instead of ad-hoc comments"
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

async fn issue_body(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> String {
    forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue lookup succeeds")
        .expect("issue exists")
        .body
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
