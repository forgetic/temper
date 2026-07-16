//! Joined ownership of one agent process and its descendant containment.

use std::process::{Child, Command};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use temper_process_containment::{ContainmentKind, ProcessContainment, configure_command};
use temper_worker_io::CqReceiver;

use super::ChildOutcome;
use crate::agent_runner::AgentRunError;
use crate::out_of_process_runner::stderr::{
    DiagnosticIdentity, emit_reader_unavailable, stream as stream_stderr,
};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How a cancellation reached process quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationOutcome {
    Graceful,
    ForcedTermination,
    HardKill,
}

/// Final state of the run's descendant containment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DescendantCleanupStatus {
    Clean,
    Terminated,
    HardKilled,
    Failed(String),
}

/// One joined, reaped process-tree result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobQuiesced {
    pub cancellation: Option<CancellationOutcome>,
    pub descendants: DescendantCleanupStatus,
    pub containment: ContainmentKind,
}

pub(super) struct SupervisorResult {
    pub(super) outcome: Result<ChildOutcome, AgentRunError>,
    pub(super) quiesced: JobQuiesced,
}

enum SupervisorCommand {
    Cancel {
        first_party_connected: bool,
        graceful_grace: Duration,
        forced_grace: Duration,
    },
}

/// Explicit owner of the child, wait loop, containment and stderr reader.
///
/// The child itself lives on the named supervisor thread. Dropping this handle
/// requests cancellation and joins that thread; there is no detached blocking
/// `Child::wait` future.
pub struct ManagedAgentProcess {
    command: mpsc::Sender<SupervisorCommand>,
    wake: CqReceiver<()>,
    shared_result: Arc<Mutex<Option<SupervisorResult>>>,
    thread: Option<JoinHandle<()>>,
    completed: bool,
}

