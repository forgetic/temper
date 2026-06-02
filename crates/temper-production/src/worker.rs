//! Runtime construction for `temper-worker`.

use std::error::Error;
use std::fmt;
use std::time::{Duration as StdDuration, Instant};

use crate::wake::{WakeConfig, WakeError, WakeListener};
use crate::worker_args::{ForgejoArgs, WorkerArgs, WorkerKind};
use crate::worker_external_tools::configure_external_tool_executors;
use crate::worker_role_agent::build_role_agent;
use crate::{runner_config, workflow};
use temper_forge::{ChangeHint, Forge, ForgeError, RepositoryPath, UpsertLabel};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::{
    MultiRepoMechanicalWorker, MultiRepoRoleWorker, RepositoryJournal, RepositorySet,
    RepositoryTarget, RunReport, Worker, WorkerError, WorkerRunReport,
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
    let repos = repositories
        .repositories()
        .iter()
        .map(RepositoryTarget::display_path)
        .collect::<Vec<_>>()
        .join(",");
    eprintln!(
        "temper-worker: resolved repositories worker_kind={kind} worker={name} repos={repos}"
    );
}

const MAX_CONSECUTIVE_TICK_FAILURES: u32 = 50;

#[async_trait::async_trait]
trait DriveWorker: Sync {
    async fn tick_for_wake(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        hints: &[ChangeHint],
    ) -> Result<temper_runner::Progress, WorkerError>;

    fn name(&self) -> &str;
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized> DriveWorker for MultiRepoRoleWorker<'_, F> {
    async fn tick_for_wake(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        hints: &[ChangeHint],
    ) -> Result<temper_runner::Progress, WorkerError> {
        let known = known_hints_for(self.repositories(), hints);
        self.tick_hinted(now, &known).await.into_worker_result()
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
    async fn tick_for_wake(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        _hints: &[ChangeHint],
    ) -> Result<temper_runner::Progress, WorkerError> {
        Worker::tick(self, now).await
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

fn known_hints_for(repositories: &RepositorySet, hints: &[ChangeHint]) -> Vec<ChangeHint> {
    let mut known = Vec::new();
    for hint in hints {
        if repositories
            .matching_hints(std::slice::from_ref(hint))
            .is_empty()
        {
            eprintln!(
                "temper-worker: wake hint for unconfigured repo {}/{}; treating wake as broad scan",
                hint.repo.owner, hint.repo.name
            );
        } else {
            known.push(hint.clone());
        }
    }
    known
}

async fn drive_async<W: DriveWorker>(args: &WorkerArgs, worker: &W) -> Result<RunReport, RunError> {
    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let interval = args
        .poll_interval
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_millis(1_000));
    let wake = build_wake_listener(args)?;
    let mut consecutive_failures = 0u32;
    let mut next_tick_reason = TickReason::Initial;
    let mut pending_hints = Vec::new();
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
        match worker.tick_for_wake(chrono::Utc::now(), &tick_hints).await {
            Ok(progress) => {
                consecutive_failures = 0;
                report.ticks = report.ticks.saturating_add(1);
                report.workers[0].ticks = report.workers[0].ticks.saturating_add(1);
                report.workers[0].actions = report.workers[0]
                    .actions
                    .saturating_add(u64::from(progress.actions));
                if tick_reason != TickReason::Poll {
                    eprintln!(
                        "temper-worker: worker '{}' completed tick trigger={} actions={}",
                        worker.name(),
                        tick_reason.as_str(),
                        progress.actions
                    );
                }
            }
            Err(error) => {
                consecutive_failures += 1;
                eprintln!(
                    "temper-worker: worker '{}' tick failed trigger={} \
                     ({consecutive_failures}/{MAX_CONSECUTIVE_TICK_FAILURES}), retrying: {error}",
                    worker.name(),
                    tick_reason.as_str()
                );
                if consecutive_failures >= MAX_CONSECUTIVE_TICK_FAILURES {
                    return Err(RunError::Drive(Box::new(error)));
                }
            }
        }
        match wait_for_next_tick(&stop, interval, wake.as_ref()).await? {
            WaitOutcome::PollDeadline => next_tick_reason = TickReason::Poll,
            WaitOutcome::Stop => break,
            WaitOutcome::Wake(hints) => {
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
}

impl TickReason {
    fn as_str(self) -> &'static str {
        match self {
            TickReason::Initial => "initial",
            TickReason::Poll => "poll",
            TickReason::Wake => "wake",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WaitOutcome {
    PollDeadline,
    Stop,
    Wake(Vec<ChangeHint>),
}

const MAX_WAKE_DRAIN: usize = 1024;
const WAKE_DEBOUNCE: StdDuration = StdDuration::from_millis(500);

fn drain_wake_batch(
    listener: &WakeListener,
    first: Option<ChangeHint>,
) -> Result<Vec<ChangeHint>, WakeError> {
    let mut hints = first.into_iter().collect::<Vec<_>>();
    let mut drained = 0usize;
    loop {
        if drained >= MAX_WAKE_DRAIN {
            eprintln!(
                "temper-worker: wake drain hit cap ({MAX_WAKE_DRAIN}); remaining queued wakes will form a later batch"
            );
            break;
        }
        match listener.try_recv() {
            Ok(Some(Some(hint))) => {
                hints.push(hint);
                drained += 1;
            }
            Ok(Some(None)) => drained += 1,
            Ok(None) => break,
            Err(WakeError::Unauthorized) => {
                eprintln!("temper-worker: ignored unauthorized wake message");
                drained += 1;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(hints)
}

async fn wait_for_next_tick(
    stop: &StopSignal,
    interval: StdDuration,
    wake: Option<&WakeListener>,
) -> Result<WaitOutcome, RunError> {
    let deadline = tokio::time::sleep(interval);
    tokio::pin!(deadline);
    loop {
        if stop.should_stop() {
            return Ok(WaitOutcome::Stop);
        }
        let stop_check = tokio::time::sleep(StdDuration::from_millis(250));
        tokio::pin!(stop_check);
        match wake {
            Some(listener) => {
                tokio::select! {
                    _ = &mut deadline => return Ok(WaitOutcome::PollDeadline),
                    _ = &mut stop_check => {},
                    received = listener.recv() => match received {
                        Ok(hint) => {
                            tokio::time::sleep(WAKE_DEBOUNCE).await;
                            return drain_wake_batch(listener, hint)
                                .map(WaitOutcome::Wake)
                                .map_err(|error| RunError::Backend(error.to_string()));
                        }
                        Err(WakeError::Unauthorized) => {
                            eprintln!("temper-worker: ignored unauthorized wake message");
                        }
                        Err(error) => return Err(RunError::Backend(error.to_string())),
                    },
                }
            }
            None => {
                tokio::select! {
                    _ = &mut deadline => return Ok(WaitOutcome::PollDeadline),
                    _ = &mut stop_check => {},
                }
            }
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
