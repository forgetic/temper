//! L2/L3 end-to-end scenarios over in-process memory and filesystem stages.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use temper_forge_model::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreateRepository, Forge, ItemNumber,
    PullRequestQuery, PullRequestState, RepositoryId, RepositoryPath, UpsertLabel,
};
use temper_forge_filesystem::FilesystemForge;
use temper_forge_memory::MemoryForge;
use temper_runner::{
    CiSink, FixpointDriver, MultiRepoMechanicalWorker, MultiRepoRoleWorker, Progress,
    RepositoryJournal, RepositorySet, RepositoryTarget, Scenario, Stage, StageError, Worker,
    WorkerError, run_scenario_with_budget,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, RoleId};

use temper_testing::agents::{
    ClosingArchitect, FakeArchitect, FakeReviewer, RequestChangesThenApproveReviewer,
    fake_registry, fake_registry_with,
};
use temper_testing::block_on;
use temper_testing::ci::{FailThenPassCiPolicy, FilesystemCiSink, FixedCiPolicy, MemoryCiSink};
use temper_testing::runner_config;
use temper_testing::scenarios::{
    changes_requested_then_approved, ci_fails_then_passes, cross_repo_fanout_converges,
    dependency_chain_mechanically_unblocked, happy_path,
};
use temper_testing::world::{
    full_reference_filesystem_stage, full_reference_filesystem_stage_with,
    full_reference_multiprocess_stage, full_reference_stage, full_reference_stage_with,
};

const HAPPY_PATH_BUDGET: u64 = 32;
const VARIANT_BUDGET: u64 = 96;

#[test]
fn end_to_end_reference_delivery_happy_path_converges() {
    let scenario = happy_path();

    // Expected fake-driven loop: intake -> triage_to_code (architect) ->
    // claim_code + create PR + request_review (engineer) -> CI passes
    // (CiWorker) + approve_review (reviewer) -> land_pr (mechanical) ->
    // reconcile_landed (architect audit) -> quiescent. Normal role scans skip
    // terminal merged PRs; the deterministic stage adds the architect audit
    // backstop to cover that recovery path. The scenario deliberately accepts
    // two current seams: the produced code issue may stay open after merge, and
    // the single PR keeps `alignment` because owner_alignment needs a cohort of
    // five (or max_age) before it activates.
    let memory_stage =
        block_on(full_reference_stage(runner_config())).expect("memory stage builds");
    assert_mechanical_landed_without_owner_merge(&assert_scenario_converges(
        "memory",
        &memory_stage,
        &scenario,
        HAPPY_PATH_BUDGET,
    ));

    let filesystem_stage = block_on(full_reference_filesystem_stage(runner_config()))
        .expect("filesystem stage builds");
    assert_mechanical_landed_without_owner_merge(&assert_scenario_converges(
        "filesystem",
        &filesystem_stage,
        &scenario,
        HAPPY_PATH_BUDGET,
    ));

    let multiprocess_stage = block_on(full_reference_multiprocess_stage(runner_config()))
        .expect("multi-process stage builds");
    assert_mechanical_landed_without_owner_merge(&assert_scenario_converges(
        "filesystem-multiprocess-sketch",
        &multiprocess_stage,
        &scenario,
        HAPPY_PATH_BUDGET,
    ));
}

#[test]
fn changes_requested_then_approved_converges_without_premature_merge() {
    let scenario = changes_requested_then_approved();

    assert_scenario_converges_on_backends(
        &scenario,
        VARIANT_BUDGET,
        || {
            block_on(full_reference_stage_with(
                runner_config(),
                fake_registry_with(FakeArchitect, RequestChangesThenApproveReviewer::new()),
                FixedCiPolicy::pass(),
            ))
        },
        || {
            block_on(full_reference_filesystem_stage_with(
                runner_config(),
                fake_registry_with(FakeArchitect, RequestChangesThenApproveReviewer::new()),
                FixedCiPolicy::pass(),
            ))
        },
    );
}

