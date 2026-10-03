//! The records that cross the boundary with the host's parent, the worker's
//! root domain (4.5). The host defines them; its parent depends on it.
//!
//! The host has three faces, all through its parent:
//!
//! - The engine's, which the parent routes to and from the engine link. An
//!   [`Event::Assign`] is a call, answered by exactly one [`Request::Answer`].
//!   Everything else the engine sends names the run and the attempt, and is
//!   dropped unless that attempt is hosted (worker-domain.md, section 2:
//!   attempts are fenced). A [`Request::Relay`] is answered by at most one
//!   [`Event::Relayed`]: the host stops waiting for it once the run is no
//!   longer live, having answered the run's call itself, and drops an answer
//!   that comes after. [`Event::Report`] is answered by one
//!   [`Request::Hosting`].
//! - The workspace's, which the parent translates to and from the checkout
//!   child domain's vocabulary. A [`Request::Prepare`] is ended by exactly one
//!   [`Event::Prepared`] or [`Event::Unprepared`], also after a
//!   [`Request::Abort`], which abandons it as its run is cancelled; a
//!   [`Request::Push`] by one [`Event::Pushed`], and a [`Request::Save`] by
//!   one [`Event::Saved`]. [`Request::Release`] is a notice. A prepared workspace is the run's until
//!   it is released, and the host releases it only once nothing of the run is
//!   running and nothing it asked of the workspace is in flight.
//! - The agent's, which the parent translates to and from the agent
//!   child domain's vocabulary. A [`Request::Start`] is ended by exactly one
//!   [`Event::Gone`], once the agent and everything it started are gone,
//!   after an [`Event::Started`] unless the agent could not be started. In
//!   between, the agent's run makes host calls ([`Event::Called`]), each
//!   answered by exactly one [`Request::Reply`], also once the run has
//!   withdrawn it ([`Event::Withdrawn`]); yields, says how it finishes, and may
//!   be faulted by the agent child domain, which also bounces an inbound event
//!   it could not take ([`Event::Bounced`]), for the host to tell the engine.
//!   [`Request::Deliver`] and [`Request::Stop`] are notices: an agent that has
//!   gone drops them, and a stop shows in the start's `Gone`.
//!
//! A request's `owner` is the host's token for what asked: the hosted run for
//! a prepare, a start or a save, the run's call for a push. It is echoed on
//! the terminal. The host addresses a workspace and an agent by the tokens
//! their terminals gave it, `workspace` and `agent`.

use alloc::boxed::Box;

use skein_lib::{ReplyTo, Token};

/// parent -> host
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// From the engine, a call: host the run of `assignment`, and answer once
    /// it has ended. The attempt hosted, assigned again, is dropped: its one
    /// answer is the hosted run's.
    Assign { reply_to: ReplyTo, assignment: Assignment },
    /// From the engine: an inbound event for the run `run`'s attempt
    /// `attempt`, which goes down to the run as it arrives.
    Inbound { run: Token, attempt: Token, event: Box<[u8]> },
    /// From the engine: cancel the run `run`'s attempt `attempt`.
    Cancel { run: Token, attempt: Token },
    /// From the engine: the answer to the relayed call `call` of the run
    /// `run`'s attempt `attempt`.
    Relayed { run: Token, attempt: Token, call: Token, answer: Box<[u8]> },
    /// From the top level: cancel every run hosted now, for `reason` (lost
    /// contact with the engine past its grace, or shutdown). The runs are
    /// cancelled one at a time, from the ready list. A worker shutting down
    /// admits no more runs: assignments after it are refused as busy.
    CancelAll { reason: Reason },
    /// From the top level: say what the host hosts, for the engine to keep or
    /// cancel on reconnecting.
    Report,
    /// From the top level: `answers` the host made have yet to be
    /// acknowledged by the engine. Each keeps its run's slot until it is, so
    /// that the engine, which frees a slot once it has the answer, never finds
    /// a worker with more runs than slots.
    Unacknowledged { answers: u32 },
    /// Terminal for `Prepare`: the workspace is ready, and `workspace` names it
    /// from now on.
    Prepared { owner: Token, workspace: Token },
    /// Terminal for `Prepare`: the workspace could not be prepared, and
    /// nothing of it is held.
    Unprepared { owner: Token, failure: Preparation, detail: Box<[u8]> },
    /// The agent started, and `agent` names it from now on.
    Started { owner: Token, agent: Token },
    /// A host call of the run, which the host answers with one `Reply`.
    /// `call` is the agent's name for it.
    Called { owner: Token, call: Token, ask: Ask },
    /// The run withdrew its host call `call`, its own deadline for it having
    /// passed. A relayed call is answered at once as withdrawn; a push goes
    /// on, and is answered with how it went. A call answered already is not
    /// in flight, and nothing happens.
    Withdrawn { owner: Token, call: Token },
    /// The agent could not take an inbound event for the run, for `bounce`.
    Bounced { owner: Token, bounce: Bounce },
    /// The run yielded: it waits for its next inbound event.
    Yielded { owner: Token },
    /// The run said how it finishes. Its agent exits next. Said as it winds
    /// down after a stop, it is still the run's answer.
    Finished { owner: Token, finish: Finish },
    /// The agent child domain is stopping the agent for `fault`.
    Faulted { owner: Token, fault: AgentFailure },
    /// Terminal for `Start`: the agent and everything it started are gone.
    /// `detail` is for operators, such as the tail of its error output.
    Gone { owner: Token, detail: Box<[u8]> },
    /// Terminal for `Push`: what became of each repository of the workspace,
    /// in the assignment's order.
    Pushed { owner: Token, push: Box<[Landing]> },
    /// Terminal for `Save`: what became of each repository, as for a push.
    Saved { owner: Token, save: Box<[Landing]> },
}

