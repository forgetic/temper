// SPDX-License-Identifier: MPL-2.0

use crate::support::*;

#[test]
fn progress_checkpoints_are_recorded_once_per_step() {
    use temper_worker_protocol::JobProgress;

    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let number = create_ready_issue(&forge, &repo).await;
        let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));

        let job = InFlightJob {
            job_id: "acme/service/issue-1/engineer/code_ready".to_string(),
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
            artifact: Artifact {
                item: json!(number.get()),
                kind: "issue".to_string(),
            },
            job_payload: json!({ "correlation_key": "pr-for-code-9" }),
        };
        let progress = |step: u32, state: &str| JobProgress {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-a".to_string(),
            correlation_key: "pr-for-code-9".to_string(),
            step,
            status: "write failing test".to_string(),
            state: state.to_string(),
            pushed_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            note: None,
        };

        applier
            .apply_progress(job.clone(), progress(1, "done"))
            .await;
        // Re-delivery of the same (correlation_key, step, state) is a no-op.
        applier
            .apply_progress(job.clone(), progress(1, "done"))
            .await;
        // A different step (or phase) appends its own checkpoint.
        applier
            .apply_progress(job.clone(), progress(2, "started"))
            .await;

        let issue = forge
            .get_issue_by_number(&repo, number)
            .await
            .expect("issue lookup succeeds")
            .expect("issue exists");
        let comments = forge
            .list_issue_comments(&issue.id)
            .await
            .expect("comments list");
        let progress_comments: Vec<_> = comments
            .iter()
            .filter(|comment| comment.body.contains("temper-progress"))
            .collect();
        assert_eq!(
            progress_comments.len(),
            2,
            "duplicate delivery must not duplicate forge state: {progress_comments:?}"
        );
        assert!(
            progress_comments[0]
                .body
                .contains("- [x] step 1: write failing test (engineer, pushed 0123456789ab"),
            "checkpoint line renders: {}",
            progress_comments[0].body
        );
        assert!(
            progress_comments[1].body.contains("- [ ] step 2:"),
            "started phase renders unticked: {}",
            progress_comments[1].body
        );
    })
}
