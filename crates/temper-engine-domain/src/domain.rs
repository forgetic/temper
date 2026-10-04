//! The domain's state and its entry points (engine-domain.md, section 3). Each
//! hands what it is given to the child domain, or the part of the top level,
//! it is for, then completes the hand-offs between the child domains
//! (programming-model.md, 4.5) before it returns, routing what is for the
//! protocol layer out (the `route` module).

use alloc::boxed::Box;

use skein_lib::{Env, Id, Map, Queue, Set, Slab, Time, Token};
use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_forge as forge;
use temper_engine_domain_notes as notes;
use temper_engine_domain_views as views;
use temper_engine_domain_work as work;

use crate::boundary::{Decoded, Event, Item, Request};
use crate::config::Config;
use crate::facts::Fact;
use crate::items::Entry;
use crate::limits::{self, Limits};
use crate::route;
use crate::waits::{Carried, Wait};

/// The most requests an entry point emits per call under `limits`: each of
/// the child domains' requests is routed once, to the protocol layer or to a
/// sibling, and the child domains take at most `limits.steps` steps each in
/// an entry point. The loop reserves this much room in `out` before calling
/// it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    limits::routed(limits).saturating_mul(2)
}

/// The engine domain's state: its child domains', the table of items, what
/// waits for an answer, what it carries for the fleet, and room for what
/// the child domains emit within an entry point.
#[derive(Debug)]
pub struct Domain {
    pub(crate) config: Config,
    pub(crate) accounts: crate::accounts::Domain,
    pub(crate) account_out: Queue<crate::accounts::Request>,
    pub(crate) account_start: usize,
    pub(crate) account_epoch: Time,
    pub(crate) account_waiting: Set<Id<Entry>>,
    pub(crate) grant_pending: Set<(Id<Entry>, u32)>,
    pub(crate) account_wake: bool,
    pub(crate) work: work::Domain,
    pub(crate) forge: forge::Domain,
    pub(crate) fleet: fleet::Domain,
    pub(crate) brief: brief::Domain,
    pub(crate) notes: notes::Domain,
    pub(crate) wiki_pending: Set<notes::Scope>,
    pub(crate) views: views::Domain,
    /// The items held, by their names.
    pub(crate) items: Slab<Entry>,
    pub(crate) names: Map<Item, Id<Entry>>,
    pub(crate) waits: Slab<Wait>,
    /// Items whose job the forge refused as busy: their stage goes again.
    pub(crate) stalled: Queue<Id<Entry>>,
    /// Items whose record written on the side goes again: the forge refused
    /// it as busy, or it changed while one was in flight.
    pub(crate) resaves: Queue<Id<Entry>>,
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
    /// Whether the forge child domain's cold read is done.
    pub(crate) read: bool,
    /// When the cold start was done, every claim its records hold adopted.
    pub(crate) loaded: Option<Time>,
    /// The payloads decoded in the forge's answer being routed.
    pub(crate) decoded: Box<[Decoded]>,
    /// The next watcher's token.
    pub(crate) watchers: u64,
    /// The tokens the deployment's runs have spent.
    pub(crate) spent: u64,
    /// What each child domain emits in an entry point, until it is routed, and
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
    pub(crate) watches: Map<Token, skein_lib::ReplyTo>,
    pub(crate) steps: Steps,
    facts: Queue<Fact>,
    lost: u64,
}

/// The steps an entry point took of each child domain.
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

