//! The fake engine's state and its entry points.
//!
//! Events take what a worker says and emit nothing: what follows goes out as
//! alarms fire, and from the ready list, which [`resume`] drains one entry at a
//! time: the cancels decided at a hello, then the due items, each placed on a
//! worker with a free slot.

use alloc::boxed::Box;

use temper_lib::{Deadlines, Duration, Env, Id, Map, Queue, Rng, Set, Slab, Time, Token};

use crate::api::{Answer, Assignment, Bounce, Hello};
use crate::charter;
use crate::fleet::{self, Worker};
use crate::traffic::{self, Call};
use crate::work::{self, Attempt, Item};
use crate::workspace;

/// The most requests an entry point emits per call.
pub const MAX_OUT: u32 = 1;

/// How the fake behaves, handed to every step read-only. Chances are per
/// mille.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    /// Work items to run, each falling due at a time drawn from the first
    /// `window`.
    pub items: u32,
    pub window: Duration,
    /// The most workers it knows at once.
    pub workers: u32,
    /// The workstream keys, and the repositories their workspaces are drawn
    /// from, `spread_min..=spread_max` to a workstream.
    pub workstreams: Box<[Box<[u8]>]>,
    pub repositories: Box<[Origin]>,
    pub spread_min: u32,
    pub spread_max: u32,
    /// The chances that a repository starts from a commit, and else from its
    /// workstream's branch, rather than from the base branch; that it is
    /// writable; that an item saves its unfinished work; and that an item's
    /// workspace is beyond what a worker takes.
    pub commits: u32,
    pub branches: u32,
    pub writable: u32,
    pub saves: u32,
    pub invalid: u32,
    /// The length of each brief, drawn from `brief_min..=brief_max`.
    pub brief_min: u32,
    pub brief_max: u32,
    /// Each budget's turns, tokens of every kind, and time, each drawn from
    /// its range.
    pub turns_min: u32,
    pub turns_max: u32,
    pub tokens_min: u64,
    pub tokens_max: u64,
    pub time_min: Duration,
    pub time_max: Duration,
    /// What each charter's LLM asks for as `max_tokens`.
    pub max_tokens: u32,
    /// The chances that a charter's outcome may be a change, that a change
    /// must pass its checks, that its outcome may be a verdict, and that it
    /// grants sub-agents. One that may be neither may be a change.
    pub changes: u32,
    pub checks: u32,
    pub verdicts: u32,
    pub agents: u32,
    /// The most assignments made for an item, refused ones included.
    pub attempts: u32,
    /// The chances that a failure a retry may get past is retried, and one it
    /// may not, each after a delay drawn from `backoff_min..=backoff_max`. A
    /// busy refusal is always retried, after such a delay.
    pub transient: u32,
    pub permanent: u32,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// The most wakes of an item, each after a delay drawn from
    /// `wake_min..=wake_max`, and the chance that a wake resumes the run from
    /// its snapshot, when there is one, rather than starting it fresh.
    pub wakes: u32,
    pub wake_min: Duration,
    pub wake_max: Duration,
    pub resumes: u32,
    /// The chance that an item falling due when no worker has a free slot is
    /// assigned all the same.
    pub overbook: u32,
    /// The most inbound events sent to an attempt, each after a delay drawn
    /// from `inbound_min..=inbound_max`, of a length drawn from
    /// `event_min..=event_max`; and the chance that an event bounced for want
    /// of room is sent again.
    pub inbound: u32,
    pub inbound_min: Duration,
    pub inbound_max: Duration,
    pub event_min: u32,
    pub event_max: u32,
    pub resends: u32,
    /// The chances that an attempt is cancelled, that one is cancelled again
    /// after it answered, and that an inbound event is sent for an attempt
    /// once its item has made another, each after a delay drawn from
    /// `cancel_min..=cancel_max`.
    pub cancels: u32,
    pub late_cancels: u32,
    pub stale: u32,
    pub cancel_min: Duration,
    pub cancel_max: Duration,
    /// The most relayed calls held at once. It must cover the worker's
    /// limits: its slots times the calls a run may have in flight, plus the
    /// calls answered in an iteration, held until the reclaim point. Each is
    /// answered after a delay drawn from `relay_min..=relay_max`, with an
    /// opaque body of a length drawn from `answer_min..=answer_max`,
    /// error-shaped by the `relay_errors` chance.
    pub calls: u32,
    pub relay_min: Duration,
    pub relay_max: Duration,
    pub relay_errors: u32,
    pub answer_min: u32,
    pub answer_max: u32,
    /// How long a worker that lost contact keeps its runs, and the chance
    /// that a run it reports on reconnecting is kept rather than cancelled.
    pub grace: Duration,
    pub keeps: u32,
}

