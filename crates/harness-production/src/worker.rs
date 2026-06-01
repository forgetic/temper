//! Runtime construction for `harness-worker`.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use harness_forge::{Forge, ForgeError, RepositoryId, RepositoryPath};
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};
use harness_runner::{MechanicalWorker, RoleWorker, RunReport, WorkerRunReport};
use harness_workflow::{InMemoryJournal, LeasePolicy, RoleId};

use crate::forgejo_prep::ForgejoLlmPrep;
use crate::wake::{WakeConfig, WakeError, WakeListener};
use crate::worker_args::{AuthKind, ForgejoArgs, WorkerArgs, WorkerKind};
use crate::{runner_config, workflow};

#[derive(Debug)]
pub enum RunError {
    Forge(ForgeError),
    RepositoryMissing { owner: String, name: String },
    UnknownRole { role: String },
    Drive(Box<dyn Error + Send + Sync + 'static>),
    Backend(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Forge(error) => write!(formatter, "forge operation failed: {error}"),
            RunError::RepositoryMissing { owner, name } => {
                write!(formatter, "repository {owner}/{name} not found")
            }
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
    let config = runner_config();
    let role_id = RoleId::new(role);
    let provider = provider_for(args)?;
    let prep = Arc::new(ForgejoLlmPrep::new(
        args.forgejo.base_url.clone(),
        args.forgejo.token.clone(),
        args.owner.clone(),
        args.name.clone(),
    )) as Arc<dyn harness_agents::EngineerPrep<dyn Forge>>;
    let registry = harness_agents::real_registry_with(
        provider,
        harness_agents::RealRegistryConfig {
            engineer_prep: prep,
            ..Default::default()
        },
    );
    let agent = registry
        .get(&role_id)
        .ok_or_else(|| RunError::UnknownRole { role: role.into() })?
        .clone();
    let repo = resolve_repository(forge, &args.owner, &args.name).await?;
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        forge as &dyn Forge,
        &repo,
        role_id.clone(),
        agent,
        config.execution_context(&role_id),
    );
    drive_async(args, &worker).await
}

async fn run_mechanical(args: &WorkerArgs, forge: &ForgejoForge) -> Result<RunReport, RunError> {
    let workflow = workflow();
    let config = runner_config();
    let repo = resolve_repository(forge, &args.owner, &args.name).await?;
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(
        &workflow,
        forge as &dyn Forge,
        &repo,
        &journal,
        LeasePolicy::new(config.lease_ttl),
    );
    drive_async(args, &worker).await
}

fn provider_for(args: &WorkerArgs) -> Result<harness_agents::ProviderConfig, RunError> {
    let choice = match args.auth {
        AuthKind::DeepSeek => harness_agents::AuthChoice::DeepSeek,
        AuthKind::ChatGptOAuth => harness_agents::AuthChoice::ChatGptOAuth,
        AuthKind::AnthropicOAuth => harness_agents::AuthChoice::AnthropicOAuth,
    };
    harness_agents::ProviderConfig::from_auth(
        choice,
        args.codex_model.clone(),
        args.auth_file.clone(),
    )
    .map_err(|error| RunError::Backend(error.to_string()))
}

async fn resolve_repository<F: Forge + ?Sized>(
    forge: &F,
    owner: &str,
    name: &str,
) -> Result<RepositoryId, RunError> {
    let path = RepositoryPath::new(owner, name);
    forge
        .get_repository_by_path(&path)
        .await?
        .map(|repo| repo.id)
        .ok_or_else(|| RunError::RepositoryMissing {
            owner: owner.into(),
            name: name.into(),
        })
}

const MAX_CONSECUTIVE_TICK_FAILURES: u32 = 50;

async fn drive_async<W: harness_runner::Worker>(
    args: &WorkerArgs,
    worker: &W,
) -> Result<RunReport, RunError> {
    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let interval = args
        .poll_interval
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_millis(1_000));
    let wake = build_wake_listener(args)?;
    let mut consecutive_failures = 0u32;
    let mut report = RunReport {
        ticks: 0,
        workers: vec![WorkerRunReport {
            name: worker.name().to_string(),
            ticks: 0,
            actions: 0,
        }],
    };

