// SPDX-License-Identifier: MPL-2.0

//! Companion [`ChangeSource`](temper_forge::ChangeSource) wiring for daemon wake scans.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use temper_engine_io::CqSender;
use temper_forge::{ChangeSource, ChangeSourceEvent, Forge};
use temper_workflow::{CompiledWorkflow, ValidatedWorkflow};

use crate::RoleFeedTarget;
use crate::lease_applier::WallClock;

use super::machine::DaemonCompletion;
use super::wake_coordinator::WakeRequest;
use super::{Daemon, HintedMechanical};

const CHANGE_SOURCE_RECV_TIMEOUT: Duration = Duration::from_millis(100);

pub(super) struct ChangeSourceListener {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ChangeSourceListener {
    fn spawn(mut source: Box<dyn ChangeSource + Send>, cq: CqSender<DaemonCompletion>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let join = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match source.recv_timeout(CHANGE_SOURCE_RECV_TIMEOUT) {
                    ChangeSourceEvent::Hint(hint) => {
                        if cq
                            .send(DaemonCompletion::ScheduleWake {
                                request: WakeRequest::from_hint(hint),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    ChangeSourceEvent::Timeout => {}
                    ChangeSourceEvent::Closed => break,
                }
            }
        });
        Self {
            stop,
            join: Some(join),
        }
    }
}

impl Drop for ChangeSourceListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Daemon {
    /// Wires a local backend change source into coordinated daemon wake work.
    ///
    /// This is the same admitted execution path used after a verified live
    /// webhook, without
    /// enabling the Forgejo webhook HTTP route. Hints are never authoritative;
    /// each hint only wakes a normal Forge scan for the configured targets.
    pub fn with_change_source<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        source: Box<dyn ChangeSource + Send>,
        wake_targets: Vec<RoleFeedTarget>,
        clock: WallClock,
    ) -> Self {
        self.with_change_source_and_mechanical(
            forge,
            workflow,
            compiled,
            source,
            wake_targets,
            clock,
            None,
        )
    }

    /// Like [`Self::with_change_source`], but also drives the mechanical
    /// accelerator for hinted repositories.
    #[allow(clippy::too_many_arguments)]
    pub fn with_change_source_and_mechanical<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        source: Box<dyn ChangeSource + Send>,
        wake_targets: Vec<RoleFeedTarget>,
        clock: WallClock,
        mechanical: Option<Arc<dyn HintedMechanical>>,
    ) -> Self {
        let daemon =
            self.with_wake_execution(forge, workflow, compiled, wake_targets, clock, mechanical);
        let listener = ChangeSourceListener::spawn(source, daemon.cq.clone());
        daemon
            .change_source_listeners
            .lock()
            .expect("change source listeners")
            .push(listener);
        daemon
    }
}
