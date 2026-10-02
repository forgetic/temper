//! The tools sub-model's state and its entry point (section 3).

use temper_lib::{Env, Queue, Slab};

use crate::boundary::{Event, Request};
use crate::job::{self, Job};
use crate::kit::{self, Kit};
use crate::limits::Limits;

/// The most requests one call of [`step`] emits under `limits`: closing a kit
/// cancels each call it is running, up to `limits.calls`, and any other event
/// emits at most two. The parent reserves this much room in `out` before
/// calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    if limits.calls > 2 { limits.calls } else { 2 }
}

/// The tools sub-model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) kits: Slab<Kit>,
    pub(crate) jobs: Slab<Job>,
}

impl Model {
    /// A model with room for `limits`.
    #[must_use]
    pub fn new(limits: &Limits) -> Model {
        let jobs = job::slots(limits).expect("worst_case accepted the limits");
        Model { kits: Slab::with_capacity(limits.kits), jobs: Slab::with_capacity(jobs) }
    }

    /// Kits present, closed ones included until they are reclaimed.
    #[must_use]
    pub fn kits(&self) -> u32 {
        self.kits.len()
    }

    /// Calls running, answered ones included until they are reclaimed.
    #[must_use]
    pub fn jobs(&self) -> u32 {
        self.jobs.len()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.jobs.reclaim();
        self.kits.reclaim();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Open { session, authority } => kit::open(model, env, session, authority, out),
        Event::Call { kit, reply_to, call, deadline } => kit::call(model, env, kit, reply_to, call, deadline, out),
        Event::Close { kit } => kit::close(model, kit, out),
        Event::Done { owner, done } => job::done(model, env, owner, done, out),
    }
}
