//! Joined ownership of one agent process and its descendant containment.

use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use temper_process_containment::{
    CleanupTrigger, ContainedProcess, PreparedContainment, RecursiveEmptyProof,
};
use temper_worker_io::CqReceiver;

use super::ChildOutcome;
use crate::agent_runner::AgentRunError;
use crate::executor::{CancellationOutcome, JobCleanup, ResourceJoinReport, ResourceJoinStatus};
use crate::out_of_process_runner::stderr::{
    DiagnosticIdentity, emit_reader_unavailable, stream as stream_stderr,
};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// One joined, reaped process-tree result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobQuiesced {
    pub cleanup: JobCleanup,
}

#[derive(Debug)]
pub(super) struct ManagedAgentSpawnError {
    pub(super) error: AgentRunError,
    pub(super) cleanup: JobCleanup,
}

pub(super) struct SupervisorResult {
    pub(super) outcome: Result<ChildOutcome, AgentRunError>,
    pub(super) quiesced: JobQuiesced,
}

enum SupervisorCommand {
    Cancel,
    ForceTerminate,
    HardKill,
}

/// Explicit owner of the child, wait loop, containment and stderr reader.
///
/// The child itself lives on the named supervisor thread. Dropping this handle
/// requests cleanup and joins that thread; there is no detached `Child::wait`
/// future.
pub struct ManagedAgentProcess {
    command: mpsc::Sender<SupervisorCommand>,
    wake: CqReceiver<()>,
    shared_result: Arc<Mutex<Option<SupervisorResult>>>,
    thread: Option<JoinHandle<()>>,
    completed: bool,
}

impl ManagedAgentProcess {
    pub fn spawn(
        prepared: PreparedContainment,
        command: temper_process_containment::ContainmentCommand,
        identity: DiagnosticIdentity,
        tracing_dispatch: tracing::Dispatch,
    ) -> Result<Self, ManagedAgentSpawnError> {
        let process = prepared
            .spawn(command)
            .map_err(|error| ManagedAgentSpawnError {
                error: AgentRunError::transient(format!("spawn contained agent command: {error}")),
                cleanup: JobCleanup::no_process(None),
            })?;
        let pid = process.id();
        let stderr = match process.take_stderr() {
            Ok(stderr) => stderr,
            Err(error) => {
                let cleanup_report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(ManagedAgentSpawnError {
                    error: AgentRunError::transient(format!("take agent stderr: {error}")),
                    cleanup: setup_cleanup(cleanup_report, ResourceJoinStatus::NotApplicable),
                });
            }
        };
        let stderr_thread = match thread::Builder::new()
            .name(format!("agent-stderr-{pid}"))
            .spawn(move || {
                tracing::dispatcher::with_default(&tracing_dispatch, || match stderr {
                    Some(stderr) => stream_stderr(stderr, &identity),
                    None => {
                        emit_reader_unavailable(&identity);
                        String::new()
                    }
                })
            }) {
            Ok(thread) => thread,
            Err(error) => {
                let cleanup_report = process.cleanup(CleanupTrigger::Shutdown);
                return Err(ManagedAgentSpawnError {
                    error: AgentRunError::transient(format!("start agent stderr reader: {error}")),
                    cleanup: setup_cleanup(cleanup_report, ResourceJoinStatus::NotApplicable),
                });
            }
        };

        let (command_tx, command_rx) = mpsc::channel();
        let (wake_tx, wake) = temper_worker_io::channel();
        let shared_result = Arc::new(Mutex::new(None));
        let thread_result = Arc::clone(&shared_result);

        // Keep setup resources recoverable if thread creation itself fails.
        // Moving them directly into the closure would drop the JoinHandle
        // without joining its reader on that path.
        let startup = Arc::new(Mutex::new(Some((process, stderr_thread))));
        let thread_startup = Arc::clone(&startup);
        let thread = match thread::Builder::new()
            .name(format!("agent-supervisor-{pid}"))
            .spawn(move || {
                let (process, stderr_thread) = thread_startup
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                    .expect("supervisor startup payload is present");
                let result = supervise(process, stderr_thread, command_rx);
                *thread_result
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
                let _ = wake_tx.send(());
            }) {
            Ok(thread) => thread,
            Err(error) => {
                let (process, stderr_thread) = startup
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                    .expect("failed supervisor startup retained its resources");
                let process_cleanup = process.cleanup(CleanupTrigger::Shutdown);
                let stderr_status = if stderr_thread.join().is_ok() {
                    ResourceJoinStatus::Joined
                } else {
                    ResourceJoinStatus::Failed("agent stderr reader panicked".to_string())
                };
                return Err(ManagedAgentSpawnError {
                    error: AgentRunError::transient(format!(
                        "start agent process supervisor: {error}"
                    )),
                    cleanup: setup_cleanup(process_cleanup, stderr_status),
                });
            }
        };

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
                        cleanup: missing_supervisor_cleanup(),
                    },
                });
                self.completed = true;
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn join_completed(&mut self) -> bool {
        self.thread
            .take()
            .is_none_or(|thread| thread.join().is_ok())
    }

    /// Enqueues cooperative cancellation without waiting for process exit.
    pub fn request_cancel(&self) -> bool {
        self.command.send(SupervisorCommand::Cancel).is_ok()
    }

    /// Starts the shared TERM/KILL/verify cleanup state machine.
    pub fn force_terminate(&self) -> bool {
        self.command.send(SupervisorCommand::ForceTerminate).is_ok()
    }

    /// Starts unconditional watchdog cleanup.
    pub fn hard_kill(&self) -> bool {
        self.command.send(SupervisorCommand::HardKill).is_ok()
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
            let _ = self.hard_kill();
            let _ = self.join_completed();
        }
    }
}

