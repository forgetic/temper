fn runner_loss_stack_builder() -> HermeticRealStackBuilder {
    let builder =
        HermeticRealStackBuilder::new().workflow(temper_testing::basic_delivery_workflow());
    #[cfg(target_os = "linux")]
    let builder =
        builder.linux_supervisor_helper(env!("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"));
    builder
}

fn diagnostic_stack(sessions: Arc<AtomicUsize>) -> HermeticRealStackBuilder {
    runner_loss_stack_builder()
        .issue(HermeticIssueSpec::ready_code(
            "Interrupted CI diagnostic fallback",
            "Diagnose runner loss read-only and park without changing the PR head.",
        ))
        .add_worker_role(WorkerRoleSpec::ci_diagnostician())
        .fake_model_script(diagnostic_script(sessions))
}

async fn wait_for_assignment_clear(
    cx: &skein::cx::Cx,
    stack: &HermeticRealStack,
    number: ItemNumber,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let metadata = parse_metadata_block(&current_pull(stack, number).await.body)
            .unwrap()
            .unwrap_or_default();
        if metadata.assignment.is_none() && metadata.lease.is_none() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for diagnostic assignment release: {metadata:?}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn await_result_for_role(
    cx: &skein::cx::Cx,
    stack: &mut HermeticRealStack,
    job_id_suffix: &str,
) -> JobResult {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match stack
            .await_worker_result(cx, Duration::from_millis(500))
            .await
        {
            Ok(result) if result.job_id.contains(job_id_suffix) => return result,
            Ok(_) => {}
            Err(error) if error.contains("timed out") && Instant::now() < deadline => {}
            Err(error) => panic!("waiting for role result `{job_id_suffix}`: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for role result `{job_id_suffix}`"
        );
    }
}

async fn open_initial_pull(
    cx: &skein::cx::Cx,
    handle: &skein::runtime::RuntimeHandle,
    stack: &mut HermeticRealStack,
) -> (PullRequest, String, String) {
    let initial = stack
        .run_open_pr_job(cx, handle)
        .await
        .expect("initial implementation completes");
    assert_eq!(
        initial.job_result.status,
        ResultStatus::Success,
        "initial result failed: {:?}",
        initial.job_result.failure
    );
    assert_eq!(initial.pull_requests.len(), 1);
    stack.crash_worker().await;
    let mut pull = initial.pull_requests[0].clone();
    let head = initial.job_result.repos[0].branch.head_sha.clone();
    let branch = initial.job_result.repos[0].branch.name.clone();
    pull = stack
        .forge()
        .set_pull_request_head(&pull.id, Some(head.clone()))
        .expect("Forge observes initial PR head");
    stack
        .publish_pull_request_head_ref(pull.number, &head)
        .expect("provider-style PR head ref is available to read-only diagnostics");
    (pull, head, branch)
}

fn running_attempt(stack: &HermeticRealStack, head: &str, attempt: &str) -> HermeticCiAttempt {
    let updated = stack.clock().now() + chrono::Duration::minutes(1);
    HermeticCiAttempt::new(head, RUN_ID, attempt).job(
        HermeticCiJobSpec::new("validate", CiJobStatus::Running, None)
            .provider_evidence("running", "quick tests passed; end-to-end tests started")
            .url(RUN_URL)
            .timestamps(
                updated - chrono::Duration::minutes(16),
                Some(updated - chrono::Duration::minutes(15)),
                None,
                updated,
            ),
    )
}

fn ambiguous_forgejo_failure_attempt(
    stack: &HermeticRealStack,
    head: &str,
    attempt: &str,
) -> HermeticCiAttempt {
    let completed = stack.clock().now() + chrono::Duration::minutes(17);
    let mut job = HermeticCiJobSpec::new(
        "validate",
        CiJobStatus::Completed,
        Some(CiJobConclusion::Unknown),
    );
    job.provider_conclusion = Some("failure".to_string());
    HermeticCiAttempt::new(head, RUN_ID, attempt).job(job.url(RUN_URL).timestamps(
        completed - chrono::Duration::minutes(32),
        Some(completed - chrono::Duration::minutes(31)),
        Some(completed),
        completed,
    ))
}

fn runner_lost_attempt(stack: &HermeticRealStack, head: &str, attempt: &str) -> HermeticCiAttempt {
    let completed = stack.clock().now() + chrono::Duration::minutes(17);
    HermeticCiAttempt::new(head, RUN_ID, attempt).job(
        HermeticCiJobSpec::new(
            "validate",
            CiJobStatus::Completed,
            Some(CiJobConclusion::RunnerLost),
        )
        .provider_evidence(
            "failure",
            "runner process disappeared after host restart; no terminal test failure",
        )
        .url(RUN_URL)
        .timestamps(
            completed - chrono::Duration::minutes(32),
            Some(completed - chrono::Duration::minutes(31)),
            Some(completed),
            completed,
        ),
    )
}

fn pending_attempt(stack: &HermeticRealStack, head: &str, attempt: &str) -> HermeticCiAttempt {
    let updated = stack.clock().now() + chrono::Duration::minutes(18);
    HermeticCiAttempt::new(head, RUN_ID, attempt).job(
        HermeticCiJobSpec::new("validate", CiJobStatus::Queued, None)
            .provider_evidence("queued", "provider retry accepted")
            .url(RUN_URL)
            .timestamps(updated, None, None, updated),
    )
}

fn successful_attempt(stack: &HermeticRealStack, head: &str, attempt: &str) -> HermeticCiAttempt {
    let completed = stack.clock().now() + chrono::Duration::minutes(24);
    HermeticCiAttempt::new(head, RUN_ID, attempt).job(
        HermeticCiJobSpec::new(
            "validate",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Success),
        )
        .provider_evidence("success", "all configured tests completed")
        .url(RUN_URL)
        .timestamps(
            completed - chrono::Duration::minutes(5),
            Some(completed - chrono::Duration::minutes(4)),
            Some(completed),
            completed,
        ),
    )
}