impl Domain {
    /// A domain of the deployment `config`, which [`crate::accepts`] takes,
    /// with room for `limits`, which [`crate::worst_case`] accepts, drawing
    /// randomness from `seed`, made at `now`.
    #[must_use]
    pub fn new(config: Config, limits: &Limits, seed: u64, now: Time) -> Domain {
        assert!(limits::accepts(&config, limits), "the limits take the configuration");
        let waits = limits::waits(limits).expect("worst_case accepted the limits");
        let carried = limits::carried(limits).expect("worst_case accepted the limits");
        let facts = limits::facts(limits).expect("worst_case accepted the limits");
        let items = limits.work.items;
        let forge = forge::Domain::new(&limits.forge, config.forge.clone(), seed.rotate_left(17));
        Domain {
            accounts: crate::accounts::Domain::new(&limits.accounts),
            account_out: Queue::with_capacity(limits::account_out(limits)),
            account_start: 0,
            account_epoch: now,
            account_waiting: Set::with_capacity(limits.work.items),
            grant_pending: Set::with_capacity(
                limits
                    .work
                    .items
                    .checked_mul(limits::grant_accounts(limits).expect("account counts fit"))
                    .expect("worst_case accepted account fanout"),
            ),
            account_wake: false,
            work: work::Domain::new(&limits.work, seed),
            forge,
            fleet: fleet::Domain::new(&limits.fleet),
            brief: brief::Domain::new(&limits.brief),
            notes: notes::Domain::new(&limits.notes),
            wiki_pending: Set::with_capacity(limits.notes.scopes),
            views: views::Domain::new(&limits.views, now),
            config,
            items: Slab::with_capacity(limits::entries(limits).expect("worst_case accepted the limits")),
            names: Map::with_capacity(items),
            waits: Slab::with_capacity(waits),
            stalled: Queue::with_capacity(items),
            resaves: Queue::with_capacity(items),
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
    pub const fn work(&self) -> &work::Domain {
        &self.work
    }

    /// The forge child domain, for a world to look at.
    #[must_use]
    pub const fn forge(&self) -> &forge::Domain {
        &self.forge
    }

    /// The fleet, for a world to look at.
    #[must_use]
    pub const fn fleet(&self) -> &fleet::Domain {
        &self.fleet
    }

    /// The brief child domain, for a world to look at.
    #[must_use]
    pub const fn brief(&self) -> &brief::Domain {
        &self.brief
    }

    /// The notes child domain, for a world to look at.
    #[must_use]
    pub const fn notes(&self) -> &notes::Domain {
        &self.notes
    }

    /// The views child domain, for a world to look at.
    #[must_use]
    pub const fn views(&self) -> &views::Domain {
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

    /// When the earliest alarm falls due: a child domain's.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut next = self.work.next_deadline();
        for at in [
            self.accounts.next_deadline(),
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

    /// Whether a child domain has work on its ready list, or a job the forge
    /// refused as busy waits to go again. While one does, the loop calls
    /// [`resume`] at the start of the domain's stage, before its input
    /// events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.account_start < self.config.accounts.len()
            || self.account_wake
            || !self.grant_pending.is_empty()
            || self.forge.is_ready()
            || self.fleet.is_ready()
            || !self.wiki_pending.is_empty()
            || self.notes.is_ready()
            || !self.stalled.is_empty()
            || !self.resaves.is_empty()
    }

    /// The oldest fact not drained yet, a child domain's or the top level's.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, the child domains'
    /// included.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.lost
            .saturating_add(self.accounts.facts_lost())
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
pub(crate) fn keep(domain: &mut Domain, fact: Fact) {
    if domain.facts.try_push(fact).is_err() {
        domain.lost = domain.lost.saturating_add(1);
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    domain.steps = Steps::NONE;
    route::event(domain, env, event, out);
    settle(domain, env, out);
}

/// Fires the earliest alarm due at `env.now`, a child domain's, if there is
/// one, emitting at most [`max_out`] requests. On a tie, the hub's goes
/// first, then the forge child domain's, the fleet's, the brief's and the
/// views'.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    domain.steps = Steps::NONE;
    let Some(at) = domain.next_deadline() else { return };
    if at > env.now {
        return;
    }
    if domain.work.next_deadline() == Some(at) {
        route::work_fire(domain, env);
    } else if domain.forge.next_deadline() == Some(at) {
        route::forge_fire(domain, env);
    } else if domain.fleet.next_deadline() == Some(at) {
        route::fleet_fire(domain, env);
    } else if domain.brief.next_deadline() == Some(at) {
        route::brief_fire(domain, env);
    } else if domain.accounts.next_deadline() == Some(at) {
        crate::credentials::fire(domain, env);
    } else {
        route::views_fire(domain, env);
    }
    settle(domain, env, out);
}

/// Goes on with one thing on a ready list, if there is one, emitting at most
/// [`max_out`] requests: a call of the forge child domain's within its request
/// budget, a placement of the fleet's, a call of the notes', or a job the
/// forge refused as busy.
pub fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    domain.steps = Steps::NONE;
    if crate::credentials::ready(domain) {
        crate::credentials::resume(domain, env);
    } else if !domain.wiki_pending.is_empty() {
        route::wiki_resume(domain, env);
    } else if domain.forge.is_ready() {
        route::forge_resume(domain, env);
    } else if domain.fleet.is_ready() {
        route::fleet_resume(domain, env);
    } else if domain.notes.is_ready() {
        route::notes_resume(domain, env);
    } else if let Some(id) = domain.stalled.pop() {
        crate::jobs::again(domain, env, id);
    } else if let Some(id) = domain.resaves.pop() {
        crate::items::aside(domain, env, id);
    }
    settle(domain, env, out);
}

/// Completes the hand-offs, then gathers the facts.
fn settle(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    route::hand_off(domain, env, out);
    for _ in 0..domain.requests.len() {
        let Some(request) = domain.requests.pop() else { break };
        out.push(request);
    }
    domain.decoded = Box::new([]);
    gather(domain, &env.limits);
}

/// Drains the child domains' facts into the domain's own queue, counting what
/// does not fit.
fn gather(domain: &mut Domain, limits: &Limits) {
    for _ in 0..limits.accounts.facts {
        let Some(fact) = domain.accounts.pop_fact() else { break };
        keep(domain, Fact::Accounts { fact });
    }
    for _ in 0..limits.work.facts {
        let Some(fact) = domain.work.pop_fact() else { break };
        keep(domain, Fact::Work { fact });
    }
    for _ in 0..limits.forge.facts {
        let Some(fact) = domain.forge.pop_fact() else { break };
        keep(domain, Fact::Forge { fact });
    }
    for _ in 0..limits.fleet.facts {
        let Some(fact) = domain.fleet.pop_fact() else { break };
        keep(domain, Fact::Fleet { fact });
    }
    for _ in 0..limits.brief.facts {
        let Some(fact) = domain.brief.pop_fact() else { break };
        keep(domain, Fact::Brief { fact });
    }
    for _ in 0..limits.notes.facts {
        let Some(fact) = domain.notes.pop_fact() else { break };
        keep(domain, Fact::Notes { fact });
    }
    for _ in 0..limits.views.facts {
        let Some(fact) = domain.views.pop_fact() else { break };
        keep(domain, Fact::Views { fact });
    }
}