/// host -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// To the engine, the answer to an `Assign`: exactly one per assignment.
    Answer { to: ReplyTo, run: Token, attempt: Token, answer: Answer },
    /// To the engine: a host call of the run `run`'s attempt `attempt`, which
    /// the host names `call`, relayed as it is.
    Relay { run: Token, attempt: Token, call: Token, body: Box<[u8]> },
    /// To the engine: an inbound event for the run `run`'s attempt `attempt`
    /// was not passed on, for `bounce`.
    Bounced { run: Token, attempt: Token, bounce: Bounce },
    /// To the top level, the answer to `Report`: every run hosted, in the
    /// order of the engine's names for them.
    Hosting { runs: Box<[Hosting]> },
    /// Prepare `workspace` for the hosted run `owner`.
    Prepare { owner: Token, workspace: Workspace },
    /// Abandon the prepare in flight for `owner`, whose run is cancelled. Its
    /// terminal still comes: `Unprepared`, or `Prepared` if the prepare won
    /// the race, and then the workspace is released.
    Abort { owner: Token },
    /// Start an agent on `charter`, resumed from `snapshot` if there is one,
    /// in the prepared workspace `workspace`, which says where the
    /// repositories sit.
    Start { owner: Token, workspace: Token, charter: Box<[u8]>, snapshot: Option<Box<[u8]>> },
    /// An inbound event for the run of the agent `agent`.
    Deliver { agent: Token, event: Box<[u8]> },
    /// The one answer to the host call `call` of the agent `agent`.
    Reply { agent: Token, call: Token, reply: Reply },
    /// Stop the agent `agent`: cancel its run, then kill what is left of it
    /// past the grace. Its start's `Gone` comes once it has all gone. Sent
    /// whenever its run leaves live, also once the run has said how it
    /// finishes or the agent was faulted, when it changes nothing: the agent
    /// child domain winds the agent down then anyway.
    Stop { agent: Token },
    /// Commit what the writable repositories of `workspace` hold, with
    /// `message`, and push it: the run's call `owner`.
    Push { owner: Token, workspace: Token, message: Box<[u8]> },
    /// Commit what the writable repositories of `workspace` hold, and push it
    /// to the saved-work branch `branch`.
    Save { owner: Token, workspace: Token, branch: Box<[u8]> },
    /// The run is done with `workspace`: it goes back to the cache.
    Release { workspace: Token },
}

/// What the engine gives the worker for one run (worker-domain.md, 4.1).
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Assignment {
    /// The engine's names for the run and for this attempt at it.
    pub run: Token,
    pub attempt: Token,
    pub workspace: Workspace,
    /// The saved-work branch, if unfinished work is saved.
    pub save: Option<Box<[u8]>>,
    /// What the agent's run is given, passed through.
    pub charter: Box<[u8]>,
    /// The state of a parked run to resume from, passed through.
    pub snapshot: Option<Box<[u8]>>,
}

