//! The records that cross the boundary with the protocol layer
//! (programming-model.md, 4.4). The domain defines them; the protocol crate
//! depends on it.
//!
//! Four peers are behind it, and each record names which by its variant:
//!
//! - The forge, through the forge child domain (engine-domain.md, section 12).
//!   A [`Request::Forge`] is a call of the forge child domain's
//!   (`temper_engine_domain_forge::api`), with the payload it names by token
//!   filled in, typed: the engine's record, an outcome posted, a note's page
//!   ([`Payload`]). It is ended by exactly one [`Event::Answered`] echoing its
//!   `call`, which carries what the protocol layer decoded of the engine's own
//!   payloads inside the answer ([`Decoded`]): the protocol encodes them into
//!   comments and wiki pages, and decodes them back. A webhook is an
//!   [`Event::Hint`], never relied on.
//! - Workers, over their channels (worker-domain.md, sections 2 and 4), in
//!   the fleet's terms, a run named by its item and an attempt by its count,
//!   which the protocol layer packs into the channel's tokens. A worker's
//!   channel is in contact from its [`Event::Hello`] until its
//!   [`Event::Lost`]. A [`Request::Assign`] is a call, answered by exactly
//!   one [`Event::Answer`] under the run's and the attempt's names, which the
//!   worker sends again after every hello until [`Request::Acknowledge`]; a
//!   refusal goes once. [`Request::Inbound`], [`Request::Cancel`] and
//!   [`Request::Relayed`] are notices; an [`Event::Relay`] is a run's call,
//!   answered by at most one [`Request::Relayed`]: on the channel it came
//!   on, at once and unserved, if its attempt is not the live claim or
//!   there is no room for it; else once served, on the channel hosting the
//!   attempt then, unless the attempt is fenced off or out of contact by
//!   then. [`Event::Bounced`] and [`Event::Told`] are notices.
//!   [`Request::Refuse`] closes a worker's channel: turned away at its
//!   hello, or, later, when the engine has no room to carry an answer it
//!   sent, which it sends again after its next hello.
//! - People, through the web (engine-domain.md, sections 2, 6 and 11): an
//!   [`Event::Ask`] is a call, answered by exactly one [`Request::Reply`]
//!   echoing its `reply_to`, at once or once what it asked is done. A watch
//!   taken ([`Reply::Watching`]) is a stream named by its `watcher`: each
//!   [`Request::Deliver`] to it is ended by exactly one [`Event::Delivered`],
//!   one in flight at a time, and the watch by exactly one
//!   [`Request::Ended`], after an [`Event::Unwatch`] or once the run it
//!   watches has finished.
//! - The engine's store: a [`Request::Store`] is ended by exactly one
//!   [`Event::Stored`] echoing its `owner`. Snapshots are kept by item, one
//!   per item, a put replacing the last; traces are appended and expired,
//!   in the order they are asked for.
//!
//! Tokens of different families may be equal: the variant routes a terminal
//! to whoever asked.

use alloc::boxed::Box;

use skein_lib::{ReplyTo, Time, Token};
use temper_engine_domain_brief::Section;
use temper_engine_domain_fleet::Bounce;
use temper_engine_domain_forge::{News, Read, api};
use temper_engine_domain_notes::{Change, Entry, Noted, Page, Recall, Scope};
use temper_engine_domain_plan::{self as plan, Budget, Decided, Finish, Grants, Why};
use temper_engine_domain_rules::Permission;
use temper_engine_domain_views::{End, Kind, Policy};
use temper_engine_domain_work::Lifecycle;

/// An issue or pull request (seams: "Names"), by its forge name: its
/// repository, by its place in the deployment's list, and the number the
/// forge gave it there. The hub's names are the engine's.
pub use temper_engine_domain_work::Item;

