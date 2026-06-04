//! Runtime construction for `temper-worker`.

use std::error::Error;
use std::fmt;
use std::time::{Duration as StdDuration, Instant};

use crate::wake::{wait_for_wake_or_poll, WakeConfig, WakeListener, WakeWaitOutcome};
use crate::worker_args::{ForgejoArgs, WorkerArgs, WorkerKind};
use crate::worker_external_tools::configure_external_tool_executors;
use crate::worker_role_agent::build_role_agent;
use crate::{runner_config, workflow};
use temper_forge::{ChangeHint, Forge, ForgeError, RepositoryPath, UpsertLabel};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::{
    render_worker_capability_event, MultiRepoMechanicalWorker, MultiRepoRoleWorker,
    RepositoryJournal, RepositorySet, RepositoryTarget, RunReport, Worker, WorkerCapabilitySummary,
    WorkerError, WorkerRunReport,
};
use temper_workflow::{CommandJournal, InMemoryJournal, LeasePolicy, RoleId};

#[derive(Debug)]
pub enum RunError {
    Forge(ForgeError),
    RepositoryUnavailable { owner: String, name: String },
    UnknownRole { role: String },
    Drive(Box<dyn Error + Send + Sync + 'static>),
    Backend(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Forge(error) => write!(formatter, "forge operation failed: {error}"),
            RunError::RepositoryUnavailable { owner, name } => write!(
                formatter,
                "repository {owner}/{name} not found or not readable by this worker token"
            ),
            RunError::UnknownRole { role } => {
                write!(formatter, "no agent registered for role '{role}'")
            }
            RunError::Drive(error) => write!(formatter, "worker run failed: {error}"),
            RunError::Backend(message) => write!(formatter, "backend setup failed: {message}"),
        }
    }
}

impl Error for RunError {}

impl From<ForgeError> for RunError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

/// Runs the production worker to completion.
pub fn run(args: &WorkerArgs) -> Result<RunReport, RunError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RunError::Backend(format!("failed to start Tokio runtime: {error}")))?;
    runtime.block_on(run_async(args))
}

async fn run_async(args: &WorkerArgs) -> Result<RunReport, RunError> {
    let forge = build_forge(&args.forgejo);
    match &args.kind {
        WorkerKind::Role { role, user: _ } => run_role(args, &forge, role).await,
        WorkerKind::Mechanical => run_mechanical(args, &forge).await,
    }
}

fn build_forge(forgejo: &ForgejoArgs) -> ForgejoForge {
    let mut config = ForgejoConfig::new(forgejo.base_url.clone(), forgejo.token.clone());
    if let (Some(username), Some(password)) = (&forgejo.username, &forgejo.password) {
        config = config.with_web_ui_credentials(username, password);
    }
    ForgejoForge::new(config)
}

async fn run_role(
    args: &WorkerArgs,
    forge: &ForgejoForge,
    role: &str,
) -> Result<RunReport, RunError> {
    let workflow = workflow();
    let compiled = workflow.compile();
    let mut config = runner_config();
    let external_tool_executors =
        configure_external_tool_executors(&compiled, &mut config).map_err(RunError::Backend)?;
    let role_id = RoleId::new(role);
    let role_manifest = compiled
        .role(&role_id)
        .ok_or_else(|| RunError::UnknownRole { role: role.into() })?;
    let bound_external_tool_ids = config
        .bound_external_tools_for(role_manifest)
        .map_err(|error| RunError::Backend(error.to_string()))?
        .into_iter()
        .map(|tool| tool.id.to_string())
        .collect::<Vec<_>>();
    let authorized_actions = role_manifest
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let agent = build_role_agent(
        args,
        &compiled,
        &config,
        external_tool_executors,
        &role_id,
        role_manifest,
    )
    .map_err(RunError::Backend)?;
    let repositories = resolve_repositories(forge, &args.repositories).await?;
    ensure_workflow_labels(forge, &repositories, &compiled).await?;
    log_repository_set("role", role, &repositories);
    log_worker_capabilities(WorkerCapabilitySummary {
        worker_kind: "role".to_string(),
        worker: format!("multi-role:{role}"),
        role: Some(role.to_string()),
        repositories: repository_display_paths(&repositories),
        responder_mode: "process".to_string(),
        authorized_actions,
        bound_external_tool_ids,
    });
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        forge as &dyn Forge,
        repositories,
        role_id.clone(),
        agent,
        config.execution_context(&role_id),
    );
    drive_async(args, &worker).await
}

