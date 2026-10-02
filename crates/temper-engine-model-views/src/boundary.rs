//! The records that cross the boundary with the views' parent, the engine's
//! top-level model (programming-style.md, 4.5), which routes them to and from
//! the people watching on the web and the engine's store. The views sub-model
//! defines them; its parent depends on it.
//!
//! Notices in: [`Event::Started`], [`Event::Reported`] and
//! [`Event::Finished`] follow a run, from its assignment to its end;
//! [`Event::Phase`] tells an item's phase as it changes. None is answered.
//!
//! Watches. An [`Event::Watch`] is answered at once by exactly one
//! [`Request::Watching`] or [`Request::Refused`]; a watch that is watching
//! is delivered the snapshot it came with first, and ends with exactly one
//! [`Request::Ended`], after a [`Event::Unwatch`] or, for a run's watch,
//! once the run has finished and the watcher has had what waited for it. In
//! between, each [`Request::Deliver`] is ended by exactly one
//! [`Event::Delivered`], and a watcher has at most one delivery in flight,
//! so its token names its delivery too; its watch ends only once its
//! delivery has. The parent bounds each delivery's time and says when the
//! stream did not take it; what it held is told as missed with the next.
//! The parent names each watch by a token of its own, and uses it for no
//! other watch until the watch's end.
//!
//! The store. A [`Request::Append`] is ended by exactly one
//! [`Event::Appended`], an [`Request::Expire`] by exactly one
//! [`Event::Expired`], each echoing its `owner`. The parent's contract: the
//! store takes its operations in the order they are asked for, the parent
//! bounds each one's time and says when it failed, and a failed append may
//! have kept some of its records, those it kept in order from the first.
//! Times are the views' clock, `env.now`, which starts again with the
//! engine: the store maps them to wall time, both in what it keeps and in
//! what an expire names, so that what was kept before a restart is
//! forgotten after it.
//!
//! A terminal that names nothing in flight, a duplicate included, is
//! dropped.

use alloc::boxed::Box;

use temper_lib::{Time, Token};

/// What a run reports, as the parent names it (agent-model.md, section 7):
/// the views pass its content on and keep it, and never parse it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    /// A session's text, as it is written.
    Text,
    /// The run's progress: admitted, turns, yielding, ended.
    Progress,
    /// An LLM call started, retried or finished.
    Call,
    /// A tool or a check started or finished.
    Tool,
    /// The usage of a turn.
    Usage,
}

/// What a trace keeps of a report.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Capture {
    /// No record at all.
    Nothing,
    /// Its kind, when it was reported and its size, not its content.
    Shape,
    /// All of it.
    Content,
}

/// A run's capture policy, which the engine sends with each assignment: what
/// its trace keeps of each kind of report. It bounds the trace, not what
/// the run's watchers see.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Policy {
    pub text: Capture,
    pub progress: Capture,
    pub calls: Capture,
    pub tools: Capture,
    pub usage: Capture,
}

impl Policy {
    /// What the policy keeps of a report of `kind`.
    #[must_use]
    pub const fn capture(&self, kind: Kind) -> Capture {
        match kind {
            Kind::Text => self.text,
            Kind::Progress => self.progress,
            Kind::Call => self.calls,
            Kind::Tool => self.tools,
            Kind::Usage => self.usage,
        }
    }
}

/// What a watcher watches: a run, by the parent's token for it; an item, by
/// the parent's token for it, which takes in the reports of each of its runs
/// and its phase changes; or the board of the repository at that place in
/// the deployment's list, which takes in its items' phase changes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Subject {
    Run(Token),
    Item(Token),
    Board(u32),
}

/// A piece of a watcher's stream.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Chunk {
    /// What the subject was when the watch began, as the parent gave it.
    Snapshot { at: Time, content: Box<[u8]> },
    /// What the attempt `attempt` of the run `run` reported at `at`.
    Report { run: Token, attempt: Token, kind: Kind, at: Time, content: Box<[u8]> },
    /// The item `item` went to `phase` at `at`.
    Phase { item: Token, phase: u32, at: Time },
}

/// A report, as a trace keeps it: its run and attempt, its kind, when it was
/// reported, how many bytes it had, and those bytes if the run's policy keeps
/// them.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Record {
    pub run: Token,
    pub attempt: Token,
    pub kind: Kind,
    pub at: Time,
    pub size: u32,
    pub content: Option<Box<[u8]>>,
}

/// Why a watch was refused at the entrance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// No room for one more watcher.
    Busy,
    /// The run is not followed: it has finished, or never started.
    Unknown,
    /// The run is not followed: it started when there was no room for it.
    Unfollowed,
    /// The snapshot is past the limits.
    Oversized,
}

/// Why a watch ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum End {
    /// The person stopped watching.
    Unwatched,
    /// The watched run finished, and the watcher has had what waited for it.
    Finished,
}

/// parent -> views
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The attempt `attempt` of the run `run` was assigned, for the item
    /// `item`, under `policy`: what it reports from now on is streamed and
    /// traced. A run started again, for a new attempt, takes the attempt,
    /// item and policy given last.
    Started { run: Token, attempt: Token, item: Token, policy: Policy },
    /// The run `run` reported this, of `kind`. A report of a run that is not
    /// followed, or past the limits, is dropped, and its watchers told they
    /// missed it.
    Reported { run: Token, kind: Kind, content: Box<[u8]> },
    /// The run `run` ended: nothing more comes of it.
    Finished { run: Token },
    /// The item `item`, of the repository at `repository` in the deployment's
    /// list, went to `phase`, a code the parent defines.
    Phase { item: Token, repository: u32, phase: u32 },
    /// A person watches `subject`, under the parent's token `watcher`, from
    /// `snapshot`, what the parent knows of it now, which is delivered first.
    /// Answered by exactly one `Watching` or `Refused`.
    Watch { watcher: Token, subject: Subject, snapshot: Box<[u8]> },
    /// The person stopped watching, or their stream closed. Answered by the
    /// watch's `Ended`, unless it has ended already.
    Unwatch { watcher: Token },
    /// Terminal for `Deliver`: the watcher's stream has taken it, or did not
    /// in time.
    Delivered { watcher: Token, done: bool },
    /// Terminal for `Append`: the records were kept, or not (some of them
    /// may have been).
    Appended { owner: Token, done: bool },
    /// Terminal for `Expire`: what it named is gone, or not (some of it may
    /// be).
    Expired { owner: Token, done: bool },
}

/// views -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The `Watch` of `watcher` is taken: its stream follows.
    Watching { watcher: Token },
    /// The `Watch` of `watcher` was refused at the entrance.
    Refused { watcher: Token, refusal: Refusal },
    /// To the stream of `watcher`: the newest of the chunks that came since
    /// its last delivery, in order, and how many it missed since the last
    /// delivery it took: dropped from its backlog before these, held by a
    /// delivery it did not take, or of reports dropped.
    Deliver { watcher: Token, missed: u64, chunks: Box<[Chunk]> },
    /// The watch of `watcher` is over: nothing more comes to it, and its
    /// token is the parent's again.
    Ended { watcher: Token, end: End },
    /// Keep these records in the store, in this order.
    Append { owner: Token, records: Box<[Record]> },
    /// Forget every record reported before `before`.
    Expire { owner: Token, before: Time },
}