/// protocol -> domain
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    Refreshed {
        account: u32,
        generation: u64,
        valid: skein_lib::Duration,
    },
    RefreshFailed {
        account: u32,
        generation: u64,
        failure: crate::accounts::Failure,
    },
    Rejected {
        channel: Token,
        item: Item,
        attempt: u64,
        account: u32,
        generation: u64,
    },
    Exhausted {
        channel: Token,
        item: Item,
        attempt: u64,
        account: u32,
        retry_after: skein_lib::Duration,
    },
    /// Terminal for `Forge`: what the forge answered, or why it did not, and
    /// the engine's payloads the protocol layer found inside the answer and
    /// decoded. A record or a page that does not decode is not among them:
    /// the answer marks it mangled.
    Answered {
        call: Token,
        /// HTTP requests actually served, including cache misses and failures.
        cost: u32,
        result: Result<api::Answer, api::Error>,
        decoded: Box<[Decoded]>,
    },
    /// A webhook: something changed in `repository`, about the item `item`,
    /// the commit `commit` or the branch `branch`, if it names one.
    Hint {
        repository: u32,
        item: Option<u64>,
        commit: Option<[u8; 32]>,
        branch: Option<Box<[u8]>>,
        by: Option<u64>,
        wiki: bool,
    },
    /// From a worker, first on its channel: its slots, the workstreams it
    /// holds checkouts for, and the runs it hosts or holds answers of.
    Hello {
        channel: Token,
        hello: Hello,
    },
    /// From the protocol: the channel of a worker closed.
    Lost {
        channel: Token,
    },
    /// From a worker, the answer to an `Assign` of the item's attempt
    /// `attempt`: sent at once, and again after every hello until it is
    /// acknowledged.
    Answer {
        channel: Token,
        item: Item,
        attempt: u64,
        answer: Answer,
    },
    /// From a worker, on its channel `channel`, a call of the item's run, at
    /// its attempt `attempt`, which the worker names `call`.
    Relay {
        channel: Token,
        item: Item,
        attempt: u64,
        call: Token,
        body: Call,
    },
    /// From a worker: an inbound event for the item's attempt `attempt` was
    /// not passed on, for `bounce`.
    Bounced {
        channel: Token,
        item: Item,
        attempt: u64,
        name: Token,
        bounce: Bounce,
    },
    /// From a worker: a fact the item's run told at its attempt `attempt`
    /// (agent-domain.md, section 7), of `kind`, as the protocol layer decodes
    /// it.
    Told {
        channel: Token,
        item: Item,
        attempt: u64,
        kind: Kind,
        content: Box<[u8]>,
    },
    /// From a person on the web, a call: answered by exactly one `Reply`.
    Ask {
        reply_to: ReplyTo,
        person: u64,
        ask: Ask,
    },
    /// The person watching as `watcher` stopped, or their stream closed.
    Unwatch {
        watcher: Token,
    },
    /// Terminal for `Deliver`: the stream of `watcher` took it, or did not in
    /// the time the protocol layer gives a delivery.
    Delivered {
        watcher: Token,
        done: bool,
    },
    /// Terminal for `Store`.
    Stored {
        owner: Token,
        stored: Stored,
    },
}