/// A repository on the forge that workspaces are drawn from: the directory
/// name a workspace gives it, and its address.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Origin {
    pub name: Box<[u8]>,
    pub remote: Box<[u8]>,
}

/// protocol -> model
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The worker `worker` dialled in and said `hello`, first or again after
    /// losing contact. `worker` is the world's name for it, the same across
    /// reconnects.
    Hello { worker: Token, hello: Hello },
    /// The channel to the worker `worker` dropped.
    Lost { worker: Token },
    /// Terminal for `Assign`: the answer of the worker `worker` for the run
    /// `run`'s attempt `attempt`.
    Answered { worker: Token, run: Token, attempt: Token, answer: Answer },
    /// A host call of the run `run`'s attempt `attempt` on the worker
    /// `worker`, which names it `call`: a forge read or an outlet, relayed as
    /// it is. Answered by at most one `Relayed`.
    Relay { worker: Token, run: Token, attempt: Token, call: Token, body: Box<[u8]> },
    /// The worker `worker` did not pass on an inbound event for the run
    /// `run`'s attempt `attempt`, for `bounce`.
    Bounced { worker: Token, run: Token, attempt: Token, bounce: Bounce },
    /// A fact of the run `run`'s attempt `attempt` on the worker `worker`,
    /// forwarded best effort: only counted.
    Fact { worker: Token, run: Token, attempt: Token },
}

/// model -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Host `assignment` on the worker `worker`. Exactly one `Answered` comes
    /// for its attempt, unless the attempt is lost with the worker's contact.
    Assign { worker: Token, assignment: Assignment },
    /// An inbound event for the run `run`'s attempt `attempt`, opaque.
    Inbound { worker: Token, run: Token, attempt: Token, event: Box<[u8]> },
    /// Cancel the run `run`'s attempt `attempt`. Its answer still comes, if it
    /// has not already.
    Cancel { worker: Token, run: Token, attempt: Token },
    /// The answer to the relayed call `call` of the run `run`'s attempt
    /// `attempt`, opaque: at most one per call.
    Relayed { worker: Token, run: Token, attempt: Token, call: Token, answer: Box<[u8]> },
}

/// What the fake has done and heard, for a world to check at settle.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tally {
    /// Hellos heard.
    pub hellos: u32,
    /// Assignments made, one per attempt, and those made past the free slots.
    pub assigned: u32,
    pub overbooked: u32,
    /// Answers taken, by kind, those of attempts presumed lost aside.
    pub busy: u32,
    pub invalid: u32,
    pub ended: u32,
    pub parked: u32,
    pub failed: u32,
    /// Attempts presumed lost, and the answers that came for them later,
    /// which changed nothing.
    pub lost: u32,
    pub late: u32,
    /// Items due again after a refusal, a failure or a loss; items woken after
    /// a park, and the wakes drawn to resume a run from its snapshot.
    pub retries: u32,
    pub wakes: u32,
    pub resumed: u32,
    /// Cancels sent for attempts not answered yet, and for attempts that had
    /// answered.
    pub cancels: u32,
    pub late_cancels: u32,
    /// Inbound events sent to live attempts, and to attempts their items had
    /// replaced.
    pub inbound: u32,
    pub stale: u32,
    /// Inbound events bounced, and those sent again.
    pub bounced: u32,
    pub resent: u32,
    /// Relayed calls taken; answered, error-shaped ones among them; and those
    /// never answered, their worker out of contact past the grace.
    pub relayed: u32,
    pub replies: u32,
    pub errors: u32,
    pub dropped: u32,
    /// Calls, bounces and hello listings for attempts that had answered, were
    /// cancelled or were presumed lost, dropped as they came.
    pub fenced: u32,
    /// Repositories the answers say landed a change, and saved work.
    pub landed: u32,
    pub saved: u32,
    pub facts: u32,
    /// How the items closed.
    pub endings: Endings,
}