impl ManagedAgentProcess {
    pub fn spawn(
        mut command: Command,
        identity: DiagnosticIdentity,
        tracing_dispatch: tracing::Dispatch,
    ) -> Result<Self, AgentRunError> {
        configure_command(&mut command);
        let mut child = command.spawn().map_err(|error| {
            AgentRunError::transient(format!(
                "spawn agent command `{}`: {error}",
                command.get_program().to_string_lossy()
            ))
        })?;
        let containment = match ProcessContainment::attach(&child) {
            Ok(containment) => containment,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentRunError::transient(format!(
                    "attach agent process containment: {error}"
                )));
            }
        };
        let stderr = child.stderr.take();
        let stderr_thread = thread::Builder::new()
            .name(format!("agent-stderr-{}", child.id()))
            .spawn(move || {
                tracing::dispatcher::with_default(&tracing_dispatch, || match stderr {
                    Some(stderr) => stream_stderr(stderr, &identity),
                    None => {
                        emit_reader_unavailable(&identity);
                        String::new()
                    }
                })
            })
            .map_err(|error| {
                let _ = containment.hard_kill(&mut child);
                let _ = child.wait();
                AgentRunError::transient(format!("start agent stderr reader: {error}"))
            })?;

        let (command_tx, command_rx) = mpsc::channel();
        let (wake_tx, wake) = temper_worker_io::channel();
        let shared_result = Arc::new(Mutex::new(None));
        let thread_result = Arc::clone(&shared_result);
        let thread = thread::Builder::new()
            .name(format!("agent-supervisor-{}", child.id()))
            .spawn(move || {
                let result = supervise(child, containment, stderr_thread, command_rx);
                *thread_result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
                let _ = wake_tx.send(());
            })
            .map_err(|error| {
                AgentRunError::transient(format!("start agent process supervisor: {error}"))
            })?;

        Ok(Self {
            command: command_tx,
            wake,
            shared_result,
            thread: Some(thread),
            completed: false,
        })
    }

    pub fn poll_outcome(&mut self, cx: &mut Context<'_>) -> Poll<SupervisorResult> {
        if let Some(result) = self.take_result() {
            self.completed = true;
            return Poll::Ready(result);
        }
        let mut receive = Box::pin(self.wake.recv());
        let wake_poll = receive.as_mut().poll(cx);
        drop(receive);
        match wake_poll {
            Poll::Ready(_) => {
                let result = self.take_result().unwrap_or_else(|| SupervisorResult {
                    outcome: Err(AgentRunError::transient(
                        "agent supervisor ended without an outcome",
                    )),
                    quiesced: JobQuiesced {
                        cancellation: None,
                        descendants: DescendantCleanupStatus::Failed(
                            "supervisor outcome missing".to_string(),
                        ),
                        containment: fallback_kind(),
                    },
                });
                self.completed = true;
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn join_completed(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    pub fn cancel_and_join(
        &mut self,
        first_party_connected: bool,
        graceful_grace: Duration,
        forced_grace: Duration,
    ) -> SupervisorResult {
        if !self.completed {
            let _ = self.command.send(SupervisorCommand::Cancel {
                first_party_connected,
                graceful_grace,
                forced_grace,
            });
        }
        self.join_completed();
        self.completed = true;
        self.take_result().unwrap_or_else(|| SupervisorResult {
            outcome: Err(AgentRunError::transient(
                "agent supervisor joined without an outcome",
            )),
            quiesced: JobQuiesced {
                cancellation: Some(CancellationOutcome::HardKill),
                descendants: DescendantCleanupStatus::Failed(
                    "supervisor outcome missing".to_string(),
                ),
                containment: fallback_kind(),
            },
        })
    }

    fn take_result(&self) -> Option<SupervisorResult> {
        self.shared_result
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

impl Drop for ManagedAgentProcess {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.cancel_and_join(false, Duration::ZERO, Duration::ZERO);
        }
    }
}

fn supervise(
    mut child: Child,
    containment: ProcessContainment,
    stderr_thread: JoinHandle<String>,
    commands: mpsc::Receiver<SupervisorCommand>,
) -> SupervisorResult {
    let containment_kind = containment.kind();
    let mut cancellation = None;
    let mut descendants = DescendantCleanupStatus::Clean;
    let mut graceful_deadline = None;
    let mut forced_deadline = None;
    let mut forced_grace_after_graceful = Duration::ZERO;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = containment.hard_kill(&mut child);
                let _ = child.wait();
                descendants =
                    DescendantCleanupStatus::Failed(format!("poll agent process exit: {error}"));
                break Err(AgentRunError::transient(format!(
                    "poll agent process exit: {error}"
                )));
            }
        }

        if cancellation.is_none() {
            match commands.recv_timeout(PROCESS_POLL_INTERVAL) {
                Ok(SupervisorCommand::Cancel {
                    first_party_connected,
                    graceful_grace,
                    forced_grace,
                }) => {
                    if first_party_connected && !graceful_grace.is_zero() {
                        cancellation = Some(CancellationOutcome::Graceful);
                        graceful_deadline = Some(Instant::now() + graceful_grace);
                        forced_grace_after_graceful = forced_grace;
                    } else {
                        cancellation = Some(CancellationOutcome::ForcedTermination);
                        descendants = terminate(&containment, &mut child);
                        forced_deadline = Some(Instant::now() + forced_grace);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    cancellation = Some(CancellationOutcome::HardKill);
                    descendants = hard_kill(&containment, &mut child);
                }
            }
        } else {
            thread::sleep(PROCESS_POLL_INTERVAL);
        }

        let now = Instant::now();
        if graceful_deadline.is_some_and(|deadline| now >= deadline) {
            cancellation = Some(CancellationOutcome::ForcedTermination);
            descendants = terminate(&containment, &mut child);
            graceful_deadline = None;
            // Continue with the independently configured forced-termination
            // grace before hard-killing the group.
            forced_deadline = Some(now + forced_grace_after_graceful);
        }
        if forced_deadline.is_some_and(|deadline| now >= deadline) {
            cancellation = Some(CancellationOutcome::HardKill);
            descendants = hard_kill(&containment, &mut child);
            forced_deadline = None;
        }
    };

    // Even a cooperative direct-child exit may have left tool grandchildren in
    // the process group/job. Closing with a hard group kill makes the quiesced
    // boundary descendant-complete before any worker capacity can be released.
    let final_cleanup = hard_kill(&containment, &mut child);
    if !matches!(final_cleanup, DescendantCleanupStatus::Clean) {
        descendants = final_cleanup;
    }
    let stderr_tail = stderr_thread.join().unwrap_or_default();
    let outcome = status.map(|status| ChildOutcome {
        status_code: status.code(),
        stderr_tail,
    });
    SupervisorResult {
        outcome,
        quiesced: JobQuiesced {
            cancellation,
            descendants,
            containment: containment_kind,
        },
    }
}

fn terminate(containment: &ProcessContainment, child: &mut Child) -> DescendantCleanupStatus {
    match containment.terminate(child) {
        Ok(()) => DescendantCleanupStatus::Terminated,
        Err(error) => DescendantCleanupStatus::Failed(format!("terminate descendants: {error}")),
    }
}

fn hard_kill(containment: &ProcessContainment, child: &mut Child) -> DescendantCleanupStatus {
    match containment.hard_kill(child) {
        Ok(()) => DescendantCleanupStatus::HardKilled,
        Err(error) => DescendantCleanupStatus::Failed(format!("hard-kill descendants: {error}")),
    }
}

fn fallback_kind() -> ContainmentKind {
    #[cfg(unix)]
    {
        ContainmentKind::UnixProcessGroup
    }
    #[cfg(windows)]
    {
        ContainmentKind::WindowsJobObject
    }
    #[cfg(not(any(unix, windows)))]
    {
        ContainmentKind::DirectChildFallback
    }
}