/// domain -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Only Refresh, Keep, or Cancel; availability/grants are routed inside the domain.
    Account {
        request: crate::accounts::Request,
    },
    Grant {
        channel: Token,
        item: Item,
        attempt: u64,
        grant: crate::accounts::Grant,
    },
    /// To the forge: do `op` on `repository`, its payload `payload`, which
    /// the op names by token, filled in. Ended by one `Answered`.
    Forge {
        call: Token,
        repository: u32,
        op: api::Op,
        payload: Option<Payload>,
    },
    /// To a worker, a call: host the run of `assignment`.
    Assign {
        channel: Token,
        assignment: Assignment,
    },
    /// To a worker: an inbound event for the item's attempt it hosts.
    Inbound {
        channel: Token,
        item: Item,
        attempt: u64,
        name: Token,
        event: Inbound,
    },
    /// To a worker: cancel the item's attempt `attempt`.
    Cancel {
        channel: Token,
        item: Item,
        attempt: u64,
    },
    /// To a worker: the answer to the call `call` of the item's attempt.
    Relayed {
        channel: Token,
        item: Item,
        attempt: u64,
        call: Token,
        served: Served,
    },
    /// To a worker: the engine has the answer of the item's attempt
    /// `attempt`, durably or not wanted, and the worker forgets it.
    Acknowledge {
        channel: Token,
        item: Item,
        attempt: u64,
    },
    /// About a worker: no room for it, its hello is beyond the limits, or
    /// there is no room to carry an answer it sent. Close its channel; it
    /// dials again later, and sends its answers again.
    Refuse {
        channel: Token,
    },
    /// To a person, the one answer to an `Ask`.
    Reply {
        to: ReplyTo,
        reply: Reply,
    },
    /// To the stream of `watcher`: `missed` chunks were dropped since its last
    /// delivery, right before these. Ended by one `Delivered`.
    Deliver {
        watcher: Token,
        missed: u64,
        chunks: Box<[Chunk]>,
    },
    /// The watch of `watcher` is over: nothing more comes to it.
    Ended {
        watcher: Token,
        end: End,
    },
    /// To the store. Ended by one `Stored`.
    Store {
        owner: Token,
        op: Store,
    },
}

/// The engine's record of an item (engine-domain.md, 4.1; seams: "The
/// record"), as the domain composes it from its owners' parts, less the
/// forge child domain's (the inbox position, and the nonce of the write), which
/// travels in the op. The protocol layer encodes it inside the record's
/// comment, and decodes it back.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Record {
    /// The hub's part: where the item is in its lifecycle.
    pub lifecycle: Lifecycle,
    /// The plan's part: the step the item carries, how far it has come, and
    /// on a goal's item, its plan.
    pub step: plan::Record,
    /// The top level's part: what the forge cannot hold, and what decisions
    /// read that no other part says.
    pub relations: Relations,
}

/// What the top level keeps in an item's record (seams: "The record").
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Relations {
    /// When the engine took the item in, on the engine's clock.
    pub created: Time,
    /// The goal whose plan the item's step is in.
    pub goal: Option<Item>,
    /// The item whose outcome made this one, if one did.
    pub parent: Option<Item>,
    /// The pull request carrying the item's change, once it is open.
    pub pull: Option<u64>,
    /// The head of the item's branch, once a run has pushed to it.
    pub branch: Option<[u8; 32]>,
    /// The items of the steps it comes after, in the step's order.
    pub dependencies: Box<[Related]>,
    /// The items of the steps it added; on a goal's item, of its plan's.
    pub children: Box<[Related]>,
    /// A person's latest decision on the step, and the permission they held
    /// if they accepted.
    pub decision: Option<Decided>,
    pub accepted: Option<Permission>,
    /// The outcome the decision is on, by its comment: one held for a
    /// person's acceptance. `None` for a decision on the step itself (a wait
    /// for one, a gate, a run or an action the rules held).
    pub accepting: Option<u64>,
    /// The permission the rules want of whoever accepts what the item is
    /// held for: kept in the record, as a person may accept it after a
    /// restart.
    pub wants: Option<Permission>,
    /// Whether the store holds the snapshot of a run that parked.
    pub snapshot: bool,
    /// The tokens its runs have spent.
    pub spent: u64,
}

/// An item related to another, by its step's name, and when it was done.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Related {
    pub name: Box<[u8]>,
    pub item: Item,
    pub done: Option<Time>,
}

/// An outcome as it is posted on its item (4.4): the attempt that gave it,
/// what it says, and the head its run pushed, if it pushed one.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Posted {
    pub attempt: u64,
    pub outcome: Outcome,
    pub head: Option<[u8; 32]>,
}