#[test]
fn ci_fails_then_passes_converges_without_premature_merge() {
    let scenario = ci_fails_then_passes();

    assert_scenario_converges_on_backends(
        &scenario,
        VARIANT_BUDGET,
        || {
            block_on(full_reference_stage_with(
                runner_config(),
                fake_registry(),
                FailThenPassCiPolicy,
            ))
        },
        || {
            block_on(full_reference_filesystem_stage_with(
                runner_config(),
                fake_registry(),
                FailThenPassCiPolicy,
            ))
        },
    );
}

#[test]
fn dependency_chain_is_mechanically_unblocked_and_merged() {
    let scenario = dependency_chain_mechanically_unblocked();

    assert_scenario_converges_on_backends(
        &scenario,
        VARIANT_BUDGET,
        || {
            block_on(full_reference_stage_with(
                runner_config(),
                fake_registry_with(ClosingArchitect, FakeReviewer),
                FixedCiPolicy::pass(),
            ))
        },
        || {
            block_on(full_reference_filesystem_stage_with(
                runner_config(),
                fake_registry_with(ClosingArchitect, FakeReviewer),
                FixedCiPolicy::pass(),
            ))
        },
    );
}

#[test]
fn cross_repo_fanout_converges_on_one_fixed_worker_fleet() {
    let scenario = cross_repo_fanout_converges();
    assert_cross_repo_converges_on_memory(&scenario, VARIANT_BUDGET);
    assert_cross_repo_converges_on_filesystem(&scenario, VARIANT_BUDGET);
}

fn assert_scenario_converges_on_backends<M, F, MB, FB>(
    scenario: &Scenario,
    budget: u64,
    memory_builder: MB,
    filesystem_builder: FB,
) where
    M: Stage,
    F: Stage,
    MB: FnOnce() -> Result<M, StageError>,
    FB: FnOnce() -> Result<F, StageError>,
{
    let memory_stage = memory_builder().expect("memory stage builds");
    assert_scenario_converges("memory", &memory_stage, scenario, budget);

    let filesystem_stage = filesystem_builder().expect("filesystem stage builds");
    assert_scenario_converges("filesystem", &filesystem_stage, scenario, budget);
}

fn assert_scenario_converges<S: Stage>(
    backend: &str,
    stage: &S,
    scenario: &Scenario,
    budget: u64,
) -> temper_runner::RunReport {
    let report = block_on(run_scenario_with_budget(stage, scenario, budget))
        .unwrap_or_else(|error| panic!("{backend} scenario passes: {error}"));

    assert!(
        report.ticks <= budget,
        "{backend} scenario used {} ticks, over budget {budget}",
        report.ticks
    );
    report
}

fn assert_mechanical_landed_without_owner_merge(report: &temper_runner::RunReport) {
    assert!(
        report.action_count("mechanical").unwrap_or(0) > 0,
        "mechanical worker should apply the landing transition"
    );
    assert_eq!(
        report.action_count("role:owner"),
        Some(0),
        "owner role should not service a normal merge queue"
    );
}

fn assert_cross_repo_converges_on_memory(scenario: &Scenario, budget: u64) {
    let forge = MemoryForge::new();
    let repos = block_on(provision_two_repos(&forge)).expect("memory repos provision");
    block_on((scenario.seed)(&forge, &repos[0].id)).expect("cross-repo scenario seeds");
    let config = runner_config();
    let role_forges = role_forges(&config, |user| forge.as_user(user));
    let report = block_on(run_cross_repo_fleet(
        &forge,
        &role_forges,
        repos.clone(),
        MemoryCiSink::new(forge.clone()),
        budget,
    ))
    .unwrap_or_else(|error| panic!("memory cross-repo fleet converges: {error}"));
    block_on((scenario.assert)(&forge, &repos[0].id)).expect("memory cross-repo assertion passes");
    assert!(
        report.ticks <= budget,
        "memory cross-repo scenario over budget"
    );
}