/// How the items closed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Endings {
    /// Their runs ended.
    pub finished: u32,
    /// Their assignments were refused as invalid.
    pub rejected: u32,
    /// Held for a person: a failure not retried, or no attempts left.
    pub held: u32,
    /// Parked with no wakes left.
    pub parked: u32,
    /// The engine cancelled them, and their runs did not end.
    pub cancelled: u32,
}

impl Tally {
    const ZERO: Tally = Tally {
        hellos: 0,
        assigned: 0,
        overbooked: 0,
        busy: 0,
        invalid: 0,
        ended: 0,
        parked: 0,
        failed: 0,
        lost: 0,
        late: 0,
        retries: 0,
        wakes: 0,
        resumed: 0,
        cancels: 0,
        late_cancels: 0,
        inbound: 0,
        stale: 0,
        bounced: 0,
        resent: 0,
        relayed: 0,
        replies: 0,
        errors: 0,
        dropped: 0,
        fenced: 0,
        landed: 0,
        saved: 0,
        facts: 0,
        endings: Endings { finished: 0, rejected: 0, held: 0, parked: 0, cancelled: 0 },
    };
}

/// The fake engine's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) items: Slab<Item>,
    /// The attempts not answered yet, presumed lost ones among them, by their
    /// tokens.
    pub(crate) attempts: Map<Token, Attempt>,
    /// Attempts made so far: each is named by its number, from one.
    pub(crate) made: u64,
    /// The workers that have said hello, by the world's names for them.
    pub(crate) workers: Map<Token, Worker>,
    pub(crate) calls: Slab<Call>,
    /// The calls in flight, by their attempts and the runs' names for them.
    pub(crate) named: Set<(Token, Token)>,
    /// Items due, waiting for a free slot, oldest first.
    pub(crate) due: Queue<Id<Item>>,
    /// Attempts to cancel, as a hello decided.
    pub(crate) cancels: Set<Token>,
    pub(crate) timers: Deadlines<Alarm>,
    pub(crate) rng: Rng,
    pub(crate) tally: Tally,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    /// The item falls due: its first start, a retry or a wake.
    Item(Id<Item>),
    /// The next inbound event for the attempt.
    Inbound(Token),
    /// The attempt's cancel.
    Cancel(Token),
    /// A cancel for the run `run`'s attempt `attempt` on `worker`, which has
    /// answered.
    Late { worker: Token, run: Token, attempt: Token },
    /// An inbound event for the run `run`'s attempt `attempt` on `worker`,
    /// which its item has replaced.
    Stale { worker: Token, run: Token, attempt: Token },
    /// The relayed call's answer.
    Call(Id<Call>),
    /// The worker has been out of contact for the grace.
    Grace(Token),
}

