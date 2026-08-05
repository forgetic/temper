//! Terminal-history request-budget and real-Forgejo benchmark coverage.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
struct WarmHistoryPassBudget {
    provider_requests: u64,
    issue_candidate_lists: usize,
    pull_request_candidate_lists: usize,
    exact_issue_reads: usize,
    exact_pull_request_reads: usize,
    dependency_detail_reads: usize,
    ci_reads: usize,
    review_reads: usize,
    comment_reads: usize,
}

fn repeated_history_pass_budgets(history_count: usize) -> Vec<WarmHistoryPassBudget> {
    let memory = MemoryForge::new();
    let repository = block_on(memory.create_repository(CreateRepository {
        owner: "acme".into(),
        name: format!("history-{history_count}"),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("history repository is created");

    for number in 0..history_count {
        let issue = block_on(memory.create_issue(
            &repository.id,
            CreateIssue {
                title: format!("inert closed issue {number}"),
                body: String::new(),
                labels: vec![
                    "code".into(),
                    "planned".into(),
                    "validated".into(),
                    "needs-human".into(),
                ],
                assignees: Vec::new(),
            },
        ))
        .expect("history issue is created");
        block_on(memory.update_issue(
            &issue.id,
            UpdateIssue {
                state: Some(IssueState::Closed),
                ..UpdateIssue::default()
            },
        ))
        .expect("history issue is closed");

        let pull_request = create_pr(
            &memory,
            &repository.id,
            &format!("inert merged PR {number}"),
            &format!("history-{number}"),
            &["implementation", "landed", "needs-human"],
        );
        block_on(memory.merge_pull_request(
            &pull_request.id,
            MergePullRequest {
                method: MergeMethod::Squash,
                commit_title: None,
                commit_body: None,
                delete_source_branch: false,
            },
        ))
        .expect("history PR is merged");
    }

    let forge = CountingForge::new(memory);
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let journal = InMemoryJournal::new();
    let discovery = TerminalDiscoveryState::default();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repository.id,
        &journal,
        LeasePolicy::new(chrono::Duration::minutes(30)),
    )
    .with_terminal_discovery_state(discovery.clone());
    let roles = configured_roles();
    let mut budgets = Vec::new();

    for generation in 0..3 {
        let provider_before = forge.provider_request_count().unwrap();
        let shape_before = forge.read_shape();
        let progress = block_on(worker.tick(temper_testing::ts(&format!(
            "2026-05-29T00:00:0{generation}Z"
        ))))
        .expect("warm mechanical history pass succeeds");
        assert!(!progress.changed, "inert history never mutates");
        let role_items = block_on(scan_roles_wake_with_discovery(
            &forge,
            &repository.id,
            &workflow,
            &compiled,
            temper_testing::ts(&format!("2026-05-29T00:01:0{generation}Z")),
            &roles,
            &discovery,
            TerminalDiscoveryRead::RetainedOnly,
        ))
        .expect("warm broad role history pass succeeds");
        assert!(
            role_items.is_empty(),
            "inert history never enters a role feed"
        );
        let shape_after = forge.read_shape();
        let snapshot = discovery.snapshot(&repository.id).expect("discovery state");
        assert!(
            snapshot.retained_targets.is_empty(),
            "irrelevant rows never become retained exact targets"
        );

        budgets.push(WarmHistoryPassBudget {
            provider_requests: forge
                .provider_request_count()
                .unwrap()
                .saturating_sub(provider_before),
            issue_candidate_lists: shape_after
                .issue_candidate_list_calls
                .saturating_sub(shape_before.issue_candidate_list_calls),
            pull_request_candidate_lists: shape_after
                .pull_request_candidate_list_calls
                .saturating_sub(shape_before.pull_request_candidate_list_calls),
            exact_issue_reads: shape_after
                .exact_issue_reads
                .saturating_sub(shape_before.exact_issue_reads),
            exact_pull_request_reads: shape_after
                .exact_pull_request_reads
                .saturating_sub(shape_before.exact_pull_request_reads),
            dependency_detail_reads: shape_after
                .dependency_detail_reads
                .saturating_sub(shape_before.dependency_detail_reads),
            ci_reads: shape_after.ci_reads.saturating_sub(shape_before.ci_reads),
            review_reads: shape_after
                .review_reads
                .saturating_sub(shape_before.review_reads),
            comment_reads: shape_after
                .comment_reads
                .saturating_sub(shape_before.comment_reads),
        });
    }
    budgets
}

pub(super) fn assert_repeated_mechanical_and_role_budgets_ignore_large_labelled_terminal_history() {
    let zero_history = repeated_history_pass_budgets(0);
    let large_history = repeated_history_pass_budgets(250);
    assert_eq!(large_history, zero_history);
    for pass in large_history {
        assert!(
            pass.issue_candidate_lists
                .saturating_add(pass.pull_request_candidate_lists)
                <= 8,
            "each combined warm pass keeps its fixed logical-list ceiling: {pass:?}"
        );
        assert_eq!(pass.exact_issue_reads, 0);
        assert_eq!(pass.exact_pull_request_reads, 0);
        assert_eq!(pass.dependency_detail_reads, 0);
        assert_eq!(pass.ci_reads, 0);
        assert_eq!(pass.review_reads, 0);
        assert_eq!(pass.comment_reads, 0);
    }
}

async fn seed_forgejo_labelled_terminal_history(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    count: usize,
) {
    for label in ["planned", "validated"] {
        forge
            .upsert_label(
                repository,
                UpsertLabel {
                    name: label.to_string(),
                    color: Some("6e7781".to_string()),
                    description: Some("idle benchmark terminal history".to_string()),
                },
            )
            .await
            .expect("benchmark history label is provisioned");
    }

    for number in 0..count {
        let issue = forge
            .create_issue(
                repository,
                CreateIssue {
                    title: format!("benchmark inert terminal issue {number}"),
                    body: String::new(),
                    labels: vec![
                        "code".into(),
                        "planned".into(),
                        "validated".into(),
                        "needs-human".into(),
                    ],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("benchmark history issue is created");
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("benchmark history issue is closed");

        let branch = format!("terminal-history-{number}");
        forge
            .create_branch(
                repository,
                CreateBranch {
                    new_branch: branch.clone(),
                    from_branch: "main".to_string(),
                },
            )
            .await
            .expect("benchmark history branch is created");
        forge
            .commit_file(
                repository,
                CommitFile {
                    path: format!("history/{number}.md"),
                    contents: format!("terminal history {number}\n").into_bytes(),
                    message: format!("seed terminal history {number}"),
                    branch: branch.clone(),
                },
            )
            .await
            .expect("benchmark history commit is created");
        let pull_request = forge
            .create_pull_request(
                repository,
                temper_testing::pull_request_input(
                    repository,
                    format!("benchmark inert terminal PR {number}"),
                    "",
                    branch,
                    vec![
                        "implementation".into(),
                        "landed".into(),
                        "needs-human".into(),
                    ],
                ),
            )
            .await
            .expect("benchmark history PR is created");
        forge
            .merge_pull_request(
                &pull_request.id,
                MergePullRequest {
                    method: MergeMethod::Squash,
                    commit_title: None,
                    commit_body: None,
                    delete_source_branch: false,
                },
            )
            .await
            .expect("benchmark history PR is merged");
    }
}

pub(super) fn run_local_forgejo_two_pass_idle_broad_benchmark() {
    temper_engine_io::block_on(async move {
        let cached = skein::runtime::spawn_blocking(
            temper_testing::forgejo_server::start_cached_provisioned_server,
        )
        .await
        .expect("cached Forgejo fixture starts");
        let server = cached.server;
        let provisioned = cached.provisioned;
        let base_url = server.base_url().to_string();
        let setup = ForgejoForge::new(ForgejoConfig::new(&base_url, &provisioned.admin_token));
        let dependency = setup
            .create_issue(
                &provisioned.repository,
                CreateIssue {
                    title: "Idle benchmark unresolved dependency".into(),
                    body: String::new(),
                    labels: vec!["design".into(), "draft".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("benchmark dependency is created");
        let blocked = setup
            .create_issue(
                &provisioned.repository,
                CreateIssue {
                    title: "Idle benchmark blocked code".into(),
                    body: String::new(),
                    labels: vec!["code".into(), "blocked".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("benchmark source is created");
        setup
            .add_issue_dependency(&blocked.id, dependency.number)
            .await
            .expect("benchmark dependency link is created");

        let client = CountingHttpClient::new(EngineHttpClient::new(&base_url));
        let forge = Arc::new(ForgejoForge::with_client(
            ForgejoConfig::new(&base_url, &provisioned.admin_token),
            client.clone(),
        ));
        let path = RepositoryPath::new(&provisioned.owner, &provisioned.name);
        let target = RepositoryTarget::new(provisioned.repository.clone(), path.clone());
        let trigger = MechanicalTrigger::new(
            Arc::clone(&forge),
            Arc::new(temper_testing::workflow()),
            MechanicalBackstopConfig {
                repositories: RepositorySet::new(vec![target]),
                cadence: Duration::from_secs(300),
                lease_policy: LeasePolicy::new(chrono::Duration::minutes(30)),
                pull_request_merge_observer: None,
            },
            Arc::new(|| temper_testing::ts("2026-05-29T00:00:00Z")),
        );

        let cold_started = Instant::now();
        trigger
            .run_coordinated_broad(path.clone())
            .await
            .expect("zero-history cold broad pass succeeds");
        println!(
            "history_rows=0 phase=broad.cold_sweep generation=1 duration_ms={}",
            cold_started.elapsed().as_millis()
        );

        let zero_warm_start = client.request_count();
        let zero_warm_started = Instant::now();
        trigger
            .run_coordinated_broad(path.clone())
            .await
            .expect("zero-history warm broad pass succeeds");
        let zero_warm_shape = client.forgejo_read_shape_since(zero_warm_start);
        println!(
            "history_rows=0 phase=broad.warm duration_ms={} shape={zero_warm_shape:?}",
            zero_warm_started.elapsed().as_millis()
        );

        const HISTORY_PER_ARTIFACT_TYPE: usize = 120;
        let seed_started = Instant::now();
        seed_forgejo_labelled_terminal_history(
            &setup,
            &provisioned.repository,
            HISTORY_PER_ARTIFACT_TYPE,
        )
        .await;
        println!(
            "phase=seed terminal_issues={HISTORY_PER_ARTIFACT_TYPE} terminal_prs={HISTORY_PER_ARTIFACT_TYPE} duration_ms={}",
            seed_started.elapsed().as_millis()
        );
        trigger
            .terminal_discovery_state()
            .invalidate_repository(&provisioned.repository);

        let mut cold_generation = 0usize;
        while trigger
            .terminal_discovery_state()
            .snapshot(&provisioned.repository)
            .is_none_or(|snapshot| !snapshot.authoritative)
        {
            cold_generation = cold_generation.saturating_add(1);
            assert!(
                cold_generation <= 4,
                "cold sweep must make bounded progress"
            );
            let start_index = client.request_count();
            let started = Instant::now();
            trigger
                .run_coordinated_broad(path.clone())
                .await
                .expect("labelled-history cold sweep generation succeeds");
            let shape = client.forgejo_read_shape_since(start_index);
            println!(
                "history_rows={} phase=broad.cold_sweep generation={cold_generation} duration_ms={} shape={shape:?}",
                HISTORY_PER_ARTIFACT_TYPE * 2,
                started.elapsed().as_millis()
            );
            assert_eq!(shape.exact_artifact_reads, 0, "inert rows are not hydrated");
            assert_eq!(
                shape.dependency_requests, 0,
                "inert rows have no relation hydration"
            );
        }

        let history_warm_start = client.request_count();
        let history_warm_started = Instant::now();
        trigger
            .run_coordinated_broad(path)
            .await
            .expect("labelled-history warm broad pass succeeds");
        let history_warm_shape = client.forgejo_read_shape_since(history_warm_start);
        println!(
            "history_rows={} phase=broad.warm duration_ms={} shape={history_warm_shape:?}",
            HISTORY_PER_ARTIFACT_TYPE * 2,
            history_warm_started.elapsed().as_millis()
        );
        for (method_path, count) in client.normalized_method_path_counts_since(history_warm_start) {
            println!("history_warm_requests count={count} {method_path}");
        }

        let fixed_warm_provider_ceiling =
            MAX_PERIODIC_TERMINAL_CANDIDATE_PROVIDER_REQUESTS.saturating_add(5);
        for (history_rows, shape) in [
            (0, zero_warm_shape),
            (HISTORY_PER_ARTIFACT_TYPE * 2, history_warm_shape),
        ] {
            let candidate_protocol_requests = shape
                .candidate_list_requests
                .saturating_add(shape.exact_artifact_reads);
            assert!(
                candidate_protocol_requests <= fixed_warm_provider_ceiling,
                "history_rows={history_rows} exceeded fixed per-pass provider ceiling {fixed_warm_provider_ceiling}: {shape:?}"
            );
            assert_eq!(
                shape.exact_artifact_reads, 0,
                "history_rows={history_rows} must not reload per-artifact detail: {shape:?}"
            );
            assert_eq!(
                shape.dependency_requests, 0,
                "history_rows={history_rows} must not reload dependencies: {shape:?}"
            );
            assert_eq!(
                shape.ci_requests, 0,
                "idle history must not read CI: {shape:?}"
            );
            assert_eq!(
                shape.review_requests, 0,
                "idle history must not read reviews: {shape:?}"
            );
            assert_eq!(
                shape.comment_requests, 0,
                "idle history must not read comments: {shape:?}"
            );
            assert_eq!(
                shape.other_reads, 0,
                "idle warm pass must contain only candidate lists: {shape:?}"
            );
        }
    });
}