/// A run's outcome (seams: "Outcomes and writes"), as the protocol layer
/// decodes the worker's opaque bytes: what the plan applies, and the words
/// the outcome's comment carries.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// A change, pushed: its message. What landed is the answer's work.
    Change {
        message: Box<[u8]>,
    },
    /// A verdict on the head the run was given to review.
    Verdict {
        verdict: plan::Verdict,
        text: Box<[u8]>,
    },
    Report {
        text: Box<[u8]>,
    },
    /// A plan proposed, whose goal is the item.
    Plan {
        plan: plan::Plan,
        text: Box<[u8]>,
    },
    /// Steps added to the goal's plan.
    Steps {
        steps: Box<[plan::Step]>,
        text: Box<[u8]>,
    },
    /// Tasks to make, each carrying a step.
    Tasks {
        tasks: Box<[plan::Step]>,
        text: Box<[u8]>,
    },
    /// A session's reply.
    Reply {
        text: Box<[u8]>,
    },
    /// A session's last turn.
    Finished {
        text: Box<[u8]>,
    },
    /// A supervising session releases the step of its goal named `step`.
    Release {
        step: Box<[u8]>,
        text: Box<[u8]>,
    },
    /// The run asks for a decision it cannot make.
    Escalation {
        text: Box<[u8]>,
    },
}

/// A payload a forge op names by token, filled in as the call goes out.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Payload {
    /// The engine's record of the item the op is about.
    Record(Box<Record>),
    /// An outcome, posted on its item.
    Outcome(Box<Posted>),
    /// A note's page, written to the wiki.
    Page(Box<Page>),
}

/// An engine payload the protocol layer found inside a forge answer, and
/// decoded.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Decoded {
    /// The comment `comment` holds the engine's record.
    Record { comment: u64, record: Box<Record> },
    /// The comment `comment` holds an outcome posted.
    Outcome { comment: u64, posted: Box<Posted> },
    /// The wiki page `name` holds a note's page.
    Page { name: Box<[u8]>, page: Box<Page> },
}

/// What a worker says first on every channel (engine-domain.md, section 8).
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Hello {
    pub slots: u32,
    pub workstreams: Box<[Box<[u8]>]>,
    /// The runs it hosts, and those whose answers it holds.
    pub hosting: Box<[Hosted]>,
}

/// A run a worker hosts, or whose answer it holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hosted {
    pub item: Item,
    pub attempt: u64,
    pub phase: temper_engine_domain_fleet::Phase,
}

/// A run's answer (worker-domain.md, 4.3), as the protocol layer decodes it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// Refused at the entrance, every slot taken or the worker shutting
    /// down: placed again.
    Busy,
    /// Refused at the entrance: the assignment does not fit the worker.
    Invalid,
    /// It ended with its outcome.
    Ended { outcome: Outcome, work: Work },
    /// It parked, with its snapshot if it had one.
    Parked { snapshot: Option<Box<[u8]>>, work: Work },
    /// It failed.
    Failed { failure: Failure, work: Work },
}

/// What a run left on the forge: the last commit it landed in each
/// repository it pushed to, by the deployment's index for it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Work {
    pub landed: Box<[Landed]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Landed {
    pub repository: u32,
    pub commit: [u8; 32],
}

/// Why a run failed, as the engine classes it (worker-domain.md, 4.3): its
/// workspace could not be prepared for a while (or the worker cancelled it
/// shutting down), or until the forge changes; the run failed, as it
/// reported; its agent failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    Transient,
    Permanent,
    Run,
    Agent,
}

/// An assignment (worker-domain.md, 4.1): the run, its workspace, where its
/// unfinished work is saved, its charter and the snapshot it resumes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Assignment {
    pub grants: Box<[crate::accounts::Grant]>,
    pub item: Item,
    pub attempt: u64,
    pub workspace: Workspace,
    pub save: Option<Box<[u8]>>,
    pub charter: Charter,
    pub snapshot: Option<Box<[u8]>>,
}

/// The checkout a run works in: named by its workstream, so the runs of an
/// item find it cached.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Workspace {
    pub key: Box<[u8]>,
    pub repositories: Box<[Checkout]>,
}