impl Model {
    /// An engine with `config.items` items to run, drawn from `seed`, from time
    /// zero.
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Model {
        assert!(
            !config.workstreams.is_empty() && !config.repositories.is_empty() && config.spread_min > 0,
            "a workstream and a repository to draw workspaces from"
        );
        assert!(config.attempts > 0, "an item is assigned at least once");
        let items = config.items;
        let attempts = items.saturating_mul(config.attempts);
        let mut model = Model {
            items: Slab::with_capacity(items),
            attempts: Map::with_capacity(attempts),
            made: 0,
            workers: Map::with_capacity(config.workers),
            calls: Slab::with_capacity(config.calls),
            named: Set::with_capacity(config.calls),
            due: Queue::with_capacity(items),
            cancels: Set::with_capacity(attempts),
            // An alarm per item, two per attempt out, a late cancel and a
            // stale event per attempt made, one per call, one per worker.
            timers: Deadlines::with_capacity(
                items
                    .saturating_mul(3)
                    .saturating_add(attempts.saturating_mul(2))
                    .saturating_add(config.calls)
                    .saturating_add(config.workers),
            ),
            rng: Rng::new(seed),
            tally: Tally::ZERO,
        };
        let streams = workspace::streams(&mut model.rng, config);
        for place in 0..items {
            let (workspace, save) = workspace::draw(&mut model.rng, config, &streams, place);
            let charter = charter::encode(&charter::draw(&mut model.rng, config, work::writable(&workspace)));
            let id = model.items.insert(Item::new(workspace, save, charter)).expect("a slot per item");
            let at = Time::ZERO.saturating_add(Duration::from_nanos(model.rng.below(config.window.as_nanos())));
            model.timers.arm(Alarm::Item(id), at).expect("a timer per item");
        }
        model
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Items not closed yet, closed ones included until they are reclaimed.
    #[must_use]
    pub fn items(&self) -> u32 {
        self.items.len()
    }

    /// Attempts the workers hold, as far as the engine knows: assignments not
    /// answered yet, those presumed lost aside unless a worker listed them
    /// since.
    #[must_use]
    pub fn outstanding(&self) -> u32 {
        let mut outstanding: u32 = 0;
        for (_, worker) in &self.workers {
            outstanding = outstanding.saturating_add(worker.placed.len());
        }
        outstanding
    }

    /// Relayed calls not answered or dropped yet, closed ones included until
    /// they are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.timers.next()
    }

    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.timers.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Whether a cancel is to be sent, or a due item has a worker with a free
    /// slot. While either holds, the loop calls [`resume`].
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.cancels.is_empty() || (!self.due.is_empty() && fleet::free(&self.workers).is_some())
    }

    pub fn reclaim(&mut self) {
        self.items.reclaim();
        self.calls.reclaim();
    }
}

/// Handles one event. Events emit nothing: see the module doc.
pub fn step(model: &mut Model, env: &Env<Config>, event: Event, _out: &mut Queue<Request>) {
    match event {
        Event::Hello { worker, hello } => fleet::hello(model, env, worker, hello),
        Event::Lost { worker } => fleet::lost(model, env, worker),
        Event::Answered { worker, run, attempt, answer } => work::answered(model, env, worker, run, attempt, answer),
        Event::Relay { worker, run, attempt, call, body: _ } => traffic::relay(model, env, worker, run, attempt, call),
        Event::Bounced { worker, run, attempt, bounce } => traffic::bounced(model, env, worker, run, attempt, bounce),
        Event::Fact { worker, run, attempt } => {
            // Checked as any traffic, then only counted, from fenced attempts too.
            work::state(model, worker, run, attempt);
            model.tally.facts = model.tally.facts.saturating_add(1);
        }
    }
}

/// Fires the earliest alarm due at `env.now`, if there is one, emitting at most
/// [`MAX_OUT`] requests.
pub fn fire(model: &mut Model, env: &Env<Config>, out: &mut Queue<Request>) {
    let Some(alarm) = model.timers.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Item(id) => work::due(model, env, id, out),
        Alarm::Inbound(attempt) => traffic::inbound(model, env, attempt, out),
        Alarm::Cancel(attempt) => traffic::cancel(model, env, attempt, out),
        Alarm::Late { worker, run, attempt } => traffic::late(model, worker, run, attempt, out),
        Alarm::Stale { worker, run, attempt } => traffic::stale(model, env, worker, run, attempt, out),
        Alarm::Call(id) => traffic::reply(model, env, id, out),
        Alarm::Grace(worker) => fleet::grace(model, env, worker),
    }
}

/// Takes the first entry of the ready list, if there is one, emitting at most
/// [`MAX_OUT`] requests: a cancel a hello decided, or else the oldest due
/// item, placed on the first worker with a free slot.
pub fn resume(model: &mut Model, env: &Env<Config>, out: &mut Queue<Request>) {
    if let Some(&attempt) = model.cancels.first() {
        model.cancels.remove(&attempt);
        traffic::recancel(model, attempt, out);
        return;
    }
    let Some(worker) = fleet::free(&model.workers) else {
        return;
    };
    let Some(id) = model.due.pop() else {
        return;
    };
    work::place(model, env, id, worker, out);
}
