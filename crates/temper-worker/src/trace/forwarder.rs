use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use skein::cx::Cx;
use temper_protocol_worker::{
    WORKER_PROTOCOL_VERSION, WorkerActivityBatch, WorkerAuth, WorkerProtocolMessage,
};
use temper_worker_io::{OneshotReceiver, Spawner, oneshot, sleep_for, timer_now};

use crate::transport::Transport;
use crate::worker_shell::WorkerCancellation;

use super::{RecoveredForwardingRun, TraceCollector};

pub(crate) const FORWARD_BATCH_EVENT_LIMIT: usize = 50;
pub(crate) const FORWARD_BATCH_ENCODED_BYTE_LIMIT: usize = 64 * 1024;
const RECOVERY_BACKSTOP: Duration = Duration::from_secs(30);
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

enum ForwarderWork {
    FullRecovery,
    Notified(BTreeSet<String>),
}

#[derive(Default)]
struct ForwarderRetry {
    full_recovery: bool,
    run_ids: BTreeSet<String>,
    failures: Vec<ForwardingFailure>,
}

struct ForwardingFailure {
    run_id: Option<String>,
    error: String,
}

impl ForwarderRetry {
    fn full_recovery(error: String) -> Self {
        Self {
            full_recovery: true,
            failures: vec![ForwardingFailure {
                run_id: None,
                error,
            }],
            ..Self::default()
        }
    }

    fn run(run_id: String, error: String) -> Self {
        let mut run_ids = BTreeSet::new();
        run_ids.insert(run_id.clone());
        Self {
            run_ids,
            failures: vec![ForwardingFailure {
                run_id: Some(run_id),
                error,
            }],
            ..Self::default()
        }
    }

    fn merge(&mut self, mut other: Self) {
        self.full_recovery |= other.full_recovery;
        self.run_ids.append(&mut other.run_ids);
        self.failures.append(&mut other.failures);
    }

    fn is_empty(&self) -> bool {
        !self.full_recovery && self.run_ids.is_empty()
    }
}

enum ForwarderWake {
    Append,
    RecoveryBackstop,
}

pub(crate) fn spawn_activity_forwarder<T, S>(
    spawner: S,
    collector: TraceCollector,
    transport: Arc<T>,
    worker_id: String,
    auth: Option<WorkerAuth>,
    cancellation: WorkerCancellation,
) -> Option<OneshotReceiver<()>>
where
    T: Transport,
    S: Spawner,
{
    if !collector.forwarding_enabled() {
        return None;
    }
    let (joined_tx, joined) = oneshot();
    spawner.spawn_task_with_cx(move |cx| async move {
        let mut backoff = INITIAL_RETRY_BACKOFF;
        let mut next_full_recovery = timer_now(&cx);
        let mut work = ForwarderWork::FullRecovery;
        loop {
            let full_recovery = matches!(&work, ForwarderWork::FullRecovery);
            if full_recovery {
                // Everything published before this drain is covered by the
                // full pass. An append during recovery stays dirty and is
                // handled by a targeted follow-up.
                let _covered = collector.drain_dirty_runs();
            }
            let retry = cancellation
                .run(forward_work(
                    cx.clone(),
                    &collector,
                    Arc::clone(&transport),
                    &worker_id,
                    auth.clone(),
                    work,
                ))
                .await;
            let Some(retry) = retry else {
                break;
            };
            if full_recovery && !retry.full_recovery {
                next_full_recovery = timer_now(&cx) + RECOVERY_BACKSTOP;
            }

            if !retry.is_empty() {
                for failure in &retry.failures {
                    tracing::warn!(
                        target: "temper::worker",
                        service = "worker",
                        event = "agent.activity.forward_failed",
                        worker_id,
                        run_id = failure.run_id.as_deref().unwrap_or(""),
                        error = %failure.error,
                        backoff_ms = backoff.as_millis() as u64,
                        "worker could not forward durable agent activity; product work will continue"
                    );
                }
                if cancellation.run(sleep_for(backoff)).await.is_none() {
                    break;
                }
                backoff = backoff.saturating_mul(2).min(MAX_RETRY_BACKOFF);
                if retry.full_recovery {
                    work = ForwarderWork::FullRecovery;
                } else {
                    let mut run_ids = retry.run_ids;
                    run_ids.extend(
                        collector
                            .drain_dirty_runs()
                            .runs
                            .into_iter()
                            .map(|run| run.run_id),
                    );
                    work = ForwarderWork::Notified(run_ids);
                }
                continue;
            }

            backoff = INITIAL_RETRY_BACKOFF;
            let dirty = collector.drain_dirty_runs();
            let until_recovery = Duration::from_nanos(
                next_full_recovery.duration_since(timer_now(&cx)),
            );
            if until_recovery.is_zero() {
                work = ForwarderWork::FullRecovery;
                continue;
            }
            if !dirty.runs.is_empty() {
                work = ForwarderWork::Notified(
                    dirty
                        .runs
                        .into_iter()
                        .map(|run| run.run_id)
                        .collect(),
                );
                continue;
            }

            let wake = cancellation
                .run(wait_for_forwarder_wake(
                    &collector,
                    dirty.generation,
                    until_recovery,
                ))
                .await;
            let Some(wake) = wake else {
                break;
            };
            work = match wake {
                ForwarderWake::Append => ForwarderWork::Notified(
                    collector
                        .drain_dirty_runs()
                        .runs
                        .into_iter()
                        .map(|run| run.run_id)
                        .collect(),
                ),
                ForwarderWake::RecoveryBackstop => ForwarderWork::FullRecovery,
            };
        }
        joined_tx.send(());
    });
    Some(joined)
}

