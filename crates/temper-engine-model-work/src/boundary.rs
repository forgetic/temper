//! The records that cross the boundary with the work hub's parent, the
//! engine's top-level model (programming-style.md, 4.5), which routes them to
//! and from the plan, the rules, the forge sub-model, the fleet and people.
//! The work hub defines them; its parent depends on it.
//!
//! Three shapes cross it:
//!
//! - Calls in. [`Event::Take`] is answered by exactly one [`Request::Taken`]
//!   or [`Request::Refused`], [`Event::Stop`] by one [`Request::Stopped`] or
//!   `Refused`, and [`Event::Release`] by one [`Request::Released`] or
//!   `Refused`, each at once and echoing its `reply_to`. Taking in is refused
//!   at the entrance when the working set is full: the item waits there, and
//!   is offered again; nothing taken in is dropped.
//! - Requests out, each ended by exactly one terminal event that echoes its
//!   `owner`, the hub's token for the item: [`Request::Due`] by
//!   [`Event::Decided`], [`Request::Write`] by [`Event::Written`],
//!   [`Request::Record`] by [`Event::Recorded`], [`Request::Apply`] by
//!   [`Event::Applied`] and [`Request::Act`] by [`Event::Acted`]. An item has
//!   at most one of them in flight at a time, so the writes of an item are
//!   serialised.
//! - Runs, named as the fleet names them: by their item and attempt
//!   (engine-model.md, 4.2 and 8). A [`Request::Start`] is ended by exactly one
//!   [`Event::Answered`] for its attempt, after an [`Event::Running`] once a
//!   worker has taken it, unless it is lost first. Answers also come for
//!   attempts this process never started (a worker reconnecting after the
//!   engine restarted, an answer sent again): the hub takes the answer of the
//!   attempt in flight, and says of every other one that it is
//!   [`Request::Stale`]. It cancels an attempt a worker runs that is not the
//!   one in flight. [`Request::Cancel`], [`Request::Relay`],
//!   [`Request::Keep`], [`Request::Acknowledge`], [`Request::Stale`] and
//!   [`Request::Left`] are notices.
//!
//! What the hub passes on without reading (the spec of a run that is due, the
//! writes of an engine action, a run's outcome and snapshot, an inbox event,
//! the plan's reason to hold an item) is named by tokens its parent issues,
//! and echoed. What it decides on is typed: phases, attempts, failure classes,
//! deadlines.

use temper_lib::{ReplyTo, Time, Token};

/// parent -> work
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// A call: take `item` in (engine-model.md, 4.6), from what its record
    /// said as it was read, at a start or later.
    Take { reply_to: ReplyTo, item: Item, read: Read },
    /// A call, from a person: stop the item's run. A run whose claim is being
    /// written is not started; one in flight is cancelled. Either way the
    /// item is held once the run has answered.
    Stop { reply_to: ReplyTo, item: Item },
    /// A call, from a person: release the held item. It is due again; an
    /// outcome held for a person's acceptance is applied again, and the
    /// person's decision, on the forge by then, is read with it.
    Release { reply_to: ReplyTo, item: Item },
    /// From the forge, through the parent: an inbox event (4.3), which the
    /// parent names `event`. It is relayed to the item's run if one is in
    /// flight. Otherwise it wakes the item at `wake`, as its step's wake rule
    /// says (5.4): now or at a later time, or, if `None`, not by itself.
    Inbox { item: Item, event: Token, wake: Option<Time> },
    /// From the fleet: the attempt runs on a worker, which took it, or which
    /// says on reconnecting that it still hosts it.
    Running { item: Item, attempt: u64 },
    /// From the fleet: the attempt's answer. Terminal for `Start`.
    Answered { item: Item, attempt: u64, answer: Answer },
    /// Terminal for `Due`: what the plan says is due for the item now.
    Decided { owner: Token, due: Due },
    /// Terminal for `Write`.
    Written { owner: Token, wrote: Wrote },
    /// Terminal for `Record`: the comment the outcome is now on the item as,
    /// or `None` if it could not be posted.
    Recorded { owner: Token, comment: Option<u64> },
    /// Terminal for `Apply`.
    Applied { owner: Token, applied: Applied },
    /// Terminal for `Act`.
    Acted { owner: Token, acted: Acted },
}