async fn run_mechanical(args: &WorkerArgs, forge: &ForgejoForge) -> Result<RunReport, RunError> {
    let workflow = workflow();
    let compiled = workflow.compile();
    let config = runner_config();
    let repositories = resolve_repositories(forge, &args.repositories).await?;
    ensure_workflow_labels(forge, &repositories, &compiled).await?;
    log_repository_set("mechanical", "mechanical", &repositories);
    log_worker_capabilities(WorkerCapabilitySummary {
        worker_kind: "mechanical".to_string(),
        worker: "multi-mechanical".to_string(),
        role: None,
        repositories: repository_display_paths(&repositories),
        responder_mode: "none".to_string(),
        authorized_actions: Vec::new(),
        bound_external_tool_ids: Vec::new(),
    });
    let journals: Vec<InMemoryJournal> = repositories
        .repositories()
        .iter()
        .map(|_| InMemoryJournal::new())
        .collect();
    let journal_bindings: Vec<RepositoryJournal<'_, InMemoryJournal>> = repositories
        .repositories()
        .iter()
        .zip(journals.iter())
        .map(|(repository, journal)| RepositoryJournal {
            repository: &repository.id,
            journal,
        })
        .collect();
    let worker = MultiRepoMechanicalWorker::new(
        &workflow,
        forge as &dyn Forge,
        repositories.clone(),
        journal_bindings,
        LeasePolicy::new(config.lease_ttl),
    )
    .map_err(|error| RunError::Backend(error.to_string()))?;
    drive_async(args, &worker).await
}

async fn resolve_repositories<F: Forge + ?Sized>(
    forge: &F,
    paths: &[RepositoryPath],
) -> Result<RepositorySet, RunError> {
    let mut repositories = Vec::new();
    for path in paths {
        let repo = forge.get_repository_by_path(path).await?.ok_or_else(|| {
            RunError::RepositoryUnavailable {
                owner: path.owner.clone(),
                name: path.name.clone(),
            }
        })?;
        repositories.push(RepositoryTarget::new(
            repo.id,
            RepositoryPath::new(repo.owner, repo.name),
        ));
    }
    Ok(RepositorySet::new(repositories))
}

async fn ensure_workflow_labels<F: Forge + ?Sized>(
    forge: &F,
    repositories: &RepositorySet,
    compiled: &temper_workflow::CompiledWorkflow,
) -> Result<(), RunError> {
    for repository in repositories.repositories() {
        for label in compiled.labels().labels() {
            forge
                .upsert_label(
                    &repository.id,
                    UpsertLabel {
                        name: label.id.to_string(),
                        color: Some("#ededed".to_string()),
                        description: None,
                    },
                )
                .await?;
        }
    }
    Ok(())
}

fn log_repository_set(kind: &str, name: &str, repositories: &RepositorySet) {
    let repos = repository_display_paths(repositories).join(",");
    eprintln!(
        "temper-worker: resolved repositories worker_kind={kind} worker={name} repos={repos}"
    );
}

fn log_worker_capabilities(summary: WorkerCapabilitySummary) {
    eprintln!("{}", render_worker_capability_event(&summary));
}

fn repository_display_paths(repositories: &RepositorySet) -> Vec<String> {
    repositories
        .repositories()
        .iter()
        .map(RepositoryTarget::display_path)
        .collect()
}

const MAX_CONSECUTIVE_TICK_FAILURES: u32 = 50;

struct DriveTickReport {
    progress: temper_runner::Progress,
    scanned_repository_count: usize,
    scanned_repository_paths: Vec<String>,
}

impl DriveTickReport {
    fn from_multi_repo(report: temper_runner::MultiRepoTickReport) -> Result<Self, WorkerError> {
        let scanned_repository_count = report.scanned_repository_count();
        let scanned_repository_paths = report.scanned_repository_paths();
        let progress = report.into_worker_result()?;
        Ok(Self {
            progress,
            scanned_repository_count,
            scanned_repository_paths,
        })
    }
}