fn assert_cross_repo_converges_on_filesystem(scenario: &Scenario, budget: u64) {
    let root = TempRoot::new("runner-cross-repo");
    let forge = root.forge();
    let repos = block_on(provision_two_repos(&forge)).expect("filesystem repos provision");
    block_on((scenario.seed)(&forge, &repos[0].id)).expect("cross-repo scenario seeds");
    let config = runner_config();
    let role_forges = role_forges(&config, |user| forge.as_user(user));
    let report = block_on(run_cross_repo_fleet(
        &forge,
        &role_forges,
        repos.clone(),
        FilesystemCiSink::new(forge.clone()),
        budget,
    ))
    .unwrap_or_else(|error| panic!("filesystem cross-repo fleet converges: {error}"));
    block_on((scenario.assert)(&forge, &repos[0].id))
        .expect("filesystem cross-repo assertion passes");
    assert!(
        report.ticks <= budget,
        "filesystem cross-repo scenario over budget"
    );
}

fn role_forges<F, M>(
    config: &temper_runner::RunnerConfig,
    mut map: M,
) -> Vec<(temper_workflow::RoleId, F)>
where
    M: FnMut(temper_forge_model::User) -> F,
{
    config
        .role_bindings
        .iter()
        .map(|binding| (binding.role.clone(), map(binding.user.clone())))
        .collect()
}

async fn provision_two_repos<F: Forge + ?Sized>(
    forge: &F,
) -> Result<Vec<RepositoryTarget>, Box<dyn std::error::Error>> {
    let config = runner_config();
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let paths = [
        RepositoryPath::new(&config.repository.owner, &config.repository.name),
        RepositoryPath::new(&config.repository.owner, "service-canary"),
    ];
    let mut repos = Vec::new();
    for path in paths {
        let repo = forge
            .create_repository(CreateRepository {
                owner: path.owner.clone(),
                name: path.name.clone(),
                default_branch: config.repository.default_branch.clone(),
                description: None,
            })
            .await?;
        for label in compiled.labels().labels() {
            forge
                .upsert_label(
                    &repo.id,
                    UpsertLabel {
                        name: label.id.to_string(),
                        color: None,
                        description: None,
                    },
                )
                .await?;
        }
        repos.push(RepositoryTarget::new(repo.id, path));
    }
    Ok(repos)
}

async fn run_cross_repo_fleet<F, S>(
    forge: &F,
    role_forges: &[(temper_workflow::RoleId, F)],
    repos: Vec<RepositoryTarget>,
    ci_sink: S,
    budget: u64,
) -> Result<temper_runner::RunReport, Box<dyn std::error::Error>>
where
    F: Forge + 'static,
    S: CiSink + Send + Sync,
{
    let config = runner_config();
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let repo_set = RepositorySet::new(repos.clone());
    let ci_repos = repos.iter().map(|repo| repo.id.clone()).collect();
    let registry = fake_registry_with(ClosingArchitect, FakeReviewer);
    let journals: Vec<InMemoryJournal> = repos.iter().map(|_| InMemoryJournal::new()).collect();
    let bindings: Vec<RepositoryJournal<'_, InMemoryJournal>> = repos
        .iter()
        .zip(journals.iter())
        .map(|(repo, journal)| RepositoryJournal {
            repository: &repo.id,
            journal,
        })
        .collect();
    let mechanical = MultiRepoMechanicalWorker::new(
        &workflow,
        forge,
        repo_set.clone(),
        bindings,
        LeasePolicy::new(config.lease_ttl),
    )?;
    let mut workers: Vec<Box<dyn Worker + '_>> = vec![Box::new(mechanical)];
    for role in compiled.roles() {
        let Some(agent) = registry.get(&role.id) else {
            continue;
        };
        let Some((_, role_forge)) = role_forges.iter().find(|(role_id, _)| role_id == &role.id)
        else {
            continue;
        };
        workers.push(Box::new(MultiRepoRoleWorker::new(
            &workflow,
            &compiled,
            role_forge,
            repo_set.clone(),
            role.id.clone(),
            Arc::clone(agent),
            config.execution_context(&role.id),
        )));
        if role_uses_test_audit_backstop(&role.id) {
            workers.push(Box::new(MultiRepoRoleAuditWorker::new(
                &role.id,
                MultiRepoRoleWorker::new(
                    &workflow,
                    &compiled,
                    role_forge,
                    repo_set.clone(),
                    role.id.clone(),
                    Arc::clone(agent),
                    config.execution_context(&role.id),
                ),
            )));
        }
    }
    workers.push(Box::new(MultiRepoPassCiWorker::new(
        forge, ci_repos, ci_sink,
    )));
    let worker_refs: Vec<&dyn Worker> = workers.iter().map(|worker| worker.as_ref()).collect();
    Ok(FixpointDriver::new(worker_refs).run(budget).await?)
}

