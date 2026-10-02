//! The model's state and its entry points (engine-model.md, section 3). Each
//! hands what it is given to the sub-model, or the part of the top level,
//! it is for, then completes the hand-offs between the sub-models
//! (programming-style.md, 4.5) before it returns, routing what is for the
//! protocol layer out (the `route` module).

use alloc::boxed::Box;

use temper_engine_model_brief as brief;
use temper_engine_model_fleet as fleet;
use temper_engine_model_forge as forge;
use temper_engine_model_notes as notes;
use temper_engine_model_views as views;
use temper_engine_model_work as work;
use temper_lib::{Env, Id, Map, Queue, Slab, Time, Token};

use crate::boundary::{Decoded, Event, Item, Request};
use crate::config::Config;
use crate::facts::Fact;
use crate::items::Entry;
use crate::limits::{self, Limits};
use crate::route;
use crate::waits::{Carried, Wait};

/// The most requests an entry point emits per call under `limits`: each of
/// the sub-models' requests is routed once, to the protocol layer or to a
/// sibling, and the sub-models take at most `limits.steps` steps each in
/// an entry point. The loop reserves this much room in `out` before calling
/// it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    limits::routed(limits).saturating_mul(2)
}

/// The engine model's state: its sub-models', the table of items, what
/// waits for an answer, what it carries for the fleet, and room for what
/// the sub-models emit within an entry point.
#[derive(Debug)]
pub struct Model {
    pub(crate) config: Config,
    pub(crate) work: work::Model,
    pub(crate) forge: forge::Model,
    pub(crate) fleet: fleet::Model,
    pub(crate) brief: brief::Model,
    pub(crate) notes: notes::Model,
    pub(crate) views: views::Model,
    /// The items held, by their names.
    pub(crate) items: Slab<Entry>,
    pub(crate) names: Map<Item, Id<Entry>>,
    pub(crate) waits: Slab<Wait>,
    /// Items whose job the forge refused as busy: their stage goes again.
    pub(crate) stalled: Queue<Id<Entry>>,
    pub(crate) carried: Slab<Carried>,
    /// The answers handed to the hub and not yet acknowledged, by their
    /// item and attempt: the fleet's `Acknowledge` goes once the hub has
    /// them durably, or does not want them.
    pub(crate) handed: Map<(Item, u64), Token>,
    /// Runs prepared before the cold start was done, which start once it is.
    pub(crate) held: Queue<Id<Entry>>,
    /// Items handed in that the working set had no room for: tracked again
    /// once it has.
    pub(crate) roomless: Queue<Item>,
    /// Whether the forge sub-model's cold read is done.
    pub(crate) read: bool,
    /// When the cold start was done, every claim its records hold adopted.
    pub(crate) loaded: Option<Time>,
    /// The payloads decoded in the forge's answer being routed.
    pub(crate) decoded: Box<[Decoded]>,
    /// The next watcher's token.
    pub(crate) watchers: u64,
    /// The tokens the deployment's runs have spent.
    pub(crate) spent: u64,
    /// What each sub-model emits in an entry point, until it is routed, and
    /// how many steps of it the entry point took. Empty between entry points.
    pub(crate) work_out: Queue<work::Request>,
    pub(crate) forge_out: Queue<forge::Request>,
    pub(crate) fleet_out: Queue<fleet::Request>,
    pub(crate) brief_out: Queue<brief::Request>,
    pub(crate) notes_out: Queue<notes::Request>,
    pub(crate) views_out: Queue<views::Request>,
    /// What the top level asks of the protocol layer itself as it routes
    /// (the store, people), until the entry point hands it out.
    pub(crate) requests: Queue<Request>,
    /// The people's watches being taken, by their watchers' tokens.
    pub(crate) watches: Map<Token, temper_lib::ReplyTo>,
    pub(crate) steps: Steps,
    facts: Queue<Fact>,
    lost: u64,
}

/// The steps an entry point took of each sub-model.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Steps {
    pub(crate) work: u32,
    pub(crate) forge: u32,
    pub(crate) fleet: u32,
    pub(crate) brief: u32,
    pub(crate) notes: u32,
    pub(crate) views: u32,
}

impl Steps {
    pub(crate) const NONE: Steps = Steps { work: 0, forge: 0, fleet: 0, brief: 0, notes: 0, views: 0 };
}