/// A repository of a workspace, by the deployment's index for it: the
/// protocol layer maps it to its name, remote and the identity the worker
/// uses for it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Checkout {
    pub repository: u32,
    pub start: Start,
    /// The branch a change is pushed to, if the run may push.
    pub push: Option<Box<[u8]>>,
}

/// Where a repository's checkout starts.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Start {
    Base { branch: Box<[u8]> },
    Branch { branch: Box<[u8]> },
    Commit { commit: [u8; 32] },
    Saved { branch: Box<[u8]> },
}

/// What a run is given (agent-domain.md, 4.1; seams: "Charters and briefs"),
/// in the engine's terms: the protocol layer encodes it into the bytes the
/// worker carries.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Charter {
    pub why: Why,
    /// The brief, as typed sections.
    pub brief: Box<[Section]>,
    /// The guidance the plan writes, apart from the brief.
    pub instructions: Box<[u8]>,
    pub grants: Grants,
    /// What it may finish with: its outcome spec.
    pub finish: Finish,
    pub budget: Budget,
    /// The models it runs with, as configured.
    pub models: Box<[Model]>,
    /// What its trace keeps of what it reports.
    pub policy: Policy,
}

/// An LLM selection, understood by the engine so it can name its account.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Model {
    pub endpoint: u32,
    pub model: Box<[u8]>,
    pub max_tokens: u32,
}

/// An inbound event for a live run (engine-domain.md, 4.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Inbound {
    /// News of the item's own, as the forge child domain tells it.
    News(News),
    /// The item itself, an item it comes after, or one it added, is done.
    Finished { item: Item },
    /// A step of the goal the run's item supervises is held: its run
    /// escalated, or it waited on the forge too long (engine-domain.md,
    /// section 6).
    Held { item: Item },
    /// A person decided on the run's item: accepted what it waits for, or
    /// rejected it.
    Decided { accepted: bool },
}

/// A run's call (seams: "Workers"): a forge read, a `recall`, or an outlet.
#[derive(PartialEq, Eq, Debug)]
pub enum Call {
    Read(Read),
    Recall(Recall),
    /// The `note` outlet: an entry made, revised or removed.
    Note {
        scope: Scope,
        name: Box<[u8]>,
        change: Change,
    },
    /// A comment on the item: a session's reply, or a word to people.
    Comment {
        text: Box<[u8]>,
    },
    /// An escalation: the run asks its goal's session, or a person, for a
    /// decision, and goes on.
    Escalate {
        text: Box<[u8]>,
    },
}

/// The answer to a run's call.
#[derive(PartialEq, Eq, Debug)]
pub enum Served {
    Read(api::Answer),
    Recalled {
        entries: Box<[Entry]>,
        failed: u32,
    },
    Noted(Noted),
    /// The comment posted.
    Posted {
        comment: u64,
    },
    Unserved(Unserved),
}

/// Why a run's call was not served.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Unserved {
    /// The run's grants do not allow it, or it names what is not in its
    /// scope.
    Ungranted,
    /// No room for it now: the run may call again.
    Busy,
    /// Beyond the limits.
    Invalid,
    /// The rules refuse it, or want a person's acceptance first.
    Refused,
    /// The forge did not do it.
    Failed,
}

/// A person's call through the web (seams: "People").
#[derive(PartialEq, Eq, Debug)]
pub enum Ask {
    /// Open a session in `repository`: an issue titled `title`, keyed by the
    /// request, its first message the person's.
    Open {
        repository: u32,
        key: Box<[u8]>,
        title: Box<[u8]>,
        message: Box<[u8]>,
    },
    /// Write `message` on the item, for the person, keyed by the request.
    Message {
        item: Item,
        key: Box<[u8]>,
        message: Box<[u8]>,
    },
    /// Accept, or reject, what the held item waits for a person to accept.
    Accept {
        item: Item,
    },
    Reject {
        item: Item,
    },
    /// Stop the item's run.
    Stop {
        item: Item,
    },
    /// Release the held item.
    Release {
        item: Item,
    },
    /// Watch a run, an item or a board: a stream follows.
    Watch {
        subject: Watched,
    },
}

