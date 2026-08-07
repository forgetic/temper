use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jig_core::{Reply, Script, StopReason, Turn};
use temper_protocol_agent::SubmitForPrResponse;
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    HermeticIssueSpec, HermeticRealStackBuilder, HermeticRepoSpec, WorkerRoleSpec,
};

use super::{default_now, wait_for_issue_matching};

#[test]
fn hermetic_real_stack_basic_delivery_architect_triages_then_engineer_opens_pr() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let observed_architect = Arc::new(AtomicUsize::new(0));
        let observed_engineer_continuation = Arc::new(AtomicUsize::new(0));
        let host_validations = Arc::new(AtomicUsize::new(0));
        let validations_for_host = Arc::clone(&host_validations);
        let submit_host: temper_agent::SubmitForPrHost = Arc::new(move |request, _context, cwd| {
            assert_eq!(
                request.summary.as_deref(),
                Some("Implemented deterministic environment banner.")
            );
            assert_eq!(
                std::fs::read_to_string(cwd.join("service/BANNER.txt"))
                    .expect("host validates the agent write before accepting submit"),
                "environment: hermetic\n"
            );
            validations_for_host.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(SubmitForPrResponse::accepted(
                "host validated the banner",
            )))
        });
        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .issue(HermeticIssueSpec::untriaged_intake(
                "Service banner should identify the environment",
                "Operators need a deterministic environment banner in the service repo.",
            ))
            .workflow(temper_testing::basic_delivery_workflow())
            .worker_role(WorkerRoleSpec::architect())
            .add_worker_role(WorkerRoleSpec::engineer())
            .fake_model_script(basic_delivery_architect_then_engineer_script(
                Arc::clone(&observed_architect),
                Arc::clone(&observed_engineer_continuation),
            ))
            .submit_for_pr_host(submit_host)
            .build(&handle)
            .await
            .expect("basic-delivery hermetic stack builds");

        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("architect", default_now())
                .await
                .expect("architect triage scan enqueues"),
            1
        );
        stack.start_worker(&handle);

        let architect_result = stack
            .await_worker_result(&cx, Duration::from_secs(10))
            .await
            .expect("architect read-only native-agent turn reports a verdict");
        assert_eq!(architect_result.status, ResultStatus::Success);
        assert_eq!(architect_result.verdict.as_deref(), Some("ready_code"));
        assert!(architect_result.repos.is_empty());
        assert_eq!(
            observed_architect.load(Ordering::SeqCst),
            1,
            "fake Jig model should have served the architect turn"
        );

        let triaged = wait_for_issue_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "basic-delivery architect verdict to rewrite the intake into ready code",
            |issue| {
                issue.labels.iter().any(|label| label == "code")
                    && issue.labels.iter().any(|label| label == "ready")
                    && !issue.labels.iter().any(|label| label == "untriaged")
                    && issue.body.contains("## Code spec")
                    && issue.body.contains("Add `BANNER.txt`")
            },
        )
        .await;
        assert_eq!(
            triaged.assignees.len(),
            1,
            "triage transition should claim the source issue: {:?}",
            triaged.assignees
        );
        assert!(
            stack
                .pull_requests()
                .await
                .expect("pull requests list after triage")
                .is_empty(),
            "read-only architect triage must not open an implementation PR"
        );
        assert!(
            stack
                .origin_file(stack.primary_repo_path(), "main", "BANNER.txt")
                .is_err(),
            "read-only architect triage must not write product files"
        );

        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("engineer", default_now())
                .await
                .expect("engineer code_ready scan enqueues"),
            1
        );
        let engineer_result = loop {
            let result = stack
                .await_worker_result(&cx, Duration::from_secs(10))
                .await
                .expect("engineer native-agent turn reports a pushed product diff");
            if result.verdict.is_none() && !result.repos.is_empty() {
                break result;
            }
        };
        assert_eq!(engineer_result.status, ResultStatus::Success);
        assert_eq!(engineer_result.verdict.as_deref(), None);
        assert_eq!(engineer_result.repos.len(), 1);
        let outcome = &engineer_result.repos[0];
        assert_eq!(outcome.repo, stack.primary_repo_path());
        assert_eq!(
            stack
                .origin_file(
                    stack.primary_repo_path(),
                    &outcome.branch.name,
                    "BANNER.txt"
                )
                .expect("engineer product file exists on pushed branch"),
            "environment: hermetic\n"
        );

        let pulls = stack
            .wait_for_pull_request_count(&cx, 1, Duration::from_secs(5))
            .await
            .expect("engineer success opens one implementation PR");
        let pull = &pulls[0];
        assert_eq!(pull.source.branch, outcome.branch.name);
        assert_eq!(pull.target.branch, "main");
        assert!(pull.labels.iter().any(|label| label == "implementation"));
        assert!(pull.labels.iter().any(|label| label == "landing"));
        assert!(
            pull.body
                .contains("Implemented deterministic environment banner."),
            "PR body should include engineer summary: {}",
            pull.body
        );
        let metadata = temper_workflow::parse_metadata_block(&pull.body)
            .expect("basic-delivery PR metadata parses")
            .expect("basic-delivery PR has workflow metadata");
        assert_eq!(
            metadata.parents,
            vec![temper_workflow::ArtifactRef::same_repo(
                stack.issue_number()
            )]
        );

        let finalized = wait_for_issue_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "basic-delivery source issue finalized after implementation PR creation",
            |issue| issue.labels == vec!["code".to_string()],
        )
        .await;
        assert!(finalized.body.contains("## Code spec"));
        assert!(!finalized.body.contains("Temper run ledger"));
        assert_eq!(
            observed_engineer_continuation.load(Ordering::SeqCst),
            1,
            "fake Jig model should have observed the engineer tool-result continuation"
        );
        assert_eq!(
            host_validations.load(Ordering::SeqCst),
            1,
            "the write, host validation, submit, and final Jig response form one accepted delivery"
        );
    });
}

fn basic_delivery_architect_then_engineer_script(
    observed_architect: Arc<AtomicUsize>,
    observed_engineer_continuation: Arc<AtomicUsize>,
) -> Script {
    let turn = Arc::new(AtomicUsize::new(0));
    Script::rule(move |view| {
        if view.prior_tool_results == 0 {
            match turn.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    observed_architect.fetch_add(1, Ordering::SeqCst);
                    Reply::text(
                        serde_json::json!({
                            "verdict": "ready_code",
                            "summary": "Triaged intake into a ready code spec.",
                            "body": "## Code spec\n\nAdd `BANNER.txt` with the exact contents `environment: hermetic` so operators can identify this environment."
                        })
                        .to_string(),
                    )
                }
                1 => Reply {
                    turns: vec![Turn::ToolCall {
                        id: "call_write_banner".to_string(),
                        name: "write".to_string(),
                        args: serde_json::json!({
                            "path": "service/BANNER.txt",
                            "content": "environment: hermetic\n"
                        }),
                    }],
                    usage: Default::default(),
                    stop: StopReason::ToolCalls,
                },
                extra => Reply::text(
                    serde_json::json!({
                        "summary": format!("Ignored unexpected extra basic-delivery model turn {extra}.")
                    })
                    .to_string(),
                ),
            }
        } else if view.prior_tool_results == 1 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_submit_for_pr".to_string(),
                    name: "submit_for_pr".to_string(),
                    args: serde_json::json!({
                        "summary": "Implemented deterministic environment banner."
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            observed_engineer_continuation.fetch_add(1, Ordering::SeqCst);
            Reply::text(r#"{"summary":"Implemented deterministic environment banner."}"#)
        }
    })
}
