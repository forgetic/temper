//! Forgejo worker drive loop with wake-hint narrowing.

mod targeting;
#[cfg(all(test, unix))]
mod tests;

use std::time::{Duration as StdDuration, Instant};

use temper_forge_model::{ChangeHint, Forge};
use temper_runner::{
    IdlePollBackoff, MechanicalWorker, MultiRepoMechanicalWorker, MultiRepoRoleWorker, Progress,
    RoleWorker, RunReport, Worker, WorkerError, WorkerRunReport,
};
use temper_wake::{WakeConfig, WakeListener, WakeWaitOutcome, wait_for_wake_or_poll};
use temper_workflow::{CommandJournal, RecoveryPolicy};

use targeting::{known_hints_for, targeted_multi_repo_hints, targeted_single_repo_hints};

use crate::worker_bin::args::WorkerArgs;
use crate::worker_bin::run::{RunError, StopSignal};

/// How many consecutive failing ticks abort a Forgejo worker. A real server
/// under concurrent multi-process load returns transient `5xx`/conflict errors;
/// a level-triggered poll worker must survive those and retry. A long run of
/// failures means a genuine misconfiguration, so abort loudly.
const MAX_CONSECUTIVE_TICK_FAILURES: u32 = 50;

pub(super) struct ForgejoTickReport {
    progress: Progress,
    scanned_repository_count: usize,
    scanned_repository_paths: Vec<String>,
}

impl ForgejoTickReport {
    fn single(progress: Progress) -> Self {
        Self {
            progress,
            scanned_repository_count: 1,
            scanned_repository_paths: Vec::new(),
        }
    }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TickReason {
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

    fn is_normal(self) -> bool {
        matches!(self, TickReason::Initial | TickReason::Poll)
    }
}

#[async_trait::async_trait]
pub(super) trait ForgejoDriveWorker: Sync {
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
    ) -> Result<ForgejoTickReport, WorkerError>;