/// What a person watches (engine-domain.md, section 11).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Watched {
    /// The item's run, until it finishes.
    Run { item: Item },
    /// The item: its runs' reports, and its phases.
    Item { item: Item },
    /// The phases of the items of the repository at this place in the
    /// deployment's list.
    Board { repository: u32 },
}

/// A piece of a watcher's stream.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Chunk {
    /// What the subject was when the watch began: a line per item, its
    /// number and its phase.
    Snapshot { at: Time, content: Box<[u8]> },
    /// What the item's run reported at `at`, at its attempt `attempt`.
    Report { item: Item, attempt: u64, kind: Kind, at: Time, content: Box<[u8]> },
    /// The item went to `phase` at `at`.
    Phase { item: Item, phase: Phase, at: Time },
}

/// An item's phase (engine-domain.md, 4.2), as people see it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Phase {
    Waiting,
    Due,
    Claimed,
    Running,
    Applying,
    Held,
    Done,
}

impl Phase {
    /// Its code, which the views carry.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Phase::Waiting => 0,
            Phase::Due => 1,
            Phase::Claimed => 2,
            Phase::Running => 3,
            Phase::Applying => 4,
            Phase::Held => 5,
            Phase::Done => 6,
        }
    }

    /// The phase of `code`, if it is one.
    #[must_use]
    pub const fn of(code: u32) -> Option<Phase> {
        match code {
            0 => Some(Phase::Waiting),
            1 => Some(Phase::Due),
            2 => Some(Phase::Claimed),
            3 => Some(Phase::Running),
            4 => Some(Phase::Applying),
            5 => Some(Phase::Held),
            6 => Some(Phase::Done),
            _ => None,
        }
    }

    /// Its name, as a snapshot writes it.
    #[must_use]
    pub const fn name(self) -> &'static [u8] {
        match self {
            Phase::Waiting => b"waiting",
            Phase::Due => b"due",
            Phase::Claimed => b"claimed",
            Phase::Running => b"running",
            Phase::Applying => b"applying",
            Phase::Held => b"held",
            Phase::Done => b"done",
        }
    }
}

/// A report, as a trace keeps it: its run's item and attempt, its kind,
/// when it was reported, its size, and its bytes if the run's capture
/// policy keeps them.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Trace {
    pub item: Item,
    pub attempt: u64,
    pub kind: Kind,
    pub at: Time,
    pub size: u32,
    pub content: Option<Box<[u8]>>,
}

/// The answer to an `Ask`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reply {
    Done,
    /// The session is open, on the item.
    Opened {
        item: Item,
    },
    /// The watch is taken: its stream is `watcher`.
    Watching {
        watcher: Token,
    },
    Refused(Refusal),
}

/// Why a person's call was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// No room for it now.
    Busy,
    /// The item is not one the engine tracks, or the repository not the
    /// deployment's.
    Unknown,
    /// The person's permission on the repository does not allow it.
    Unpermitted,
    /// The item has no run to stop.
    Idle,
    /// The item is not held.
    Unheld,
    /// The forge did not take it.
    Failed,
    /// The run is not followed: it has finished, or never started, or
    /// started when the views had no room for it.
    Unfollowed,
}

/// An operation on the engine's store.
#[derive(PartialEq, Eq, Debug)]
pub enum Store {
    /// Keep `snapshot` as the item's, in place of its last.
    Put {
        item: Item,
        snapshot: Box<[u8]>,
    },
    Get {
        item: Item,
    },
    Drop {
        item: Item,
    },
    /// Keep these traces, in this order.
    Append {
        traces: Box<[Trace]>,
    },
    /// Forget every trace reported before `before`.
    Expire {
        before: Time,
    },
}

/// How an operation on the store ended.
#[derive(PartialEq, Eq, Debug)]
pub enum Stored {
    Done,
    /// The item's snapshot, if the store has one.
    Got(Option<Box<[u8]>>),
    Failed,
}