/// work -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The answer to `Take`: the item is taken in.
    Taken { to: ReplyTo },
    /// The answer to `Stop`: the run is not started, or is cancelled.
    Stopped { to: ReplyTo },
    /// The answer to `Release`: the item is released.
    Released { to: ReplyTo },
    /// The answer to a call the hub refuses, and why.
    Refused { to: ReplyTo, refusal: Refusal },
    /// To the plan: what is due for the item now (engine-model.md, 5.3).
    Due { owner: Token, item: Item },
    /// To the forge: write the hub's part of the item's record, composed with
    /// the others (the parent holds them). A record written `Done` is the
    /// last.
    Write { owner: Token, item: Item, lifecycle: Lifecycle },
    /// To the forge: post the outcome of the item's attempt `attempt`, which
    /// the parent keeps and named `outcome`, on the item, keyed by the attempt
    /// (4.4).
    Record { owner: Token, item: Item, attempt: u64, outcome: Token },
    /// Apply the outcome of the item's attempt `attempt`, posted as the
    /// comment `outcome`: read afresh what it touches, ask the plan what it
    /// writes and the rules about each write, and make them, keyed by the
    /// attempt. The record's update is not part of it: the hub writes it
    /// next, as the commit point.
    Apply { owner: Token, item: Item, attempt: u64, outcome: u64 },
    /// Make the writes of the engine action (4.5) the parent named `action`
    /// as it was decided, checked by the rules like an outcome's. The record's
    /// update follows, as for an outcome.
    Act { owner: Token, item: Item, action: Token },
    /// To the fleet: start the item's attempt `attempt`, on the run the parent
    /// named `run` as it was decided, its charter rendered now. The claim
    /// naming the attempt is written.
    Start { item: Item, attempt: u64, run: Token },
    /// To the fleet: cancel the item's attempt `attempt`. If it was started,
    /// its answer still comes.
    Cancel { item: Item, attempt: u64 },
    /// To the fleet: the inbox event the parent named `event`, for the item's
    /// attempt `attempt`.
    Relay { item: Item, attempt: u64, event: Token },
    /// To the store: keep the snapshot the item's attempt `attempt` parked
    /// with, which the parent named `snapshot`, in place of the item's last.
    Keep { item: Item, attempt: u64, snapshot: Token },
    /// To the fleet: the answer of the item's attempt `attempt` is on the
    /// forge, and its worker may forget it.
    Acknowledge { item: Item, attempt: u64 },
    /// To the fleet: an answer of the item's attempt `attempt`, which is not
    /// the attempt in flight (it was replaced, presumed lost, or answered
    /// already), is dropped. Its worker may forget it, and what the parent
    /// keeps of it goes.
    Stale { item: Item, attempt: u64 },
    /// The item is done, its record says so, and the hub no longer tracks it.
    Left { item: Item },
}

/// An item (seams: an issue or pull request), by its forge name: its
/// repository, by its place in the deployment's list, and the number the
/// forge gave it there.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Item {
    pub repository: u32,
    pub number: u64,
}

/// An item's record as it was read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// It has none yet: the item is new to the engine, handed in or just
    /// made. The hub writes its first.
    New,
    /// The hub's part of it.
    Record(Lifecycle),
    /// It does not decode: a person mangled it. The item is held for a
    /// person.
    Mangled,
}

/// The hub's part of an item's record (engine-model.md, 4.1): where it is in
/// its lifecycle, its attempts and its failures.
///
/// Due and running are never written: what is due is decided afresh after a
/// restart, and a claimed run is running once the fleet says so, which a
/// restart learns from the workers that reconnect.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Lifecycle {
    pub phase: Phase,
    /// The attempts made at the item's runs: the last claim's. It only grows,
    /// across restarts too, so a stale attempt stays fenced.
    pub attempts: u64,
    /// The item's failures since its last run that did not fail, or since a
    /// person released it.
    pub failures: Failures,
}

/// Where an item is in its lifecycle (4.2), as its record says.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    /// Waiting for what is due next.
    Waiting,
    /// Its last run parked: waiting for its next wake, its snapshot in the
    /// store.
    Parked,
    /// Its last run failed in this class: retried after a backoff.
    Retrying(Class),
    /// A run is claimed: the attempt the record's `attempts` says.
    Claimed,
    /// The outcome of the claimed attempt, posted as the comment `outcome`, is
    /// being applied: a restart resumes it.
    Applying { outcome: u64 },
    /// Held for a person, and why.
    Held(Hold),
    /// Its step is done, and the item closed.
    Done,
}

/// Why an item is held for a person. A release puts it back to due.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    /// Its plan holds it, for a reason the parent names: one it can read back
    /// from the record.
    Plan(Token),
    /// Its runs failed in this class more often than the class allows.
    Failures(Class),
    /// A person stopped its run.
    Stopped,
    /// The rules want a person's acceptance before the writes are made: of the
    /// outcome posted as this comment, or of the engine action due if `None`.
    Acceptance { outcome: Option<u64> },
    /// A write its outcome or action needed failed for good.
    Writes,
    /// Its record does not decode, or could not be written.
    Record,
}