fn failed_attempt(stack: &HermeticRealStack, head: &str, attempt: &str) -> HermeticCiAttempt {
    let completed = stack.clock().now() + chrono::Duration::minutes(24);
    HermeticCiAttempt::new(head, RUN_ID, attempt).job(
        HermeticCiJobSpec::new(
            "validate",
            CiJobStatus::Completed,
            Some(CiJobConclusion::Failure),
        )
        .provider_evidence(
            "failure",
            "test recovery::ordinary_failure failed with assertion",
        )
        .url(RUN_URL)
        .timestamps(
            completed - chrono::Duration::minutes(5),
            Some(completed - chrono::Duration::minutes(4)),
            Some(completed),
            completed,
        ),
    )
}

fn submit_terminal_hint(stack: &HermeticRealStack, number: ItemNumber, head: &str, jobs: &[CiJob]) {
    let status = CiStatus::from_jobs(jobs);
    assert!(status.is_recovery_required());
    stack
        .daemon()
        .submit_ci_poll_transition(CiStatusTransition::Terminal(CiTerminalTransition {
            hint: ChangeHint::pull_request(
                RepositoryPath::new("acme", "service"),
                number,
                ChangeKind::Ci,
            ),
            head_sha: head.to_string(),
            verdict: CiTerminalVerdict::RecoveryRequired,
            terminal_evidence: status.terminal_evidence().to_vec(),
            completed_at: jobs.iter().filter_map(|job| job.completed_at).max(),
        }));
}