impl Model {
    /// A model of the deployment `config`, which [`crate::accepts`] takes,
    /// with room for `limits`, which [`crate::worst_case`] accepts, drawing
    /// randomness from `seed`, made at `now`.
    #[must_use]
    pub fn new(config: Config, limits: &Limits, seed: u64, now: Time) -> Model {
        assert!(limits::accepts(&config, limits), "the limits take the configuration");
        let waits = limits::waits(limits).expect("worst_case accepted the limits");
        let carried = limits::carried(limits).expect("worst_case accepted the limits");
        let facts = limits::facts(limits).expect("worst_case accepted the limits");
        let items = limits.work.items;
        let forge = forge::Model::new(&limits.forge, config.forge.clone(), seed.rotate_left(17));
        Model {
            work: work::Model::new(&limits.work, seed),
            forge,
            fleet: fleet::Model::new(&limits.fleet),
            brief: brief::Model::new(&limits.brief),
            notes: notes::Model::new(&limits.notes),
            views: views::Model::new(&limits.views, now),
            config,
            items: Slab::with_capacity(items),
            names: Map::with_capacity(items),
            waits: Slab::with_capacity(waits),
            stalled: Queue::with_capacity(items),
            carried: Slab::with_capacity(carried),
            handed: Map::with_capacity(limits.fleet.attempts),
            held: Queue::with_capacity(items),
            roomless: Queue::with_capacity(items),
            read: false,
            loaded: None,
            decoded: Box::new([]),
            watchers: 0,
            spent: 0,
            work_out: Queue::with_capacity(limits::work_out(limits)),
            forge_out: Queue::with_capacity(limits::forge_out(limits)),
            fleet_out: Queue::with_capacity(limits::fleet_out(limits)),
            brief_out: Queue::with_capacity(limits::brief_out(limits)),
            notes_out: Queue::with_capacity(limits::notes_out(limits)),
            views_out: Queue::with_capacity(limits::views_out(limits)),
            requests: Queue::with_capacity(limits::routed(limits)),
            watches: Map::with_capacity(limits.asks),
            steps: Steps::NONE,
            facts: Queue::with_capacity(facts),
            lost: 0,
        }
    }

    /// The hub, for a world to look at.
    #[must_use]
    pub const fn work(&self) -> &work::Model {
        &self.work
    }

    /// The forge sub-model, for a world to look at.
    #[must_use]
    pub const fn forge(&self) -> &forge::Model {
        &self.forge
    }

    /// The fleet, for a world to look at.
    #[must_use]
    pub const fn fleet(&self) -> &fleet::Model {
        &self.fleet
    }

    /// The brief sub-model, for a world to look at.
    #[must_use]
    pub const fn brief(&self) -> &brief::Model {
        &self.brief
    }

    /// The notes sub-model, for a world to look at.
    #[must_use]
    pub const fn notes(&self) -> &notes::Model {
        &self.notes
    }

    /// The views sub-model, for a world to look at.
    #[must_use]
    pub const fn views(&self) -> &views::Model {
        &self.views
    }

    /// Items held, those gone included until they are reclaimed.
    #[must_use]
    pub const fn items(&self) -> u32 {
        self.items.len()
    }

    /// What waits for an answer, answered waits included until they are
    /// reclaimed.
    #[must_use]
    pub const fn waits(&self) -> u32 {
        self.waits.len()
    }

    /// What the top level carries for the fleet.
    #[must_use]
    pub const fn carried(&self) -> u32 {
        self.carried.len()
    }