/// The checkout a run works in. The host checks its bounds and passes it on.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Workspace {
    /// Names the checkout, so later runs of the same work find it cached.
    pub key: Box<[u8]>,
    pub repositories: Box<[Repository]>,
}

/// A repository of a workspace. Names are compared byte for byte.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    /// The directory it sits in, side by side with the workspace's others
    /// (worker-domain.md, section 5), and what the agent calls it: one path
    /// component, unique within the workspace.
    pub name: Box<[u8]>,
    /// The forge's address of the repository, never interpreted: the
    /// protocol layer maps it to a URL.
    pub remote: Box<[u8]>,
    pub start: Start,
    pub access: Access,
    /// Who the worker is to the forge for this repository, to read it and to
    /// push to it, and who commits to it: a name the protocol layer maps to
    /// credentials and an author.
    pub identity: Box<[u8]>,
}

/// Where a repository's checkout starts.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Start {
    /// A base branch, made from the default branch if it does not exist yet.
    Base {
        branch: Box<[u8]>,
    },
    Branch {
        branch: Box<[u8]>,
    },
    /// A commit, by its object id: the full id in 32 bytes, as the protocol
    /// layer makes it of git's (a SHA-256 id as it is, a SHA-1 id followed by
    /// twelve zero bytes), compared and never looked inside.
    Commit {
        commit: [u8; 32],
    },
    /// Saved work, on the saved-work branch `branch`.
    Saved {
        branch: Box<[u8]>,
    },
}

/// Whether a repository may be written.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Access {
    ReadOnly,
    /// A change is pushed to `push`.
    Writable {
        push: Box<[u8]>,
    },
}

/// A host call of a run.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    /// Commit what the checkout holds, with `message`, and push it. The host
    /// serves it.
    Push { message: Box<[u8]> },
    /// A forge read or an outlet, relayed to the engine as it is.
    Relay { body: Box<[u8]> },
}

/// The answer to a host call.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Reply {
    /// The engine's answer to a relayed call, as it is.
    Relayed { answer: Box<[u8]> },
    /// How the push went.
    Pushed(Push),
    /// The run is cancelled or ending: nothing was done. A push in flight as
    /// the run leaves live is waited for, and answered with how it went.
    Unavailable,
    /// The run withdrew the relayed call: nothing more is done for it, and
    /// the engine's answer, if one comes, is dropped.
    Withdrawn,
    /// The run has as many calls in flight as it may, or a push in flight
    /// already: nothing was done. Calls answered within the loop's current
    /// iteration keep their slots until its reclaim point, so a busy call may
    /// find room in the next.
    Busy,
}

/// How a push went, as the run is told: done only if every repository with a
/// change landed it. A push the forge refused failed, as the run sees it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Push {
    Done,
    /// A branch moved since the run started: no change of the run can land
    /// there.
    Moved,
    /// A push failed, and none moved.
    Failed,
    /// No repository had a change.
    Nothing,
}

/// What became of one repository in a push or a save.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Landing {
    /// Its change is on the branch, as `commit`, by its object id (as
    /// [`Start::Commit`]'s).
    Landed { commit: [u8; 32] },
    /// The branch moved since the run started: nothing was pushed.
    Moved,
    /// The push failed, and did not land: a retry may succeed.
    Failed,
    /// The forge refused the push: the identity's permissions, a protected
    /// branch, a hook. A retry will not do better.
    Refused,
    /// It had no change.
    Unchanged,
}

/// How the run finishes, as it says.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Finish {
    /// It ended with `outcome`, its declared outcome, passed through.
    Ended { outcome: Box<[u8]> },
    /// It parked, handing over `snapshot` if it has one.
    Parked { snapshot: Option<Box<[u8]>> },
    /// It failed, for `failure`.
    Failed { failure: RunFailure },
}

/// Why an inbound event was not passed on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Bounce {
    /// It holds more bytes than the limits allow.
    TooLarge,
    /// The run is not live yet, and holds as many events as it may.
    Full,
    /// The run is ending.
    Ending,
}

/// A hosted run, for the reconnect report.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hosting {
    pub run: Token,
    pub attempt: Token,
    pub phase: Phase,
}