async fn wait_for_recovery_marker(
    cx: &skein::cx::Cx,
    stack: &HermeticRealStack,
    number: ItemNumber,
    expected: bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if recovery_state(stack, number).await.is_some() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for interrupted-CI marker presence={expected}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn recovery_state(
    stack: &HermeticRealStack,
    number: ItemNumber,
) -> Option<temper_workflow::InterruptedCiRecoveryState> {
    parse_metadata_block(&current_pull(stack, number).await.body)
        .unwrap()
        .unwrap_or_default()
        .interrupted_ci_recovery
}

async fn current_pull(stack: &HermeticRealStack, number: ItemNumber) -> PullRequest {
    stack
        .forge()
        .get_pull_request_by_number(stack.primary_repo_id(), number)
        .await
        .expect("pull request read")
        .expect("pull request exists")
}

async fn ci_jobs_for_attempt(stack: &HermeticRealStack, attempt: &str) -> Vec<CiJob> {
    stack
        .forge()
        .list_ci_jobs(stack.primary_repo_id(), Default::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|job| job.attempt.as_deref() == Some(attempt))
        .collect()
}

fn assert_one_exact_retry(
    stack: &HermeticRealStack,
    pull: &PullRequest,
    head: &str,
    attempt: &str,
) {
    let requests = stack.forge().ci_retry_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.repo_id(), stack.primary_repo_id());
    assert_eq!(request.pull_request_id(), &pull.id);
    assert_eq!(request.head_sha(), head);
    assert_eq!(request.run_id(), RUN_ID);
    assert_eq!(request.attempt(), attempt);
}

async fn assert_unchanged_head_and_history(
    stack: &HermeticRealStack,
    pull: &PullRequest,
    head: &str,
    branch: &str,
    original_log: &[String],
) {
    assert_eq!(
        current_pull(stack, pull.number).await.head_sha.as_deref(),
        Some(head)
    );
    assert_eq!(
        stack
            .origin_rev(stack.primary_repo_path(), branch)
            .expect("branch head"),
        head
    );
    assert_eq!(
        stack
            .origin_log_subjects(stack.primary_repo_path(), branch, 8)
            .expect("branch history"),
        original_log
    );
}

async fn assert_actionable_single_audit(stack: &HermeticRealStack, pull: &PullRequest, head: &str) {
    let comments = stack
        .forge()
        .list_pull_request_comments(&pull.id)
        .await
        .unwrap();
    assert_eq!(comments.len(), 1);
    let body = &comments[0].body;
    for evidence in [
        head,
        "Run: `591`",
        "Attempt: `1`",
        "RunnerLost",
        "runner process disappeared after host restart",
        RUN_URL,
        "Provider retry:",
        "Diagnostic recovery:",
        "clear `needs-human` only after a newer exact-head attempt is visible",
        "temper:comment-key=interrupted_ci_recovery:",
    ] {
        assert!(
            body.contains(evidence),
            "audit omitted `{evidence}`:\n{body}"
        );
    }
}

fn numbered_repair_script(sessions: Arc<AtomicUsize>) -> Script {
    Script::rule(move |view| match view.prior_tool_results {
        0 => {
            let session = sessions.fetch_add(1, Ordering::SeqCst);
            Reply {
                turns: vec![Turn::ToolCall {
                    id: format!("write-recovery-session-{session}"),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "service/RECOVERY.md",
                        "content": if session == 0 {
                            "initial implementation\n"
                        } else {
                            "ordinary failure repaired\n"
                        },
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        }
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: "submit-recovery-session".to_string(),
                name: "submit_for_pr".to_string(),
                args: serde_json::json!({ "summary": "recovery session complete" }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(r#"{"summary":"Recovery session complete."}"#),
    })
}

fn diagnostic_script(sessions: Arc<AtomicUsize>) -> Script {
    Script::rule(move |view| match view.prior_tool_results {
        0 => {
            let session = sessions.fetch_add(1, Ordering::SeqCst);
            if session == 0 {
                Reply {
                    turns: vec![Turn::ToolCall {
                        id: "write-initial-diagnostic-fixture".to_string(),
                        name: "write".to_string(),
                        args: serde_json::json!({
                            "path": "service/RECOVERY.md",
                            "content": "initial implementation\n",
                        }),
                    }],
                    usage: Default::default(),
                    stop: StopReason::ToolCalls,
                }
            } else {
                Reply::text(
                    serde_json::json!({
                        "verdict": "diagnosed",
                        "summary": "Runner execution disappeared after tests started; retrigger this exact head after runner remediation."
                    })
                    .to_string(),
                )
            }
        }
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: "submit-initial-diagnostic-fixture".to_string(),
                name: "submit_for_pr".to_string(),
                args: serde_json::json!({ "summary": "initial diagnostic fixture" }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(r#"{"summary":"Initial diagnostic fixture complete."}"#),
    })
}