    /// Whether the cold start is done.
    #[must_use]
    pub const fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    /// When the earliest alarm falls due: a sub-model's.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut next = self.work.next_deadline();
        for at in [
            self.forge.next_deadline(),
            self.fleet.next_deadline(),
            self.brief.next_deadline(),
            self.views.next_deadline(),
        ] {
            next = earlier(next, at);
        }
        next
    }

    /// Whether an alarm is due at `now`. The loop calls [`fire`] while one is.
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.next_deadline() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Whether a sub-model has work on its ready list, or a job the forge
    /// refused as busy waits to go again. While one does, the loop calls
    /// [`resume`] at the start of the model's stage, before its input
    /// events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.forge.is_ready() || self.fleet.is_ready() || self.notes.is_ready() || !self.stalled.is_empty()
    }

    /// The oldest fact not drained yet, a sub-model's or the top level's.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, the sub-models' included.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.lost
            .saturating_add(self.work.facts_lost())
            .saturating_add(self.forge.facts_lost())
            .saturating_add(self.fleet.facts_lost())
            .saturating_add(self.brief.facts_lost())
            .saturating_add(self.notes.facts_lost())
            .saturating_add(self.views.facts_lost())
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.work.reclaim();
        self.forge.reclaim();
        self.fleet.reclaim();
        self.brief.reclaim();
        self.notes.reclaim();
        self.views.reclaim();
        self.items.reclaim();
        self.waits.reclaim();
        self.carried.reclaim();
    }
}

const fn earlier(one: Option<Time>, other: Option<Time>) -> Option<Time> {
    match one {
        Some(one) => match other {
            Some(other) => Some(if other.as_nanos() < one.as_nanos() { other } else { one }),
            None => Some(one),
        },
        None => other,
    }
}

/// Keeps `fact` if there is room for it, and counts it otherwise.
pub(crate) fn keep(model: &mut Model, fact: Fact) {
    if model.facts.try_push(fact).is_err() {
        model.lost = model.lost.saturating_add(1);
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    model.steps = Steps::NONE;
    route::event(model, env, event, out);
    settle(model, env, out);
}

/// Fires the earliest alarm due at `env.now`, a sub-model's, if there is
/// one, emitting at most [`max_out`] requests. On a tie, the hub's goes
/// first, then the forge sub-model's, the fleet's, the brief's and the
/// views'.
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    model.steps = Steps::NONE;
    let Some(at) = model.next_deadline() else { return };
    if at > env.now {
        return;
    }
    if model.work.next_deadline() == Some(at) {
        route::work_fire(model, env);
    } else if model.forge.next_deadline() == Some(at) {
        route::forge_fire(model, env);
    } else if model.fleet.next_deadline() == Some(at) {
        route::fleet_fire(model, env);
    } else if model.brief.next_deadline() == Some(at) {
        route::brief_fire(model, env);
    } else {
        route::views_fire(model, env);
    }
    settle(model, env, out);
}

/// Goes on with one thing on a ready list, if there is one, emitting at most
/// [`max_out`] requests: a call of the forge sub-model's within its request
/// budget, a placement of the fleet's, a call of the notes', or a job the
/// forge refused as busy.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    model.steps = Steps::NONE;
    if model.forge.is_ready() {
        route::forge_resume(model, env);
    } else if model.fleet.is_ready() {
        route::fleet_resume(model, env);
    } else if model.notes.is_ready() {
        route::notes_resume(model, env);
    } else if let Some(id) = model.stalled.pop() {
        crate::jobs::again(model, env, id);
    }
    settle(model, env, out);
}

/// Completes the hand-offs, then gathers the facts.
fn settle(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    route::hand_off(model, env, out);
    for _ in 0..model.requests.len() {
        let Some(request) = model.requests.pop() else { break };
        out.push(request);
    }
    model.decoded = Box::new([]);
    gather(model, &env.limits);
}

/// Drains the sub-models' facts into the model's own queue, counting what does
/// not fit.
fn gather(model: &mut Model, limits: &Limits) {
    for _ in 0..limits.work.facts {
        let Some(fact) = model.work.pop_fact() else { break };
        keep(model, Fact::Work { fact });
    }
    for _ in 0..limits.forge.facts {
        let Some(fact) = model.forge.pop_fact() else { break };
        keep(model, Fact::Forge { fact });
    }
    for _ in 0..limits.fleet.facts {
        let Some(fact) = model.fleet.pop_fact() else { break };
        keep(model, Fact::Fleet { fact });
    }
    for _ in 0..limits.brief.facts {
        let Some(fact) = model.brief.pop_fact() else { break };
        keep(model, Fact::Brief { fact });
    }
    for _ in 0..limits.notes.facts {
        let Some(fact) = model.notes.pop_fact() else { break };
        keep(model, Fact::Notes { fact });
    }
    for _ in 0..limits.views.facts {
        let Some(fact) = model.views.pop_fact() else { break };
        keep(model, Fact::Views { fact });
    }
}