fn missing_supervisor_cleanup() -> JobCleanup {
    let mut cleanup = JobCleanup::no_process(None);
    cleanup.resources.process_supervisor =
        ResourceJoinStatus::Failed("agent supervisor ended without an outcome".to_string());
    cleanup.resources.stderr_reader =
        ResourceJoinStatus::Failed("agent stderr reader join is unknown".to_string());
    cleanup
}

fn setup_cleanup(
    containment: temper_process_containment::CleanupReport,
    stderr_reader: ResourceJoinStatus,
) -> JobCleanup {
    let mut resources = ResourceJoinReport::no_process();
    resources.stderr_reader = stderr_reader;
    JobCleanup {
        cancellation: None,
        containment,
        resources,
    }
}

fn receive_immediate_hard_kill(commands: &mpsc::Receiver<SupervisorCommand>) -> bool {
    let deadline = Instant::now() + PROCESS_POLL_INTERVAL;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match commands.recv_timeout(remaining) {
            Ok(SupervisorCommand::HardKill) => return true,
            Ok(SupervisorCommand::Cancel | SupervisorCommand::ForceTerminate) => {}
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                return false;
            }
        }
    }
}

fn supervise(
    process: ContainedProcess,
    stderr_thread: JoinHandle<String>,
    commands: mpsc::Receiver<SupervisorCommand>,
) -> SupervisorResult {
    let mut cancellation = None;
    let mut observed_status: Option<i32> = None;
    let mut process_error = None;
    let trigger = loop {
        match process.try_wait_root() {
            Ok(Some(status)) => {
                observed_status = status.code();
                break CleanupTrigger::NormalRootExit;
            }
            Ok(None) => {}
            Err(error) => {
                process_error = Some(AgentRunError::transient(format!(
                    "poll agent process exit: {error}"
                )));
                break CleanupTrigger::Shutdown;
            }
        }

        match commands.recv_timeout(PROCESS_POLL_INTERVAL) {
            Ok(SupervisorCommand::Cancel) => {
                if cancellation.is_none() {
                    cancellation = Some(CancellationOutcome::Graceful);
                }
            }
            Ok(SupervisorCommand::ForceTerminate) => {
                if !matches!(cancellation, Some(CancellationOutcome::HardKill)) {
                    cancellation = Some(CancellationOutcome::ForcedTermination);
                }
                // A caller can enqueue hard kill immediately after forced
                // termination. Give that already-started escalation one poll
                // interval to reach this owner before entering blocking cleanup.
                if receive_immediate_hard_kill(&commands) {
                    cancellation = Some(CancellationOutcome::HardKill);
                }
                break CleanupTrigger::Watchdog;
            }
            Ok(SupervisorCommand::HardKill) => {
                cancellation = Some(CancellationOutcome::HardKill);
                break CleanupTrigger::Watchdog;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                cancellation = Some(CancellationOutcome::HardKill);
                break CleanupTrigger::OwnerDrop;
            }
        }
    };

    // Cleanup is the sole transition to quiescence. It cannot return while an
    // inspection is blocked, a descendant survives, or the direct child has
    // not been reaped.
    let cleanup_report = process.cleanup(trigger);
    debug_assert!(matches!(
        cleanup_report.recursive_empty(),
        RecursiveEmptyProof::Proven { .. }
    ));
    let status_code = observed_status.or_else(|| cleanup_report.direct_child_reap().exit_code());
    let (stderr_tail, stderr_status) = match stderr_thread.join() {
        Ok(tail) => (tail, ResourceJoinStatus::Joined),
        Err(_) => (
            String::new(),
            ResourceJoinStatus::Failed("agent stderr reader panicked".to_string()),
        ),
    };
    let outcome = match process_error {
        Some(error) => Err(error),
        None => Ok(ChildOutcome {
            status_code,
            stderr_tail,
        }),
    };
    let mut resources = ResourceJoinReport::no_process();
    // The owner thread is joined by RunResources after this result is received.
    resources.process_supervisor = ResourceJoinStatus::Pending;
    resources.stderr_reader = stderr_status;
    SupervisorResult {
        outcome,
        quiesced: JobQuiesced {
            cleanup: JobCleanup {
                cancellation,
                containment: cleanup_report,
                resources,
            },
        },
    }
}
