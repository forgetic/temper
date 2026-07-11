use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use temper_engine_io::{OneshotReceiver, OneshotSender, oneshot};

/// Stable synchronization points used by restart convergence scenarios.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PausePoint {
    AssignmentClaimCommitted,
    WorkerPushCompleted,
    ResultApplicationStarted,
    ResultApplicationCompleted,
    ChildCreated,
    ChildWired,
    ChildActivated,
    RecoveryBarrierOpening,
}

struct ArmedPause {
    arrived: OneshotSender<()>,
    release: OneshotReceiver<()>,
}

/// One-shot named pause registry. No polling or elapsed-time assumptions are
/// involved: the component announces arrival and waits on the matching permit.
#[derive(Clone, Default)]
pub struct PauseHooks {
    armed: Arc<Mutex<BTreeMap<PausePoint, ArmedPause>>>,
}

impl PauseHooks {
    /// Arms one point. Arming an already-armed point replaces the old permit.
    pub fn arm(&self, point: PausePoint) -> PausePermit {
        let (arrived_tx, arrived) = oneshot();
        let (release, release_rx) = oneshot();
        self.armed.lock().expect("pause hook lock").insert(
            point,
            ArmedPause {
                arrived: arrived_tx,
                release: release_rx,
            },
        );
        PausePermit {
            point,
            arrived,
            release: Some(release),
        }
    }

    /// Announces a point and blocks only when that point was armed.
    pub async fn reach(&self, point: PausePoint) {
        let armed = self.armed.lock().expect("pause hook lock").remove(&point);
        if let Some(armed) = armed {
            armed.arrived.send(());
            let _ = armed.release.recv().await;
        }
    }
}

/// Test-side half of one named pause.
pub struct PausePermit {
    point: PausePoint,
    arrived: OneshotReceiver<()>,
    release: Option<OneshotSender<()>>,
}

impl PausePermit {
    pub fn point(&self) -> PausePoint {
        self.point
    }

    /// Waits until the component reaches the point.
    pub async fn arrived(self) -> ReachedPause {
        let _ = self.arrived.recv().await;
        ReachedPause {
            point: self.point,
            release: self.release,
        }
    }
}

/// Permit held after a component has stopped at a named point.
pub struct ReachedPause {
    point: PausePoint,
    release: Option<OneshotSender<()>>,
}

impl ReachedPause {
    pub fn point(&self) -> PausePoint {
        self.point
    }

    pub fn release(mut self) {
        if let Some(release) = self.release.take() {
            release.send(());
        }
    }
}

impl Drop for ReachedPause {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release.send(());
        }
    }
}