/// A failure class (worker-model.md, 4.3, from the engine's side). Each is
/// retried a number of times of its own, after a backoff of its own.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Class {
    /// It may work later: the worker was busy or shutting down, or the forge
    /// could not be reached, or another run of the workstream held the
    /// checkout, while it was prepared.
    Transient,
    /// Something on the forge must change first (a repository, branch or
    /// commit missing, an identity refused), or the run was beyond what the
    /// worker takes.
    Permanent,
    /// The run failed, as it reported: its model, its budget, a policy, or
    /// stale.
    Run,
    /// Its agent failed: it could not be started, exited without answering,
    /// broke the channel's rules, or was stopped by the watchdog.
    Agent,
    /// It was presumed lost: its worker gone past the grace.
    Lost,
    /// Its outcome broke its step's spec, as the engine judged it.
    Invalid,
}

/// An item's failures, counted per class.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Failures {
    pub transient: u32,
    pub permanent: u32,
    pub run: u32,
    pub agent: u32,
    pub lost: u32,
    pub invalid: u32,
}

impl Failures {
    /// None yet.
    pub const NONE: Failures = Failures { transient: 0, permanent: 0, run: 0, agent: 0, lost: 0, invalid: 0 };

    /// The failures in `class`.
    #[must_use]
    pub const fn of(&self, class: Class) -> u32 {
        match class {
            Class::Transient => self.transient,
            Class::Permanent => self.permanent,
            Class::Run => self.run,
            Class::Agent => self.agent,
            Class::Lost => self.lost,
            Class::Invalid => self.invalid,
        }
    }

    /// These, with one more in `class`.
    #[must_use]
    pub const fn and(self, class: Class) -> Failures {
        let mut more = self;
        match class {
            Class::Transient => more.transient = more.transient.saturating_add(1),
            Class::Permanent => more.permanent = more.permanent.saturating_add(1),
            Class::Run => more.run = more.run.saturating_add(1),
            Class::Agent => more.agent = more.agent.saturating_add(1),
            Class::Lost => more.lost = more.lost.saturating_add(1),
            Class::Invalid => more.invalid = more.invalid.saturating_add(1),
        }
        more
    }
}

/// A run's answer, as the fleet reports it (worker-model.md, 4.2; engine-model.md, 8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// It ended with an outcome, which the parent keeps and names `outcome`.
    Ended { outcome: Token },
    /// It parked, handing over its snapshot if it had one, which the parent
    /// keeps and names `snapshot`.
    Parked { snapshot: Option<Token> },
    /// It failed, in `class`.
    Failed(Class),
    /// Its worker was gone past the grace, and the run is presumed lost. No
    /// worker answered, so there is nothing to acknowledge.
    Lost,
}

/// What is due for an item now (engine-model.md, 5.3), as the plan says.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Due {
    /// Nothing yet. The hub asks again at `until` (a timer, a batch's age, a
    /// wait's time), or when an inbox event wakes the item.
    Nothing { until: Option<Time> },
    /// A run, which the parent names `run`: the hub claims it, and starts it
    /// once the claim is written.
    Run { run: Token },
    /// An engine action, whose writes the parent names `action`.
    Act { action: Token },
    /// The step is done: the writes that finish it, which the parent names
    /// `action`, are made as an engine action's, and the record says done.
    Done { action: Token },
    /// Hold the item for a person, for the plan's reason, which the parent
    /// names `why`.
    Hold { why: Token },
}

/// How a write of the record went. The forge sub-model retries what fails
/// transiently, so a failure is for good.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Wrote {
    Done,
    Failed,
}

/// What applying an outcome came to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Applied {
    /// Its writes are made; the item goes on as `Then` says.
    Made(Then),
    /// The item moved on while the run worked: nothing of it is applied.
    Stale,
    /// It breaks its step's spec: nothing of it is applied, and its run
    /// failed.
    Invalid,
    /// The rules want a person's acceptance first: nothing of it is applied
    /// yet.
    Accepting,
    /// A write failed for good. What was made stays, keyed, and is found when
    /// it is applied again.
    Failed,
}

/// What an item does once an outcome's writes are made.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Then {
    /// It waits for what is due next.
    Wait,
    /// It is held for a person, for the plan's reason, which the parent names.
    Hold(Token),
}

/// What making an engine action's writes came to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Acted {
    /// Its writes are made.
    Made,
    /// The item moved on since the action was decided: nothing is made.
    Stale,
    /// The rules want a person's acceptance first: nothing is made yet.
    Accepting,
    /// A write failed for good. What was made stays, keyed.
    Failed,
}

/// Why a call is refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// The working set is full: the item waits at the entrance.
    Full,
    /// The item is taken in already.
    Taken,
    /// The item's record says it is done.
    Done,
    /// The item is not taken in.
    Unknown,
    /// The item has no run to stop: none is claimed or in flight.
    Idle,
    /// The item is not held.
    Unheld,
}