async fn wait_for_forwarder_wake(
    collector: &TraceCollector,
    after_generation: u64,
    recovery_delay: Duration,
) -> ForwarderWake {
    let mut appended = std::pin::pin!(collector.wait_for_append(after_generation));
    let mut recovery = std::pin::pin!(sleep_for(recovery_delay));
    std::future::poll_fn(|cx| {
        if appended.as_mut().poll(cx).is_ready() {
            return Poll::Ready(ForwarderWake::Append);
        }
        if recovery.as_mut().poll(cx).is_ready() {
            return Poll::Ready(ForwarderWake::RecoveryBackstop);
        }
        Poll::Pending
    })
    .await
}

async fn forward_work<T: Transport>(
    cx: Cx,
    collector: &TraceCollector,
    transport: Arc<T>,
    worker_id: &str,
    auth: Option<WorkerAuth>,
    work: ForwarderWork,
) -> ForwarderRetry {
    let runs = match work {
        ForwarderWork::FullRecovery => match collector.recover_forwardable() {
            Ok(runs) => runs,
            Err(error) => {
                return ForwarderRetry::full_recovery(format!("recover activity spools: {error}"));
            }
        },
        ForwarderWork::Notified(run_ids) => run_ids
            .into_iter()
            .filter_map(|run_id| collector.recover_notified_run(&run_id))
            .collect(),
    };

    let mut retry = ForwarderRetry::default();
    for run in runs {
        let run_id = run.manifest.run_id.clone();
        if let Err(error) = forward_run(
            cx.clone(),
            collector,
            Arc::clone(&transport),
            worker_id,
            auth.clone(),
            run,
        )
        .await
        {
            retry.merge(ForwarderRetry::run(run_id, error));
        }
    }
    retry
}

/// Scans all restart-readable spools and forwards every pending contiguous
/// batch. This test helper exercises the same recovery and per-run forwarding
/// path as startup recovery.
#[cfg(test)]
pub(crate) async fn forward_pending<T: Transport>(
    cx: Cx,
    collector: &TraceCollector,
    transport: Arc<T>,
    worker_id: &str,
    auth: Option<WorkerAuth>,
) -> Result<(), String> {
    let runs = collector
        .recover_forwardable()
        .map_err(|error| format!("recover activity spools: {error}"))?;
    for run in runs {
        forward_run(
            cx.clone(),
            collector,
            Arc::clone(&transport),
            worker_id,
            auth.clone(),
            run,
        )
        .await?;
    }
    Ok(())
}

async fn forward_run<T: Transport>(
    cx: Cx,
    collector: &TraceCollector,
    transport: Arc<T>,
    worker_id: &str,
    auth: Option<WorkerAuth>,
    mut run: RecoveredForwardingRun,
) -> Result<(), String> {
    while let Some(forwarding_batch) =
        run.pending_batch_bounded(FORWARD_BATCH_EVENT_LIMIT, FORWARD_BATCH_ENCODED_BYTE_LIMIT)
    {
        let (batch, boundaries) = forwarding_batch.into_parts();
        let last_sent = batch
            .events
            .last()
            .map(|event| event.seq)
            .ok_or_else(|| "forwarder built an empty activity batch".to_string())?;
        let message = WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: worker_id.to_string(),
            // Job IDs are the current durable assignment identity in the
            // worker protocol. Keeping this explicit leaves room for a
            // distinct attempt ID in a future protocol version.
            assignment_id: run.manifest.assignment.job_id.clone(),
            capture_policy: run.manifest.policy.clone(),
            batch,
        });
        let reply = transport
            .send(cx.clone(), message, auth.clone())
            .await?
            .ok_or_else(|| "daemon returned an empty activity acknowledgement".to_string())?;
        let WorkerProtocolMessage::ActivityAck(reply) = reply else {
            return Err("daemon returned a non-activity acknowledgement".to_string());
        };
        if reply.protocol_version != WORKER_PROTOCOL_VERSION || reply.worker_id != worker_id {
            return Err("activity acknowledgement worker identity mismatch".to_string());
        }
        reply
            .acknowledgement
            .validate()
            .map_err(|error| format!("malformed activity acknowledgement: {error}"))?;
        if reply.acknowledgement.run_id != run.manifest.run_id
            || reply.acknowledgement.highest_contiguous_seq <= run.acknowledged_seq
            || reply.acknowledgement.highest_contiguous_seq > last_sent
        {
            return Err("activity acknowledgement cursor is outside the sent batch".to_string());
        }
        let acknowledged = reply.acknowledgement.highest_contiguous_seq;
        let boundary = boundaries
            .iter()
            .copied()
            .find(|boundary| boundary.sequence == acknowledged)
            .ok_or_else(|| "activity acknowledgement has no durable event boundary".to_string())?;
        collector
            .acknowledge_forwarded(&run.manifest.run_id, boundary)
            .map_err(|error| format!("persist activity acknowledgement: {error}"))?;
        run.acknowledged_seq = acknowledged;
    }
    Ok(())
}

#[cfg(test)]
#[path = "forwarder_tests.rs"]
mod tests;