/// Where a hosted run is in its lifecycle (worker-domain.md, 4.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    Preparing,
    Starting,
    /// Its agent is at work.
    Active,
    /// It yielded, and waits for its next inbound event.
    Waiting,
    /// How it ends is decided: it is stopping, saving, or waiting for what is
    /// in flight to settle, and answers next.
    Ending,
}

/// The answer to an `Assign`.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// Refused at the entrance: nothing was done.
    Refused(Refusal),
    /// The run ended with `outcome`.
    Ended { outcome: Box<[u8]>, work: Work },
    /// The run parked, with its snapshot if it had one.
    Parked { snapshot: Option<Box<[u8]>>, work: Work },
    /// The run failed, for `failure`; `detail` is for operators, never for an
    /// LLM.
    Failed { failure: Failure, detail: Box<[u8]>, work: Work },
}

/// What a run left on the forge: the repositories its pushes landed in, by
/// their place in the assignment, ascending, each with the last commit landed
/// there (the head a pull request is to show); and its save, if one was made,
/// each repository's outcome in the assignment's order.
#[derive(PartialEq, Eq, Hash, Debug)]
pub struct Work {
    pub landed: Box<[Landed]>,
    pub saved: Option<Box<[Landing]>>,
}

/// A repository a run's pushes landed in: its place in the assignment, and
/// the last commit landed there.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Landed {
    pub repository: u32,
    pub commit: [u8; 32],
}

/// Why an assignment was refused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// Every slot is taken (by a run hosted, or one whose answer the engine
    /// has yet to acknowledge), the run is hosted already under another
    /// attempt that has not answered yet, or the worker is shutting down. A
    /// later retry, on this worker or another, may find room.
    Busy,
    /// The assignment does not fit the limits.
    Invalid(Invalid),
}

/// What about an assignment does not fit the limits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Invalid {
    /// The workspace lists no repository, or more than a run may hold.
    Repositories,
    /// The workspace lists one repository name twice.
    Duplicate,
    /// A workstream key, repository name, remote, branch or identity is empty
    /// or longer than a name may be; or a repository name is not one
    /// safe path component: `.`, `..`, `.git` in any case, or holding `/` or
    /// NUL.
    Name,
    /// The charter holds more bytes than a run may.
    Charter,
    /// The snapshot holds more bytes than a run may.
    Snapshot,
}

/// Why a hosted run failed (worker-domain.md, 4.3): what the engine acts on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// Its workspace could not be prepared.
    Unprepared(Preparation),
    /// The run failed, as it reports it.
    Run(RunFailure),
    /// Its agent failed.
    Agent(AgentFailure),
    /// It was cancelled.
    Cancelled(Reason),
}

/// Why a workspace could not be prepared.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Preparation {
    /// The forge could not be reached, something failed on the worker's
    /// side, an operation ran out of time, or the workspace is held by
    /// another run of its workstream: a retry may work.
    Transient,
    /// The forge does not have what the assignment names for the repository
    /// at `repository`, its place in the assignment: permanent until someone
    /// makes it.
    Missing { repository: u32, missing: Missing },
    /// The forge refused the identity of the repository at `repository`:
    /// permanent until the identity's credentials or permissions change.
    Refused { repository: u32 },
}

/// What the forge does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Missing {
    Repository,
    /// A branch to start from, or the default branch a base branch is to be
    /// made from.
    Branch,
    Commit,
}

/// Why a run failed, as it reports it (agent-domain.md, 4.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RunFailure {
    /// The LLM could not do the work.
    Model,
    /// The run's budget ran out.
    Budget,
    /// The LLM did not keep to the run's rules.
    Policy,
    /// The run was cancelled.
    Cancelled,
    /// The branch a change is pushed to moved since the run started.
    Stale,
}

/// How an agent failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AgentFailure {
    /// It could not be started.
    Unstarted,
    /// It exited without saying how its run finishes.
    Exited,
    /// It broke the channel's rules, or said more than the limits allow.
    Rules,
    /// The watchdog stopped it: no progress.
    NoProgress,
    /// The watchdog stopped it: past its wall time.
    WallTime,
}

/// Why a run was cancelled.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reason {
    /// The engine cancelled it.
    Engine,
    /// The worker lost contact with the engine for longer than the grace.
    Contact,
    /// The worker is shutting down.
    Shutdown,
}
