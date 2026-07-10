// SPDX-License-Identifier: MPL-2.0

//! The model: ground truth the simulated world is checked against.
//!
//! Workers and the applier record what they observe; after the world settles
//! the test asserts conservation properties over the recorded state — every
//! applied job was enqueued, nothing applies twice, nothing is assigned to
//! two workers at once (unless chaos legitimately re-dispatches).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use temper_engine::{InFlightJob, ResultApplier};
use temper_engine_io::Spawner;
use temper_protocol_worker::JobResult;

/// Recorded world state. All maps are ordered so snapshots compare and print
/// deterministically.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct ModelState {
    /// Jobs the coordinator handed to the daemon.
    pub enqueued: BTreeSet<String>,
    /// Jobs currently assigned: job id → worker holding it.
    pub outstanding: BTreeMap<String, String>,
    /// Apply count per job id.
    pub applies: BTreeMap<String, u64>,
    /// Jobs assigned while already outstanding (double dispatch); legitimate
    /// only when chaos kills workers and leases re-dispatch.
    pub double_assignments: Vec<String>,
    /// Hard violations: conditions no schedule may produce.
    pub violations: Vec<String>,
}

/// Shared, thread-safe model handle.
#[derive(Clone, Default)]
pub struct SimModel(Arc<Mutex<ModelState>>);

impl SimModel {
    fn state(&self) -> MutexGuard<'_, ModelState> {
        self.0.lock().expect("model lock")
    }

    pub fn record_enqueue(&self, job_id: &str) {
        self.state().enqueued.insert(job_id.to_string());
    }

    pub fn record_assign(&self, job_id: &str, worker: &str) {
        let mut state = self.state();
        if !state.enqueued.contains(job_id) {
            state
                .violations
                .push(format!("assigned job {job_id} was never enqueued"));
        }
        if state.outstanding.contains_key(job_id) {
            state.double_assignments.push(job_id.to_string());
        }
        state
            .outstanding
            .insert(job_id.to_string(), worker.to_string());
    }

    pub fn record_release(&self, job_id: &str) {
        self.state().outstanding.remove(job_id);
    }

    pub fn record_apply(&self, job_id: &str) {
        let mut state = self.state();
        if !state.enqueued.contains(job_id) {
            state
                .violations
                .push(format!("applied job {job_id} was never enqueued"));
        }
        *state.applies.entry(job_id.to_string()).or_insert(0) += 1;
    }

    pub fn snapshot(&self) -> ModelState {
        self.state().clone()
    }
}

/// Conservation checks over a settled world.
impl ModelState {
    /// No hard violations recorded by any participant.
    pub fn assert_no_violations(&self) {
        assert!(
            self.violations.is_empty(),
            "model violations: {:?}",
            self.violations
        );
    }

    /// Every enqueued job applied exactly once — the fault-free contract.
    pub fn assert_exactly_once(&self) {
        self.assert_no_violations();
        for job in &self.enqueued {
            assert_eq!(
                self.applies.get(job).copied().unwrap_or(0),
                1,
                "job {job} must apply exactly once; applies: {:?}",
                self.applies
            );
        }
        assert!(
            self.double_assignments.is_empty(),
            "no job may be dispatched twice without faults: {:?}",
            self.double_assignments
        );
    }

    /// No job applied more than once — must hold under ANY chaos.
    pub fn assert_at_most_once(&self) {
        self.assert_no_violations();
        for (job, count) in &self.applies {
            assert!(
                *count <= 1,
                "job {job} applied {count} times; at-most-once must survive chaos"
            );
        }
    }
}

/// The applier seam backed by the model: optionally takes simulated time per
/// apply (opening the enqueue-during-apply race window), then records.
pub struct ModelApplier {
    pub model: SimModel,
    pub spawner: Arc<dyn Spawner>,
    pub apply_time: Duration,
}

#[async_trait::async_trait]
impl ResultApplier for ModelApplier {
    async fn apply(&self, job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        if !self.apply_time.is_zero() {
            // `apply` runs on a task whose Cx lives in the executor's spawn
            // closure; borrow a timer through a helper task's own Cx instead.
            let (tx, rx) = temper_engine_io::oneshot();
            let delay = self.apply_time;
            self.spawner.spawn_with_cx(move |cx| async move {
                skein::time::sleep(temper_engine_io::runtime::timer_now(&cx), delay).await;
                tx.send(());
            });
            let _ = rx.recv().await;
        }
        self.model.record_apply(&job.job_id);
        temper_engine::ApplyOutcome::Applied
    }
}
