// SPDX-License-Identifier: MPL-2.0

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

        applier
            .apply_progress(
                job.clone(),
                progress(1, "started", "start engineer run", None),
            )
            .await;
        applier
            .apply_progress(
                job,
                progress(
                    2,
                    "started",
                    "resume engineer run from pushed checkpoints",
                    Some("fedcba98765432100123456789abcdef01234567"),
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
fn done_progress_checkpoints_are_recorded_once_with_checked_line() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let number = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
        let job = progress_job(number);

        applier
            .apply_progress(
                job.clone(),
                progress(3, "done", "finish engineer run", None),
            )
            .await;
        // Re-delivery of the same (correlation_key, step, state) is a no-op.
        applier
            .apply_progress(job, progress(3, "done", "finish engineer run", None))
            .await;

        let comments = issue_comments(&forge, &repo, number).await;
        let progress_comments: Vec<_> = comments
            .iter()
            .filter(|comment| comment.body.contains("temper-progress"))
            .collect();
        assert_eq!(
            progress_comments.len(),
            1,
            "duplicate delivery must not duplicate forge state: {progress_comments:?}"
        );
        assert!(
            progress_comments[0].body.contains(
                "<!-- temper-progress correlation_key=pr-for-code-9 step=3 state=done -->"
            ),
            "done checkpoint keeps its idempotency marker: {}",
            progress_comments[0].body
        );
        assert!(
            progress_comments[0]
                .body
                .contains("- [x] step 3: finish engineer run (engineer)"),
            "done checkpoint line renders checked: {}",
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
        job_payload: json!({ "correlation_key": "pr-for-code-9" }),
    }
}

fn progress(step: u32, state: &str, status: &str, pushed_sha: Option<&str>) -> JobProgress {
    JobProgress {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        correlation_key: "pr-for-code-9".to_string(),
        step,
        status: status.to_string(),
        state: state.to_string(),
        pushed_sha: pushed_sha.map(str::to_string),
        note: None,
    }
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