    fn name(&self) -> &str;
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized> ForgejoDriveWorker for RoleWorker<'_, F> {
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        _hints: &[ChangeHint],
    ) -> Result<ForgejoTickReport, WorkerError> {
        let progress = match reason {
            TickReason::Wake => self.tick_wake(now).await?,
            TickReason::Audit => self.tick_audit(now).await?,
            TickReason::Initial | TickReason::Poll => Worker::tick(self, now).await?,
        };
        Ok(ForgejoTickReport::single(progress))
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

#[async_trait::async_trait]
impl<F: Forge + ?Sized> ForgejoDriveWorker for MultiRepoRoleWorker<'_, F> {
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
    ) -> Result<ForgejoTickReport, WorkerError> {
        let report = match reason {
            TickReason::Wake => {
                let known = known_hints_for(self.repositories(), hints);
                if known.is_empty() {
                    self.tick_hinted_wake(now, &[]).await
                } else {
                    self.tick_matching_hints_wake(now, &known).await
                }
            }
            TickReason::Audit => self.tick_audit_report(now).await,
            TickReason::Initial | TickReason::Poll => self.tick_hinted(now, &[]).await,
        };
        ForgejoTickReport::from_multi_repo(report)
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

#[async_trait::async_trait]
impl<F, J, P> ForgejoDriveWorker for MechanicalWorker<'_, F, J, P>
where
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Send + Sync,
{
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
    ) -> Result<ForgejoTickReport, WorkerError> {
        let progress = match reason {
            TickReason::Audit => self.tick_deep_audit(now).await?,
            TickReason::Initial | TickReason::Poll => Worker::tick(self, now).await?,
            TickReason::Wake => match targeted_single_repo_hints(self, hints).await? {
                Some(targets) => {
                    let mut progress = Progress::unchanged();
                    for (item, kind) in targets {
                        let item_progress = self.tick_artifact(now, item, kind).await?;
                        progress.changed |= item_progress.changed;
                        progress.actions = progress.actions.saturating_add(item_progress.actions);
                    }
                    progress
                }
                None => Worker::tick(self, now).await?,
            },
        };
        Ok(ForgejoTickReport::single(progress))
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

#[async_trait::async_trait]
impl<F, J, P> ForgejoDriveWorker for MultiRepoMechanicalWorker<'_, F, J, P>
where
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Clone + Send + Sync,
{
    async fn tick_for_reason(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        reason: TickReason,
        hints: &[ChangeHint],
    ) -> Result<ForgejoTickReport, WorkerError> {
        let report = match reason {
            TickReason::Wake => match targeted_multi_repo_hints(self.repositories(), hints) {
                Some(targets) => self.tick_targeted(now, &targets).await,
                None => self.tick_report(now).await,
            },
            TickReason::Audit => self.tick_deep_audit_report(now).await,
            TickReason::Initial | TickReason::Poll => self.tick_report(now).await,
        };
        ForgejoTickReport::from_multi_repo(report)
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

/// Drives `worker` with a resilient wall-clock poll loop on the current Tokio
/// runtime. Authenticated wake hints interrupt the wait; known repo hints narrow
/// the immediate multi-repo role tick, while polls and audits scan configured repos.
pub(super) async fn drive_async<W: ForgejoDriveWorker>(
    cx: &temper_engine_io::Cx,
    args: &WorkerArgs,
    worker: &W,
) -> Result<RunReport, RunError> {
    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let interval = args
        .poll_interval
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_millis(50));
    let mut wake = match args.wake_socket.clone() {
        Some(socket) => Some(
            WakeListener::bind(
                WakeConfig::from_files(socket, args.wake_secret_file.clone())
                    .map_err(|error| RunError::Backend(error.to_string()))?,
            )
            .map_err(|error| RunError::Backend(error.to_string()))?,
        ),
        None => None,
    };
    let audit_interval = args
        .audit_interval
        .and_then(|duration| duration.to_std().ok());
    let mut idle_backoff = matches!(&args.kind, crate::worker_bin::args::WorkerKind::Mechanical)
        .then(|| {
            IdlePollBackoff::new(
                interval,
                args.idle_poll_max_interval.to_std().unwrap_or(interval),
            )
        });
    let mut next_poll_due = Instant::now() + interval;
    let mut next_audit_due = audit_interval.map(|duration| Instant::now() + duration);
    let mut next_tick_reason = TickReason::Initial;
    let mut pending_hints = Vec::new();
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
        let tick_reason = next_tick_reason;
        let tick_hints = if tick_reason == TickReason::Wake {
            std::mem::take(&mut pending_hints)
        } else {
            Vec::new()
        };
        match worker
            .tick_for_reason(chrono::Utc::now(), tick_reason, &tick_hints)
            .await
        {
            Ok(tick) => {
                consecutive_failures = 0;
                report.ticks = report.ticks.saturating_add(1);
                report.workers[0].ticks = report.workers[0].ticks.saturating_add(1);
                report.workers[0].actions = report.workers[0]
                    .actions
                    .saturating_add(u64::from(tick.progress.actions));
                let next_poll_delay = record_completed_tick_deadline(
                    tick_reason,
                    interval,
                    audit_interval,
                    idle_backoff.as_mut(),
                    Some(tick.progress),
                    &mut next_poll_due,
                    &mut next_audit_due,
                );
                eprintln!(
                    "temper-testing-worker: worker '{}' completed tick trigger={} actions={} scanned_repositories={} scanned_repository_paths={} next_poll_ms={} idle_no_action_ticks={}",
                    worker.name(),
                    tick_reason.as_str(),
                    tick.progress.actions,
                    tick.scanned_repository_count,
                    render_repo_paths(&tick.scanned_repository_paths),
                    next_poll_delay.as_millis(),
                    idle_backoff
                        .as_ref()
                        .map_or(0, IdlePollBackoff::consecutive_idle_ticks)
                );
            }
            Err(error) => {
                consecutive_failures += 1;
                record_completed_tick_deadline(
                    tick_reason,
                    interval,
                    audit_interval,
                    idle_backoff.as_mut(),
                    None,
                    &mut next_poll_due,
                    &mut next_audit_due,
                );
                eprintln!(
                    "temper-testing-worker: worker '{}' tick failed trigger={} \
                     ({consecutive_failures}/{MAX_CONSECUTIVE_TICK_FAILURES}), retrying: {error}",
                    worker.name(),
                    tick_reason.as_str()
                );
                if consecutive_failures >= MAX_CONSECUTIVE_TICK_FAILURES {
                    return Err(RunError::Drive(Box::new(error)));
                }
            }
        }
        if stop.should_stop() {
            break;
        }
        let wait_interval = wait_interval_until_next_tick(next_poll_due, next_audit_due);
        match wait_for_wake_or_poll(
            cx,
            || stop.should_stop(),
            wait_interval,
            args.wake_debounce,
            wake.as_mut(),
        )
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
                    "temper-testing-worker: worker '{}' consumed authenticated wake batch hints={wake_count}; ticking immediately",
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
    idle_backoff: Option<&mut IdlePollBackoff>,
    progress: Option<Progress>,
    next_poll_due: &mut Instant,
    next_audit_due: &mut Option<Instant>,
) -> StdDuration {
    let now = Instant::now();
    let poll_delay = match (idle_backoff, progress) {
        (Some(backoff), Some(progress)) if tick_reason.is_normal() => {
            backoff.record_normal_tick(progress)
        }
        (Some(backoff), _) => backoff.reset(),
        (None, _) => poll_interval,
    };
    *next_poll_due = now + poll_delay;
    if tick_reason == TickReason::Audit {
        *next_audit_due = audit_interval.map(|interval| now + interval);
    }
    poll_delay
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
