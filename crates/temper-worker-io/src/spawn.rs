//! Spawn capability shared by production and lab worker harnesses.
//!
//! Production worker code holds a [`skein::runtime::RuntimeHandle`]; simulation
//! harnesses hold a [`skein::lab::LabSpawner`]. Both can spawn tasks that receive
//! their own [`Cx`](skein::cx::Cx), so the worker shell is generic over this tiny
//! capability instead of being tied to one concrete runtime handle.

use std::future::Future;

use skein::cx::Cx;

/// Capability to spawn worker I/O tasks.
///
/// The closure receives the spawned task's own [`Cx`] by value. That keeps clock
/// and cancellation authority explicit and lets the same worker shell run on the
/// production skein runtime or the deterministic lab runtime.
pub trait Spawner: Clone + Send + Sync + 'static {
    fn spawn_task_with_cx<F, Fut>(&self, f: F)
    where
        F: FnOnce(Cx) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static;

    fn spawn_task<Fut>(&self, future: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.spawn_task_with_cx(move |_cx| future);
    }
}

impl Spawner for skein::runtime::RuntimeHandle {
    fn spawn_task_with_cx<F, Fut>(&self, f: F)
    where
        F: FnOnce(Cx) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.spawn_with_cx(f);
    }
}

impl Spawner for skein::lab::LabSpawner {
    fn spawn_task_with_cx<F, Fut>(&self, f: F)
    where
        F: FnOnce(Cx) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.spawn_with_cx(f);
    }
}