#[async_trait::async_trait]
trait DriveWorker: Sync {
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
        tick_id: &str,
    ) -> Result<DriveTickReport, WorkerError>;

    fn name(&self) -> &str;
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized> DriveWorker for MultiRepoRoleWorker<'_, F> {
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
        tick_id: &str,
    ) -> Result<DriveTickReport, WorkerError> {
        let report = match reason {
            TickReason::Wake => {
                let known = known_hints_for(self.repositories(), hints, "temper-worker");
                if known.is_empty() {
                    self.tick_hinted_with_observability_tick_id(now, &[], tick_id)
                        .await
                } else {
                    self.tick_matching_hints_with_observability_tick_id(now, &known, tick_id)
                        .await
                }
            }
            TickReason::Audit => {
                self.tick_audit_with_observability_tick_id(now, tick_id)
                    .await
            }
            TickReason::Initial | TickReason::Poll => {
                self.tick_hinted_with_observability_tick_id(now, &[], tick_id)
                    .await
            }
        };
        DriveTickReport::from_multi_repo(report)
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

#[async_trait::async_trait]
impl<F, J, P> DriveWorker for MultiRepoMechanicalWorker<'_, F, J, P>
where
    F: Forge + ?Sized,
    J: CommandJournal,
    P: temper_workflow::RecoveryPolicy + Clone + Send + Sync,
{
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
        _tick_id: &str,
    ) -> Result<DriveTickReport, WorkerError> {
        let report = match reason {
            TickReason::Wake => {
                let known = known_hints_for(self.repositories(), hints, "temper-worker");
                if !known.is_empty() {
                    eprintln!(
                        "temper-worker: mechanical wake uses broad scan despite configured hints to preserve cross-repo recovery"
                    );
                }
                self.tick_report(now).await
            }
            TickReason::Initial | TickReason::Poll | TickReason::Audit => {
                self.tick_report(now).await
            }
        };
        DriveTickReport::from_multi_repo(report)
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

fn known_hints_for(
    repositories: &RepositorySet,
    hints: &[ChangeHint],
    log_prefix: &str,
) -> Vec<ChangeHint> {
    let mut known = Vec::new();
    for hint in hints {
        if repositories
            .matching_hints(std::slice::from_ref(hint))
            .is_empty()
        {
            eprintln!(
                "{log_prefix}: wake hint for unconfigured repo {}/{}; treating wake as broad scan if no configured hints remain",
                hint.repo.owner, hint.repo.name
            );
        } else {
            known.push(hint.clone());
        }
    }
    known
}

fn production_tick_id(worker: &str, reason: TickReason, sequence: u64) -> String {
    format!("tick/{worker}/{}/{}", reason.as_str(), sequence)
}

async fn drive_async<W: DriveWorker>(args: &WorkerArgs, worker: &W) -> Result<RunReport, RunError> {
    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let interval = args
        .poll_interval
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_millis(1_000));
    let wake = build_wake_listener(args)?;
    let mut consecutive_failures = 0u32;
    let audit_interval = args
        .audit_interval
        .and_then(|duration| duration.to_std().ok());
    let mut next_poll_due = Instant::now() + interval;
    let mut next_audit_due = audit_interval.map(|duration| Instant::now() + duration);
    let mut next_tick_reason = TickReason::Initial;
    let mut pending_hints = Vec::new();
    let mut tick_sequence = 0u64;
    let mut report = RunReport {
        ticks: 0,
        workers: vec![WorkerRunReport {
            name: worker.name().to_string(),
            ticks: 0,
            actions: 0,
        }],
    };

    while !stop.should_stop() {
        let tick_reason = next_tick_reason;
        let tick_hints = if tick_reason == TickReason::Wake {
            std::mem::take(&mut pending_hints)
        } else {
            Vec::new()
        };
        tick_sequence = tick_sequence.saturating_add(1);
        let tick_id = production_tick_id(worker.name(), tick_reason, tick_sequence);
        match worker
            .tick_for_reason(chrono::Utc::now(), tick_reason, &tick_hints, &tick_id)
            .await
        {
            Ok(tick) => {
                consecutive_failures = 0;
                report.ticks = report.ticks.saturating_add(1);
                report.workers[0].ticks = report.workers[0].ticks.saturating_add(1);
                report.workers[0].actions = report.workers[0]
                    .actions
                    .saturating_add(u64::from(tick.progress.actions));
                record_completed_tick_deadline(
                    tick_reason,
                    interval,
                    audit_interval,
                    &mut next_poll_due,
                    &mut next_audit_due,
                );
                eprintln!(
                    "temper-worker: worker '{}' completed tick trigger={} actions={} tick_id={} scanned_repositories={} scanned_repository_paths={}",
                    worker.name(),
                    tick_reason.as_str(),
                    tick.progress.actions,
                    tick_id,
                    tick.scanned_repository_count,
                    render_repo_paths(&tick.scanned_repository_paths)
                );
            }
            Err(error) => {
                consecutive_failures += 1;
                record_completed_tick_deadline(
                    tick_reason,
                    interval,
                    audit_interval,
                    &mut next_poll_due,
                    &mut next_audit_due,
                );
                eprintln!(
                    "temper-worker: worker '{}' tick failed trigger={} tick_id={} \
                     ({consecutive_failures}/{MAX_CONSECUTIVE_TICK_FAILURES}), retrying: {error}",
                    worker.name(),
                    tick_reason.as_str(),
                    tick_id
                );
                if consecutive_failures >= MAX_CONSECUTIVE_TICK_FAILURES {
                    return Err(RunError::Drive(Box::new(error)));
                }
            }
        }
        let wait_interval = wait_interval_until_next_tick(next_poll_due, next_audit_due);
        match wait_for_wake_or_poll(|| stop.should_stop(), wait_interval, wake.as_ref())
            .await
            .map_err(|error| RunError::Backend(error.to_string()))?
        {
            WakeWaitOutcome::PollDeadline => {
                next_tick_reason = deadline_tick_reason(next_poll_due, next_audit_due)
            }
            WakeWaitOutcome::Stop => break,
            WakeWaitOutcome::Wake(hints) => {
                let wake_count = hints.len();
                pending_hints.extend(hints);
                eprintln!(
                    "temper-worker: worker '{}' consumed authenticated wake batch hints={wake_count}; ticking immediately",
                    worker.name()
                );
                next_tick_reason = TickReason::Wake;
            }
        }
    }

    Ok(report)
}

fn record_completed_tick_deadline(
    tick_reason: TickReason,
    poll_interval: StdDuration,
    audit_interval: Option<StdDuration>,
    next_poll_due: &mut Instant,
    next_audit_due: &mut Option<Instant>,
) {
    let now = Instant::now();
    *next_poll_due = now + poll_interval;
    if tick_reason == TickReason::Audit {
        *next_audit_due = audit_interval.map(|interval| now + interval);
    }
}

fn wait_interval_until_next_tick(
    next_poll_due: Instant,
    next_audit_due: Option<Instant>,
) -> StdDuration {
    let now = Instant::now();
    let next_due = next_audit_due
        .map(|audit_due| audit_due.min(next_poll_due))
        .unwrap_or(next_poll_due);
    next_due.saturating_duration_since(now)
}

fn deadline_tick_reason(_next_poll_due: Instant, next_audit_due: Option<Instant>) -> TickReason {
    let now = Instant::now();
    if next_audit_due.is_some_and(|due| now >= due) {
        TickReason::Audit
    } else {
        // The wait deadline can fire a little early if the stop-check cadence
        // races the timer; keep the normal poll path as the safe broad scan.
        TickReason::Poll
    }
}

fn render_repo_paths(paths: &[String]) -> String {
    if paths.is_empty() {
        "-".to_string()
    } else {
        paths.join(",")
    }
}

fn build_wake_listener(args: &WorkerArgs) -> Result<Option<WakeListener>, RunError> {
    let Some(socket) = args.wake_socket.clone() else {
        return Ok(None);
    };
    let config = WakeConfig::from_files(socket, args.wake_secret_file.clone())
        .map_err(|error| RunError::Backend(error.to_string()))?;
    WakeListener::bind(config)
        .map(Some)
        .map_err(|error| RunError::Backend(error.to_string()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TickReason {
    Initial,
    Poll,
    Wake,
    Audit,
}

impl TickReason {
    fn as_str(self) -> &'static str {
        match self {
            TickReason::Initial => "initial",
            TickReason::Poll => "poll",
            TickReason::Wake => "wake",
            TickReason::Audit => "audit",
        }
    }
}

struct StopSignal {
    stop_file: Option<std::path::PathBuf>,
    started: Instant,
    run_secs: Option<u64>,
}

impl StopSignal {
    fn new(stop_file: Option<std::path::PathBuf>, run_secs: Option<u64>) -> Self {
        Self {
            stop_file,
            started: Instant::now(),
            run_secs,
        }
    }

    fn should_stop(&self) -> bool {
        self.stop_file.as_ref().is_some_and(|path| path.exists())
            || self
                .run_secs
                .is_some_and(|seconds| self.started.elapsed().as_secs() >= seconds)
    }
}

#[cfg(all(test, unix))]
#[path = "worker_tests.rs"]
mod tests;
