//! The forge sub-model's state and its entry points (programming-style.md,
//! section 3).

use alloc::boxed::Box;

use temper_lib::{Deadlines, Env, Id, List, Map, Queue, Rng, Slab, Time};

use crate::boundary::Level;
use crate::boundary::{Event, Item, Request};
use crate::calls::{self, Calls};
use crate::facts::{Fact, Facts};
use crate::items::{self, Entry};
use crate::limits::{self, Limits};
use crate::reads::{self, Fetch};
use crate::scans::{self, Scan};
use crate::writes::{self, Lane, Writing};

/// The most requests an entry point emits per call under `limits`: a page of
/// a listing, each of its items offered, changed or left; or an item read,
/// announced, its labels changed, and a page of news, or as much news as its
/// inbox holds from its pull request; and the end of the cold start besides.
/// The parent reserves this much room in `out` before calling it.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    let listing = limits.page;
    let reading = limits.inbox.saturating_add(2);
    let most = if listing > reading { listing } else { reading };
    most.saturating_add(1)
}

/// What the deployment says of the forge, which the sub-model keeps.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    /// The engine's own forge user. Its comments are never news, and only its
    /// records are records.
    pub engine: u64,
    /// The label on every item the engine tracks, and the label that hands an
    /// issue to it (engine-model.md, 4.1 and 4.6). Each within the limits'
    /// `name_bytes`.
    pub tracking: Box<[u8]>,
    pub hand_in: Box<[u8]>,
}

/// The forge sub-model's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) config: Config,
    /// The working set, and its items by their names.
    pub(crate) entries: Slab<Entry>,
    pub(crate) index: Map<Item, Id<Entry>>,
    /// The pull requests linked to items of the working set, by their names.
    pub(crate) pulls: Map<Item, Id<Entry>>,
    /// Each repository's listings, by its index.
    pub(crate) scans: List<Scan>,
    pub(crate) calls: Calls,
    pub(crate) writes: Slab<Writing>,
    /// The last write of each lane: writes about one item, or one repository,
    /// go one at a time, in order.
    pub(crate) lanes: Map<Lane, Id<Writing>>,
    pub(crate) reads: Slab<Fetch>,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) rng: Rng,
    pub(crate) facts: Facts,
    pub(crate) loading: Loading,
}

/// How far the cold start is: repositories whose labels are still being
/// listed for the first time, items still being read for their records, and
/// whether its end has been told.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Loading {
    pub(crate) listing: u32,
    pub(crate) finding: u32,
    pub(crate) told: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    /// The budget's window ends.
    Window,
    /// The forge's rate limit resets.
    Reset,
    /// A repository's next listing is due.
    Poll(u32),
    /// A repository's slow pass lists its next page.
    Slow(u32),
    /// A backoff ends: the item's read, the fresh read or the write is tried
    /// again.
    Item(Id<Entry>),
    Read(Id<Fetch>),
    Write(Id<Writing>),
}

impl Model {
    /// A model with room for `limits` and the deployment's `config`, drawing
    /// its jitter from `seed`. It starts cold: each repository's first
    /// listing is due at once.
    #[must_use]
    pub fn new(limits: &Limits, config: Config, seed: u64) -> Model {
        assert!(limits::worst_case(limits).is_some(), "the shell refuses limits it cannot provision");
        let name = usize::try_from(limits.name_bytes).expect("a u32 fits in a usize");
        assert!(config.tracking.len() <= name && config.hand_in.len() <= name, "labels within the limits");
        let calls = limits::calls(limits).expect("worst_case accepted the limits");
        let alarms = limits::alarms(limits).expect("worst_case accepted the limits");
        let mut model = Model {
            config,
            entries: Slab::with_capacity(limits.items),
            index: Map::with_capacity(limits.items),
            pulls: Map::with_capacity(limits.items),
            scans: List::with_capacity(limits.repositories),
            calls: Calls::with_capacity(calls, limits.calls),
            writes: Slab::with_capacity(limits.writes),
            lanes: Map::with_capacity(limits.writes),
            reads: Slab::with_capacity(limits.reads),
            alarms: Deadlines::with_capacity(alarms),
            rng: Rng::new(seed),
            facts: Facts::with_capacity(limits.facts),
            loading: Loading { listing: limits.repositories, finding: 0, told: false },
        };
        scans::start(&mut model, limits);
        model
    }