    while !stop.should_stop() {
        match worker.tick(chrono::Utc::now()).await {
            Ok(progress) => {
                consecutive_failures = 0;
                report.ticks = report.ticks.saturating_add(1);
                report.workers[0].ticks = report.workers[0].ticks.saturating_add(1);
                report.workers[0].actions = report.workers[0]
                    .actions
                    .saturating_add(u64::from(progress.actions));
            }
            Err(error) => {
                consecutive_failures += 1;
                eprintln!(
                    "harness-worker: worker '{}' tick failed \
                     ({consecutive_failures}/{MAX_CONSECUTIVE_TICK_FAILURES}), retrying: {error}",
                    worker.name()
                );
                if consecutive_failures >= MAX_CONSECUTIVE_TICK_FAILURES {
                    return Err(RunError::Drive(Box::new(error)));
                }
            }
        }
        match wait_for_next_tick(&stop, interval, wake.as_ref()).await? {
            WaitOutcome::PollDeadline => {}
            WaitOutcome::Stop => break,
            WaitOutcome::Wake => eprintln!(
                "harness-worker: worker '{}' consumed authenticated wake; ticking immediately",
                worker.name()
            ),
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
enum WaitOutcome {
    PollDeadline,
    Stop,
    Wake,
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
                        Ok(()) => return Ok(WaitOutcome::Wake),
                        Err(WakeError::Unauthorized) => {
                            eprintln!("harness-worker: ignored unauthorized wake message");
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
mod tests {
    use super::*;
    use crate::wake::send_wake;
    use std::path::PathBuf;
    use std::thread;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "harness-production-worker-{name}-{}-{}.sock",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("timestamp has nanos")
        ));
        path
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime builds")
    }

    #[test]
    fn authenticated_wake_interrupts_long_wait() {
        let socket = temp_path("authenticated");
        let runtime = runtime();
        let _guard = runtime.enter();
        let listener = WakeListener::bind(WakeConfig {
            socket: socket.clone(),
            secret: Some("wake-secret".into()),
        })
        .expect("listener binds");
        let stop = StopSignal::new(None, None);
        let sender = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(50));
            send_wake(&socket, Some("wake-secret")).expect("wake sends");
        });
        let start = Instant::now();

        let outcome = runtime
            .block_on(wait_for_next_tick(
                &stop,
                StdDuration::from_secs(60),
                Some(&listener),
            ))
            .expect("wait succeeds");
        sender.join().expect("sender joins");

        assert_eq!(outcome, WaitOutcome::Wake);
        assert!(
            start.elapsed() < StdDuration::from_secs(1),
            "authenticated wake should beat the long poll interval"
        );
    }

    #[test]
    fn unauthorized_wake_is_ignored_until_stop_or_poll() {
        let socket = temp_path("unauthorized");
        let stop_file = temp_path("stop").with_extension("stop");
        let runtime = runtime();
        let _guard = runtime.enter();
        let listener = WakeListener::bind(WakeConfig {
            socket: socket.clone(),
            secret: Some("wake-secret".into()),
        })
        .expect("listener binds");
        let stop = StopSignal::new(Some(stop_file.clone()), None);
        let stop_file_for_thread = stop_file.clone();
        let sender = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(50));
            send_wake(&socket, Some("wrong-secret")).expect("unauthorized wake sends");
            thread::sleep(StdDuration::from_millis(150));
            std::fs::write(&stop_file_for_thread, b"stop").expect("stop file writes");
        });
        let start = Instant::now();

        let outcome = runtime
            .block_on(wait_for_next_tick(
                &stop,
                StdDuration::from_secs(60),
                Some(&listener),
            ))
            .expect("wait succeeds");
        sender.join().expect("sender joins");

        assert_eq!(outcome, WaitOutcome::Stop);
        assert!(
            start.elapsed() >= StdDuration::from_millis(150),
            "unauthorized wake must not end the wait"
        );
        assert!(
            start.elapsed() < StdDuration::from_secs(2),
            "stop backstop should end the test before the poll interval"
        );
        let _ = std::fs::remove_file(stop_file);
    }
}