struct MultiRepoRoleAuditWorker<'a, F: Forge + ?Sized> {
    name: String,
    inner: MultiRepoRoleWorker<'a, F>,
}

impl<'a, F: Forge + ?Sized> MultiRepoRoleAuditWorker<'a, F> {
    fn new(role: &RoleId, inner: MultiRepoRoleWorker<'a, F>) -> Self {
        Self {
            name: format!("multi-role-audit:{role}"),
            inner,
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Worker for MultiRepoRoleAuditWorker<'_, F> {
    async fn tick(&self, now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        self.inner.tick_audit_report(now).await.into_worker_result()
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn role_uses_test_audit_backstop(role: &RoleId) -> bool {
    // Cross-repo scenario tests use the same fixed-point driver as the
    // single-repo test stages, so give the reference architect an explicit audit
    // worker to cover terminal recovery queues after normal scans skip merged
    // pull requests.
    role.as_str() == "architect"
}

struct MultiRepoPassCiWorker<'a, F: Forge + ?Sized, S> {
    forge: &'a F,
    repos: Vec<RepositoryId>,
    sink: S,
    visits: Mutex<BTreeMap<(RepositoryId, ItemNumber), u64>>,
}

impl<'a, F: Forge + ?Sized, S> MultiRepoPassCiWorker<'a, F, S> {
    fn new(forge: &'a F, repos: Vec<RepositoryId>, sink: S) -> Self {
        Self {
            forge,
            repos,
            sink,
            visits: Mutex::new(BTreeMap::new()),
        }
    }

    fn next_visit(&self, repo: &RepositoryId, number: ItemNumber) -> u64 {
        let mut visits = self.visits.lock().expect("CI visit mutex is poisoned");
        let visit = visits.entry((repo.clone(), number)).or_insert(0);
        *visit = visit.saturating_add(1);
        *visit
    }
}

#[async_trait]
impl<F, S> Worker for MultiRepoPassCiWorker<'_, F, S>
where
    F: Forge + ?Sized,
    S: CiSink,
{
    async fn tick(&self, _now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        let mut progress = Progress::unchanged();
        for repo in &self.repos {
            let pull_requests = self
                .forge
                .list_pull_requests(
                    repo,
                    PullRequestQuery {
                        state: Some(PullRequestState::Open),
                        labels: vec!["implementation".to_string()],
                        ..PullRequestQuery::default()
                    },
                )
                .await?;
            for pull_request in pull_requests {
                let jobs = self
                    .forge
                    .list_ci_jobs(
                        repo,
                        CiJobQuery {
                            pull_request_id: Some(pull_request.id.clone()),
                            commit_sha: pull_request.head_sha.clone(),
                            status: Some(CiJobStatus::Completed),
                            ..CiJobQuery::default()
                        },
                    )
                    .await?;
                if !jobs.is_empty() {
                    continue;
                }
                self.next_visit(repo, pull_request.number);
                self.sink
                    .record(repo, pull_request.number, CiJobConclusion::Success)
                    .await?;
                progress.record(true);
            }
        }
        Ok(progress)
    }

    fn name(&self) -> &str {
        "multi-ci"
    }
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(suite: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("temper-runner-{suite}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