    /// Items in the working set, those leaving included until they are
    /// reclaimed.
    #[must_use]
    pub fn items(&self) -> u32 {
        self.entries.len()
    }

    /// Whether `item` is in the working set.
    #[must_use]
    pub fn is_tracked(&self, item: Item) -> bool {
        self.index.contains_key(&item)
    }

    /// The pull request of `item`, as last read: what a step's decisions read
    /// of a change (seams: "Plans"). `None` until one is linked and read.
    #[must_use]
    pub fn pull(&self, item: Item) -> Option<Level> {
        let id = self.index.get(&item)?;
        items::level(self.entries.get(*id)?)
    }

    /// Calls in hand, queued or out, answered ones included until they are
    /// reclaimed; and those out.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.held()
    }

    #[must_use]
    pub fn calls_out(&self) -> u32 {
        self.calls.out()
    }

    /// Fresh reads and writes in hand, done ones included until they are
    /// reclaimed.
    #[must_use]
    pub fn reads(&self) -> u32 {
        self.reads.len()
    }

    #[must_use]
    pub fn writes(&self) -> u32 {
        self.writes.len()
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Whether an alarm is due at `now`. While one is, the loop fires the
    /// top-level model, which calls [`fire`].
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Whether a call is ready to go out within the request budget. While
    /// one is, the loop resumes the top-level model, which calls [`resume`],
    /// at the start of the model's stage, before its input events.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.calls.is_ready()
    }

    /// The oldest fact not yet drained. The parent drains them at its own
    /// pace; what does not fit meanwhile is dropped and counted.
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// How many facts were dropped for want of room, since the model was
    /// made.
    #[must_use]
    pub fn facts_lost(&self) -> u64 {
        self.facts.lost()
    }

    /// The reclaim point: frees what closed in this iteration.
    pub fn reclaim(&mut self) {
        self.entries.reclaim();
        self.calls.reclaim();
        self.writes.reclaim();
        self.reads.reclaim();
    }
}

/// Handles one event, emitting at most [`max_out`] requests.
pub fn step(model: &mut Model, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Track { item } => items::track(model, env, item, out),
        Event::Untrack { item } => items::untrack(model, env, item),
        Event::Link { item, pull } => items::link(model, env, item, pull),
        Event::Took { item, through } => items::took(model, env, item, through),
        Event::Hint { repository, item: _, commit } => scans::hint(model, env, repository, commit),
        Event::Read { owner, read } => reads::read(model, env, owner, read, out),
        Event::Write { owner, write, resumed } => writes::write(model, env, owner, write, resumed, out),
        Event::Answered { call, result } => calls::answered(model, env, call, result, out),
    }
    loaded(model, out);
}

/// Fires the earliest alarm due at `env.now`, if there is one. A stage fires
/// its alarms after its input events, so an answer that arrived in the same
/// iteration wins over a deadline that passed while the loop waited. An alarm
/// only makes work: what it makes goes out through [`resume`].
pub fn fire(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    let Some(alarm) = model.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Window => calls::window(model, env),
        Alarm::Reset => calls::reset(model, env),
        Alarm::Poll(repository) => scans::poll(model, env, repository),
        Alarm::Slow(repository) => scans::slow(model, repository),
        Alarm::Item(id) => items::retry(model, env, id),
        Alarm::Read(id) => reads::retry(model, env, id),
        Alarm::Write(id) => writes::retry(model, id),
    }
    loaded(model, out);
}

/// Sends the next call ready, if the request budget allows one, emitting at
/// most one request: the call.
pub fn resume(model: &mut Model, env: &Env<Limits>, out: &mut Queue<Request>) {
    calls::send(model, env, out);
}

/// Tells the end of the cold start, once: every repository's labels listed,
/// and every item they found read.
fn loaded(model: &mut Model, out: &mut Queue<Request>) {
    let loading = &mut model.loading;
    if !loading.told && loading.listing == 0 && loading.finding == 0 {
        loading.told = true;
        out.push(Request::Loaded);
    }
}
