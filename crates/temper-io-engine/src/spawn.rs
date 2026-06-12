// SPDX-License-Identifier: MPL-2.0

//! The spawn seam: a capability to start engine tasks, passed explicitly.
//!
//! Production code holds a [`skein::runtime::RuntimeHandle`]; simulation
//! harnesses hold a [`skein::lab::LabSpawner`]. Both implement [`Spawner`],
//! so executors written against `Arc<dyn Spawner>` run unchanged under the
//! real runtime and under the deterministic lab. There is no ambient
//! fallback: whoever wants to spawn must be handed a spawner.

use std::future::Future;
use std::pin::Pin;

use skein::cx::Cx;

/// A boxed task body that receives the new task's own capability context.
pub type SpawnFactory = Box<dyn FnOnce(Cx) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Capability to spawn engine tasks.
///
/// The factory style mirrors `RuntimeHandle::spawn_with_cx`: the body is
/// handed the task's own [`Cx`] by value, so it never needs ambient context
/// for clocks or deadlines.
pub trait Spawner: Send + Sync {
    fn spawn_boxed(&self, factory: SpawnFactory);
}

impl dyn Spawner + '_ {
    /// Spawn a task; the closure receives the task's own [`Cx`].
    pub fn spawn_with_cx<F, Fut>(&self, f: F)
    where
        F: FnOnce(Cx) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.spawn_boxed(Box::new(move |cx| Box::pin(f(cx))));
    }
}

impl Spawner for skein::runtime::RuntimeHandle {
    fn spawn_boxed(&self, factory: SpawnFactory) {
        self.spawn_with_cx(factory);
    }
}

impl Spawner for skein::lab::LabSpawner {
    fn spawn_boxed(&self, factory: SpawnFactory) {
        self.spawn_with_cx(factory);
    }
}
