//! Holds: a client's hold on a workspace, from its prepare to its release,
//! and the operations it runs there, one at a time (worker-domain.md, 4.2
//! steps 2, 7 and 8, and 5).
//!
//! A prepare that passes the entrance holds a workspace ([`crate::cache`]) and
//! readies it, one operation at a time. A workspace made again is made empty,
//! and each repository cloned into it. Then each repository is fetched at its
//! starting point and checked out. A base branch the forge does not have is
//! created there from the default branch, for a repository that may be
//! written, and only created: if another party created it meanwhile, it is
//! fetched again and the workspace starts from wherever that party put it.
//!
//! A push commits each writable repository's tree exactly as it is, on the
//! commit it was checked out at or last committed, and pushes the commit to
//! the repository's push branch, as a fast-forward: a branch that moved since
//! the workspace started is left where it is (`Moved`). A repository is
//! pushed only if it has a commit the branch has not had from this hold
//! since the start: otherwise it is `Unchanged`. A save does the same to the
//! saved-work branch, for every repository changed since the start. Pushing
//! is not atomic: each repository is pushed on its own, whatever came of the
//! one before. A push that failed in a way that may have landed all the same
//! (it ran out of time, broke, or lost the forge on the way) is verified: the
//! branch is fetched, and the push landed if the branch is at its commit.
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract: one terminal event per operation, and an operation's terminal
//! only one of its own.
//!
//! ```text
//! state       event                     next        requests
//! (none)      prepare, refused          (none)      prepared: refused
//!             prepare, admitted         Preparing   held, io: make or fetch
//! Preparing   done, more to do          Preparing   io: the next operation
//!             done, all checked out     Ready       prepared: ready
//!             done, failed              Unprepared  prepared: failed
//!             done, aborting            Unprepared  prepared: aborted
//!             done, releasing           Closed      prepared: aborted, released
//!             abort, release            Preparing   cancel, the first time asked
//! Pushing     done, more to do          Pushing     io: the next operation
//!             done, a push that may     Pushing     io: fetch its branch
//!               have landed
//!             done, all pushed          Ready       pushed or saved
//!             done, aborting            Ready       pushed or saved (the rest aborted)
//!             done, releasing           Closed      pushed or saved, released
//!             abort, release            Pushing     (it waits for the operation)
//! Preparing,  push, save                (same)      pushed or saved: refused, busy
//! Pushing
//! Ready       push, save                Pushing     io: commit
//!             ... nothing writable      Ready       pushed or saved: all unchanged
//!             ... beyond the limits     Ready       pushed or saved: refused, invalid
//!             abort                     Ready       (nothing is under way)
//!             release                   Closed      released
//! Unprepared  push, save                Unprepared  pushed or saved: refused, busy
//!             abort                     Unprepared  (nothing is under way)
//!             release                   Closed      released
//! Closed      push, save, abort,        Closed      (dropped: the handle is stale)
//!               release
//! ```
//!
//! "More to do" follows the steps above. A prepare asked to abort, or to
//! release, cancels its operation in flight, ends aborted once that has
//! ended, whatever its end, and starts nothing more. A push or a save is
//! never cancelled: so that a push that lands is reported landed, the
//! operation in flight runs to its end, which its deadline bounds, and a push
//! that may have landed is verified all the same; then the push or the save
//! ends, reporting the repositories not reached as aborted. Nothing touches
//! the workspace afterwards: the hold runs one operation at a time, and it
//! ends only once that operation has.
//!
//! An operation that broke, ran out of time or was cancelled may have left
//! its repository damaged (git was killed, or failed half way), so the
//! workspace is not trusted again: the next prepare makes it again.
//!
//! What a transition tells as facts is derived from the requests it made, in
//! one place ([`tell`]), and what the new state implies (a closed hold is
//! retired, and its workspace idle again) in another ([`follow`]).

use alloc::boxed::Box;
use core::mem;

use skein_lib::bytes::{copy_of, find};
use skein_lib::{Env, Id, List, Queue, Slab, Token};

use crate::boundary::{Failure, Landing, Message, Outcome, Prepared, Refusal, Repository, Request, Spec, Start};
use crate::cache::{Cache, Workspace, count};
use crate::domain::Domain;
use crate::facts::{Cached, Fact, Facts, Tally, Target};
use crate::git::{Commit, Done, Fault, Missing, Op, Place, Want};
use crate::limits::Limits;

#[derive(Debug)]
pub(crate) struct Hold {
    holding: Holding,
    state: State,
}

/// What a hold holds in every state.
#[derive(Debug)]
struct Holding {
    /// The client's token, echoed on every record back to it.
    client: Token,
    workspace: Id<Workspace>,
    /// The spec's repositories, in its order.
    repositories: Box<[Repository]>,
    /// Where each repository checked out so far stands, in the same order.
    tips: List<Tips>,
}

/// Where a repository stands: the commit it was checked out at, the commit
/// its working tree is on (that one, or the last it committed), and the
/// newest commit its push branch is known to have from this hold (the start,
/// until a push lands).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Tips {
    start: Commit,
    head: Commit,
    pushed: Commit,
}

#[derive(Debug)]
enum State {
    /// Preparing the workspace: `step` is in flight.
    Preparing { step: PrepareStep, asked: Asked },
    /// Prepared, with nothing under way.
    Ready,
    /// Its prepare failed or was aborted: it may only be released.
    Unprepared,
    /// Pushing or saving: `step` is in flight.
    Pushing { push: Push, step: PushStep, asked: Asked },
    /// Terminal: released, holding nothing.
    Closed,
}

/// The operation a preparing hold waits for. A repository is named by its
/// place in the spec.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum PrepareStep {
    Make,
    Clone {
        repository: u32,
    },
    /// Its starting point.
    Fetch {
        repository: u32,
    },
    /// The default branch, for a base branch the forge does not have.
    Default {
        repository: u32,
    },
    /// The base branch, at the default branch's `commit`.
    Create {
        repository: u32,
        commit: Commit,
    },
    /// The base branch, which another party created meanwhile.
    Refetch {
        repository: u32,
    },
    CheckOut {
        repository: u32,
        commit: Commit,
    },
}

/// A push or a save under way.
#[derive(Debug)]
struct Push {
    to: To,
    message: Message,
    /// What came of each repository so far, in the spec's order.
    landings: List<Landing>,
}

/// Where a push goes: each writable repository's push branch, or its
/// saved-work branch.
#[derive(Debug)]
enum To {
    Branches,
    Saved { branch: Box<[u8]> },
}

/// The operation a pushing hold waits for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum PushStep {
    Commit {
        repository: u32,
    },
    Push {
        repository: u32,
    },
    /// The branch a push that may have landed went to.
    Verify {
        repository: u32,
    },
}

/// What the client asked of the operation under way: nothing, to abort it, or
/// to release the hold once it has ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Asked {
    Nothing,
    Abort,
    Release,
}

/// What a prepare does once an operation has ended.
enum Next {
    Step(PrepareStep),
    Ready,
    Failed(Failure),
}

// Entry points, one per event: look the hold up, take its state out, run the
// cell's handler, then tell what happened and follow the new state
// ([`conclude`]).

pub(crate) fn prepare(domain: &mut Domain, env: &Env<Limits>, client: Token, spec: Spec, out: &mut Queue<Request>) {
    let mark = out.len();
    if !fits(&spec, &env.limits) {
        refuse_prepare(&mut domain.facts, client, Refusal::Invalid, out);
        return;
    }
    let Spec { key, repositories } = spec;
    let (workspace, cached) = match domain.cache.hold(key, &repositories) {
        Ok(holding) => holding,
        Err(refusal) => {
            refuse_prepare(&mut domain.facts, client, refusal, out);
            return;
        }
    };
    domain.facts.push(Fact::Held { client, cached });
    let tips = List::with_capacity(count(repositories.len()));
    let holding = Holding { client, workspace, repositories, tips };
    let hold = Hold { holding, state: State::Closed };
    let id = domain.holds.insert(hold).expect("room for a hold for each workspace, and as many released");
    out.push(Request::Held { client, hold: id.token() });
    let first = match cached {
        Cached::Reused => PrepareStep::Fetch { repository: 0 },
        Cached::Rebuilt | Cached::Evicted | Cached::New => PrepareStep::Make,
    };
    let hold = domain.holds.get_mut(id).expect("inserted above");
    hold.state = prepare_step(&hold.holding, id, first, env, out);
    conclude(domain, id, out, mark);
}

pub(crate) fn push(domain: &mut Domain, env: &Env<Limits>, hold: Token, message: Message, out: &mut Queue<Request>) {
    start(domain, env, hold, To::Branches, message, out);
}

pub(crate) fn save(
    domain: &mut Domain,
    env: &Env<Limits>,
    hold: Token,
    branch: Box<[u8]>,
    message: Message,
    out: &mut Queue<Request>,
) {
    start(domain, env, hold, To::Saved { branch }, message, out);
}

fn start(domain: &mut Domain, env: &Env<Limits>, hold: Token, to: To, message: Message, out: &mut Queue<Request>) {
    let mark = out.len();
    let Some(id) = addressed(&domain.holds, hold) else {
        return;
    };
    let hold = domain.holds.get_mut(id).expect("addressed above");
    let state = mem::replace(&mut hold.state, State::Closed);
    hold.state = match state {
        State::Ready => begin(&hold.holding, id, to, message, env, out),
        busy @ (State::Preparing { .. } | State::Pushing { .. } | State::Unprepared) => {
            answer(hold.holding.client, &to, Outcome::Refused { refusal: Refusal::Busy }, out);
            busy
        }
        State::Closed => unreachable!("an addressed hold has not been released"),
    };
    conclude(domain, id, out, mark);
}

pub(crate) fn abort(domain: &mut Domain, hold: Token, out: &mut Queue<Request>) {
    ask(domain, hold, Asked::Abort, out);
}

pub(crate) fn release(domain: &mut Domain, hold: Token, out: &mut Queue<Request>) {
    ask(domain, hold, Asked::Release, out);
}

/// The client asks to abort, or to release: a prepare's operation is
/// cancelled the first time it asks, a push's runs to its end, and a hold
/// with nothing under way is released at once.
fn ask(domain: &mut Domain, hold: Token, now: Asked, out: &mut Queue<Request>) {
    let mark = out.len();
    let Some(id) = addressed(&domain.holds, hold) else {
        return;
    };
    let hold = domain.holds.get_mut(id).expect("addressed above");
    let state = mem::replace(&mut hold.state, State::Closed);
    hold.state = match state {
        State::Preparing { step, asked } => State::Preparing { step, asked: cancelling(id, asked, now, out) },
        State::Pushing { push, step, asked } => State::Pushing { push, step, asked: outranking(asked, now) },
        idle @ (State::Ready | State::Unprepared) => match now {
            Asked::Release => released(hold.holding.client, out),
            Asked::Abort => idle,
            Asked::Nothing => unreachable!("a client asks to abort or to release"),
        },
        State::Closed => unreachable!("an addressed hold has not been released"),
    };
    conclude(domain, id, out, mark);
}

/// Terminal for an operation: what the hold does next.
pub(crate) fn done(domain: &mut Domain, env: &Env<Limits>, owner: Token, done: Done, out: &mut Queue<Request>) {
    let mark = out.len();
    let id = Id::from_token(owner);
    let hold = domain.holds.get_mut(id).expect("a hold lives until its operation has ended");
    domain.facts.push(Fact::Ended { client: hold.holding.client, done });
    if damages(done) {
        domain.cache.spoil(hold.holding.workspace);
    }
    let state = mem::replace(&mut hold.state, State::Closed);
    let holding = &mut hold.holding;
    hold.state = match state {
        State::Preparing { step, asked } => {
            let next = prepared(holding, &mut domain.cache, step, done);
            prepare_next(holding, id, next, asked, env, out)
        }
        State::Pushing { push, step, asked } => pushed(holding, id, push, step, asked, done, env, out),
        State::Ready | State::Unprepared | State::Closed => {
            unreachable!("an operation ends only while its hold prepares, pushes or saves")
        }
    };
    conclude(domain, id, out, mark);
}

/// The hold a client's handle names, or `None` if it has been released: the
/// handle travelled down while the `Released` travelled up, and is dropped
/// (5.2).
fn addressed(holds: &Slab<Hold>, hold: Token) -> Option<Id<Hold>> {
    let id = Id::from_token(hold);
    match &holds.get(id)?.state {
        State::Preparing { .. } | State::Ready | State::Unprepared | State::Pushing { .. } => Some(id),
        State::Closed => None,
    }
}

/// Applied after every transition: what it tells, and what the new state
/// implies. `mark` is where the requests the transition made begin in `out`.
fn conclude(domain: &mut Domain, id: Id<Hold>, out: &Queue<Request>, mark: u32) {
    let hold = domain.holds.get(id).expect("a hold lives until it is retired");
    tell(&mut domain.facts, hold.holding.client, out, mark);
    follow(&mut domain.holds, &mut domain.cache, id);
}

/// What a transition tells, derived from the requests it made: a fact for
/// each but `Held`, which the prepare tells with how the cache found its
/// workspace.
fn tell(facts: &mut Facts, client: Token, out: &Queue<Request>, mark: u32) {
    let made = usize::try_from(mark).expect("a u32 fits in a usize");
    for request in out.iter().skip(made) {
        let fact = match request {
            Request::Held { .. } => continue,
            Request::Prepared { client, prepared } => Fact::Prepared { client: *client, prepared: *prepared },
            Request::Pushed { client, outcome } => pushed_fact(*client, Target::Push, outcome),
            Request::Saved { client, outcome } => pushed_fact(*client, Target::Saved, outcome),
            Request::Released { client } => Fact::Released { client: *client },
            Request::Io { owner: _, op, deadline: _ } => Fact::Started { client, op: op.kind() },
            Request::Cancel { owner: _ } => Fact::Aborting { client },
        };
        facts.push(fact);
    }
}

fn pushed_fact(client: Token, to: Target, outcome: &Outcome) -> Fact {
    match outcome {
        Outcome::Pushed { landings } => Fact::Pushed { client, to, tally: tally(landings) },
        Outcome::Refused { refusal } => Fact::Refused { client, refusal: *refusal },
    }
}

/// What a hold's state implies, applied after every transition: a closed
/// hold is retired, and its workspace is idle again.
fn follow(holds: &mut Slab<Hold>, cache: &mut Cache, id: Id<Hold>) {
    let hold = holds.get(id).expect("a hold lives until it is retired");
    match hold.state {
        State::Closed => {
            cache.release(hold.holding.workspace);
            holds.retire(id);
        }
        State::Preparing { .. } | State::Ready | State::Unprepared | State::Pushing { .. } => {}
    }
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Preparing, done: where the prepare goes from the operation `step` that
/// ended so.
fn prepared(holding: &mut Holding, cache: &mut Cache, step: PrepareStep, done: Done) -> Next {
    let last = count(holding.repositories.len()).checked_sub(1).expect("a spec names a repository");
    match step {
        PrepareStep::Make => match done {
            Done::Succeeded => Next::Step(PrepareStep::Clone { repository: 0 }),
            Done::Failed { fault: _ } => Next::Failed(Failure::Transient),
            Done::Fetched { .. } | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
                unreachable!("io ends a make with its own terminals")
            }
        },
        PrepareStep::Clone { repository } => match done {
            Done::Succeeded if repository < last => Next::Step(PrepareStep::Clone { repository: after(repository) }),
            Done::Succeeded => {
                cache.cloned(holding.workspace, &holding.repositories);
                Next::Step(PrepareStep::Fetch { repository: 0 })
            }
            Done::Failed { fault } => Next::Failed(failure(repository, fault)),
            Done::Fetched { .. } | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
                unreachable!("io ends a clone with its own terminals")
            }
        },
        PrepareStep::Fetch { repository } => match done {
            Done::Fetched { commit } => Next::Step(PrepareStep::CheckOut { repository, commit }),
            Done::Failed { fault: Fault::Missing { missing: Missing::Branch } }
                if creates_base(holding, repository) =>
            {
                Next::Step(PrepareStep::Default { repository })
            }
            Done::Failed { fault } => Next::Failed(failure(repository, fault)),
            Done::Succeeded | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
                unreachable!("io ends a fetch with its own terminals")
            }
        },
        PrepareStep::Default { repository } => match done {
            Done::Fetched { commit } => Next::Step(PrepareStep::Create { repository, commit }),
            Done::Failed { fault } => Next::Failed(failure(repository, fault)),
            Done::Succeeded | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
                unreachable!("io ends a fetch with its own terminals")
            }
        },
        PrepareStep::Create { repository, commit } => match done {
            Done::Succeeded => Next::Step(PrepareStep::CheckOut { repository, commit }),
            Done::Exists => Next::Step(PrepareStep::Refetch { repository }),
            Done::Failed { fault } => Next::Failed(failure(repository, fault)),
            Done::Fetched { .. } | Done::Committed { .. } | Done::Unchanged | Done::Rejected => {
                unreachable!("io ends a creation with its own terminals")
            }
        },
        PrepareStep::Refetch { repository } => match done {
            Done::Fetched { commit } => Next::Step(PrepareStep::CheckOut { repository, commit }),
            // It was there a moment ago: whoever made it removed it since.
            Done::Failed { fault: Fault::Missing { .. } } => Next::Failed(Failure::Transient),
            Done::Failed { fault } => Next::Failed(failure(repository, fault)),
            Done::Succeeded | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
                unreachable!("io ends a fetch with its own terminals")
            }
        },
        PrepareStep::CheckOut { repository, commit } => match done {
            Done::Succeeded => {
                let tips = Tips { start: commit, head: commit, pushed: commit };
                holding.tips.push(tips).expect("room for each repository's tips");
                if repository < last {
                    Next::Step(PrepareStep::Fetch { repository: after(repository) })
                } else {
                    Next::Ready
                }
            }
            Done::Failed { fault: _ } => Next::Failed(Failure::Transient),
            Done::Fetched { .. } | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
                unreachable!("io ends a checkout with its own terminals")
            }
        },
    }
}

/// Preparing, done: the next operation, or the prepare's end, which an abort
/// or a release turns into aborted.
fn prepare_next(
    holding: &Holding,
    id: Id<Hold>,
    next: Next,
    asked: Asked,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let client = holding.client;
    match asked {
        Asked::Nothing => match next {
            Next::Step(step) => prepare_step(holding, id, step, env, out),
            Next::Ready => {
                let prepared = Prepared::Ready { workspace: holding.workspace.token() };
                out.push(Request::Prepared { client, prepared });
                State::Ready
            }
            Next::Failed(failure) => {
                out.push(Request::Prepared { client, prepared: Prepared::Failed { failure } });
                State::Unprepared
            }
        },
        Asked::Abort => {
            out.push(Request::Prepared { client, prepared: Prepared::Aborted });
            State::Unprepared
        }
        Asked::Release => {
            out.push(Request::Prepared { client, prepared: Prepared::Aborted });
            released(client, out)
        }
    }
}

/// Asks io for the prepare's operation `step`.
fn prepare_step(
    holding: &Holding,
    id: Id<Hold>,
    step: PrepareStep,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let workspace = holding.workspace.token();
    let op = match step {
        PrepareStep::Make => Op::Make { workspace },
        PrepareStep::Clone { repository } => {
            let repository = nth(holding, repository);
            let remote = copy_of(&repository.remote);
            let identity = copy_of(&repository.identity);
            Op::Clone { at: place(workspace, repository), remote, identity }
        }
        PrepareStep::Fetch { repository } | PrepareStep::Refetch { repository } => {
            let repository = nth(holding, repository);
            let want = match &repository.start {
                Start::Base { branch } | Start::Branch { branch } | Start::Saved { branch } => {
                    Want::Branch { branch: copy_of(branch) }
                }
                Start::Commit { commit } => Want::Commit { commit: *commit },
            };
            let remote = copy_of(&repository.remote);
            let identity = copy_of(&repository.identity);
            Op::Fetch { at: place(workspace, repository), remote, want, identity }
        }
        PrepareStep::Default { repository } => {
            let repository = nth(holding, repository);
            let remote = copy_of(&repository.remote);
            let identity = copy_of(&repository.identity);
            Op::Fetch { at: place(workspace, repository), remote, want: Want::Default, identity }
        }
        PrepareStep::Create { repository, commit } => {
            let repository = nth(holding, repository);
            let branch = match &repository.start {
                Start::Base { branch } => copy_of(branch),
                Start::Branch { .. } | Start::Commit { .. } | Start::Saved { .. } => {
                    unreachable!("only a base branch is created")
                }
            };
            let remote = copy_of(&repository.remote);
            let identity = copy_of(&repository.identity);
            Op::Create { at: place(workspace, repository), remote, branch, commit, identity }
        }
        PrepareStep::CheckOut { repository, commit } => {
            Op::CheckOut { at: place(workspace, nth(holding, repository)), commit }
        }
    };
    io(id, op, env, out);
    State::Preparing { step, asked: Asked::Nothing }
}

/// Ready, push or save: commits and pushes each writable repository in turn,
/// or refuses what does not fit the limits.
fn begin(
    holding: &Holding,
    id: Id<Hold>,
    to: To,
    message: Message,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    if !message_fits(&message, &to, &env.limits) {
        answer(holding.client, &to, Outcome::Refused { refusal: Refusal::Invalid }, out);
        return State::Ready;
    }
    let landings = List::with_capacity(count(holding.repositories.len()));
    push_from(holding, id, Push { to, message, landings }, 0, env, out)
}

/// Pushing, from the repository at `from`: commits the next writable one, a
/// read-only one being unchanged, or, with none left, answers.
fn push_from(
    holding: &Holding,
    id: Id<Hold>,
    mut push: Push,
    from: u32,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    for repository in from..count(holding.repositories.len()) {
        let spec = nth(holding, repository);
        if spec.push.is_none() {
            push.landings.push(Landing::Unchanged).expect("room for each repository's landing");
            continue;
        }
        let at = place(holding.workspace.token(), spec);
        let parent = tips(holding, repository).head;
        let title = copy_of(&push.message.title);
        let body = copy_of(&push.message.body);
        io(id, Op::Commit { at, parent, title, body, identity: copy_of(&spec.identity) }, env, out);
        return State::Pushing { push, step: PushStep::Commit { repository }, asked: Asked::Nothing };
    }
    answer(holding.client, &push.to, Outcome::Pushed { landings: push.landings.into_boxed() }, out);
    State::Ready
}

/// Pushing, done: what came of the repository in flight, then the next one,
/// or the end, which an abort or a release brings forward.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the state's parts and the event's")]
fn pushed(
    holding: &mut Holding,
    id: Id<Hold>,
    mut push: Push,
    step: PushStep,
    asked: Asked,
    done: Done,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let (repository, landing) = match step {
        PushStep::Commit { repository } => match asked {
            Asked::Nothing => match committed(holding, repository, done) {
                Ok(()) => match unpushed(holding, &push.to, repository) {
                    Some(commit) => return push_commit(holding, id, push, repository, commit, env, out),
                    None => (repository, Landing::Unchanged),
                },
                Err(fault) => (repository, landing(fault)),
            },
            // Committed or not, nothing is pushed.
            Asked::Abort | Asked::Release => match committed(holding, repository, done) {
                Ok(()) | Err(Fault::Cancelled) => (repository, Landing::Aborted),
                Err(fault) => (repository, landing(fault)),
            },
        },
        PushStep::Push { repository } => match pushed_to(holding, &push.to, repository, done) {
            Some(landing) => (repository, landing),
            None => return verify(holding, id, push, repository, asked, env, out),
        },
        PushStep::Verify { repository } => (repository, verified(holding, &push.to, repository, done)),
    };
    push.landings.push(landing).expect("room for each repository's landing");
    match asked {
        Asked::Nothing => push_from(holding, id, push, after(repository), env, out),
        Asked::Abort | Asked::Release => end_push(holding, push, asked, out),
    }
}

/// Pushing, a commit done: the repository's new head, if it committed, or
/// why it failed.
fn committed(holding: &mut Holding, repository: u32, done: Done) -> Result<(), Fault> {
    match done {
        Done::Committed { commit } => {
            tips_mut(holding, repository).head = commit;
            Ok(())
        }
        Done::Unchanged => Ok(()),
        Done::Failed { fault } => Err(fault),
        Done::Succeeded | Done::Fetched { .. } | Done::Exists | Done::Rejected => {
            unreachable!("io ends a commit with its own terminals")
        }
    }
}

/// The head of the repository at `repository`, if it is a commit the branch
/// `to` names has not had from this hold: one since the last push that landed
/// on its push branch, or since the start for its saved-work branch.
fn unpushed(holding: &Holding, to: &To, repository: u32) -> Option<Commit> {
    let tips = tips(holding, repository);
    let base = match to {
        To::Branches => tips.pushed,
        To::Saved { .. } => tips.start,
    };
    if tips.head == base { None } else { Some(tips.head) }
}

/// Asks io to push `commit`, the head of the repository at `repository`, to
/// the branch `push` goes to.
fn push_commit(
    holding: &Holding,
    id: Id<Hold>,
    push: Push,
    repository: u32,
    commit: Commit,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let spec = nth(holding, repository);
    let branch = branch_of(spec, &push.to);
    let at = place(holding.workspace.token(), spec);
    let remote = copy_of(&spec.remote);
    let identity = copy_of(&spec.identity);
    io(id, Op::Push { at, remote, commit, branch, identity }, env, out);
    State::Pushing { push, step: PushStep::Push { repository }, asked: Asked::Nothing }
}

/// Pushing, a push done: what came of it, or `None` if it may have landed
/// all the same, to be verified. A commit that landed on its push branch is
/// the base of the next push.
fn pushed_to(holding: &mut Holding, to: &To, repository: u32, done: Done) -> Option<Landing> {
    match done {
        Done::Succeeded => Some(land(holding, to, repository)),
        Done::Rejected => Some(Landing::Moved),
        Done::Failed { fault: Fault::TimedOut | Fault::Broken | Fault::Unreachable | Fault::Cancelled } => None,
        Done::Failed { fault: fault @ (Fault::Missing { .. } | Fault::Refused) } => Some(landing(fault)),
        Done::Fetched { .. } | Done::Committed { .. } | Done::Unchanged | Done::Exists => {
            unreachable!("io ends a push with its own terminals")
        }
    }
}

/// Asks io for the branch a push that may have landed went to, whether the
/// client asked to abort or not: one more operation, bounded as any.
fn verify(
    holding: &Holding,
    id: Id<Hold>,
    push: Push,
    repository: u32,
    asked: Asked,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> State {
    let spec = nth(holding, repository);
    let want = Want::Branch { branch: branch_of(spec, &push.to) };
    let at = place(holding.workspace.token(), spec);
    let remote = copy_of(&spec.remote);
    let identity = copy_of(&spec.identity);
    io(id, Op::Fetch { at, remote, want, identity }, env, out);
    State::Pushing { push, step: PushStep::Verify { repository }, asked }
}

/// Pushing, a verification done: the push landed if its branch is at the
/// commit it pushed.
fn verified(holding: &mut Holding, to: &To, repository: u32, done: Done) -> Landing {
    match done {
        Done::Fetched { commit } if commit == tips(holding, repository).head => land(holding, to, repository),
        Done::Fetched { .. } | Done::Failed { .. } => Landing::Failed,
        Done::Succeeded | Done::Committed { .. } | Done::Unchanged | Done::Exists | Done::Rejected => {
            unreachable!("io ends a fetch with its own terminals")
        }
    }
}

/// The head of the repository at `repository` landed on the branch `to`
/// names.
fn land(holding: &mut Holding, to: &To, repository: u32) -> Landing {
    let tips = tips_mut(holding, repository);
    match to {
        To::Branches => tips.pushed = tips.head,
        To::Saved { .. } => {}
    }
    Landing::Landed { commit: tips.head }
}

/// Pushing, aborted or released once the operation in flight has ended: the
/// repositories not reached are aborted, and the client is answered.
fn end_push(holding: &Holding, mut push: Push, asked: Asked, out: &mut Queue<Request>) -> State {
    for _ in push.landings.len()..count(holding.repositories.len()) {
        push.landings.push(Landing::Aborted).expect("room for each repository's landing");
    }
    answer(holding.client, &push.to, Outcome::Pushed { landings: push.landings.into_boxed() }, out);
    match asked {
        Asked::Abort => State::Ready,
        Asked::Release => released(holding.client, out),
        Asked::Nothing => unreachable!("a push ends early only when asked to"),
    }
}

/// What the client has asked of a prepare once it asks `now`, having asked
/// `before`: the operation in flight is cancelled the first time.
fn cancelling(id: Id<Hold>, before: Asked, now: Asked, out: &mut Queue<Request>) -> Asked {
    match before {
        Asked::Nothing => out.push(Request::Cancel { owner: id.token() }),
        Asked::Abort | Asked::Release => {}
    }
    outranking(before, now)
}

/// What the client has asked once it asks `now`, having asked `before`: a
/// release outranks an abort.
const fn outranking(before: Asked, now: Asked) -> Asked {
    match before {
        Asked::Nothing | Asked::Abort => now,
        Asked::Release => Asked::Release,
    }
}

fn released(client: Token, out: &mut Queue<Request>) -> State {
    out.push(Request::Released { client });
    State::Closed
}

fn answer(client: Token, to: &To, outcome: Outcome, out: &mut Queue<Request>) {
    match to {
        To::Branches => out.push(Request::Pushed { client, outcome }),
        To::Saved { .. } => out.push(Request::Saved { client, outcome }),
    }
}

fn refuse_prepare(facts: &mut Facts, client: Token, refusal: Refusal, out: &mut Queue<Request>) {
    facts.push(Fact::Refused { client, refusal });
    out.push(Request::Prepared { client, prepared: Prepared::Refused { refusal } });
}

fn io(id: Id<Hold>, op: Op, env: &Env<Limits>, out: &mut Queue<Request>) {
    let timeout = if op.is_remote() { env.limits.remote_timeout } else { env.limits.local_timeout };
    out.push(Request::Io { owner: id.token(), op, deadline: env.now.saturating_add(timeout) });
}

/// Whether an operation that ended so may have left its repository damaged:
/// git was killed, or failed on the worker's side.
const fn damages(done: Done) -> bool {
    match done {
        Done::Failed { fault: Fault::Broken | Fault::TimedOut | Fault::Cancelled } => true,
        Done::Failed { fault: Fault::Missing { .. } | Fault::Refused | Fault::Unreachable }
        | Done::Succeeded
        | Done::Fetched { .. }
        | Done::Committed { .. }
        | Done::Unchanged
        | Done::Exists
        | Done::Rejected => false,
    }
}

/// Why a prepare failed, from an operation's fault for the repository at
/// `repository`.
const fn failure(repository: u32, fault: Fault) -> Failure {
    match fault {
        Fault::Missing { missing } => Failure::Missing { repository, missing },
        Fault::Refused => Failure::Refused { repository },
        Fault::Unreachable | Fault::Broken | Fault::TimedOut | Fault::Cancelled => Failure::Transient,
    }
}

/// What came of a repository whose commit or push failed so, for good.
const fn landing(fault: Fault) -> Landing {
    match fault {
        Fault::Refused => Landing::Refused,
        Fault::Missing { .. } | Fault::Unreachable | Fault::Broken | Fault::TimedOut | Fault::Cancelled => {
            Landing::Failed
        }
    }
}

fn tally(landings: &[Landing]) -> Tally {
    let mut tally = Tally { landed: 0, moved: 0, failed: 0, refused: 0, unchanged: 0, aborted: 0 };
    for landing in landings {
        let counted = match landing {
            Landing::Landed { .. } => &mut tally.landed,
            Landing::Moved => &mut tally.moved,
            Landing::Failed => &mut tally.failed,
            Landing::Refused => &mut tally.refused,
            Landing::Unchanged => &mut tally.unchanged,
            Landing::Aborted => &mut tally.aborted,
        };
        *counted = counted.saturating_add(1);
    }
    tally
}

/// Whether `spec` fits the limits: a workstream, and between one and
/// `Limits::repositories` repositories, each in a directory of its own named
/// by one safe path component, and every name, remote, branch and identity at
/// most `Limits::name_bytes` and none empty.
fn fits(spec: &Spec, limits: &Limits) -> bool {
    let Ok(repositories) = u32::try_from(spec.repositories.len()) else {
        return false;
    };
    if !named(&spec.key, limits) || !(1..=limits.repositories).contains(&repositories) {
        return false;
    }
    for (place, repository) in spec.repositories.iter().enumerate() {
        let start = match &repository.start {
            Start::Base { branch } | Start::Branch { branch } | Start::Saved { branch } => named(branch, limits),
            Start::Commit { .. } => true,
        };
        let push = match &repository.push {
            Some(branch) => named(branch, limits),
            None => true,
        };
        let directory = named(&repository.name, limits) && component(&repository.name);
        let reached = named(&repository.remote, limits) && named(&repository.identity, limits);
        if !directory || !reached || !start || !push {
            return false;
        }
        for earlier in spec.repositories.iter().take(place) {
            if earlier.name == repository.name {
                return false;
            }
        }
    }
    true
}

/// Whether `name` is one safe path component: not `.` or `..`, without `/`
/// or NUL, and not a git directory (`.git` in any ASCII case).
fn component(name: &[u8]) -> bool {
    let dots = name == b"." || name == b"..";
    let separated = find(name, b"/").is_some() || find(name, b"\0").is_some();
    !dots && !separated && !name.eq_ignore_ascii_case(b".git")
}

/// Whether a push's or a save's message, and a save's branch, fit the limits:
/// a title, and at most `Limits::message_bytes` with the body.
fn message_fits(message: &Message, to: &To, limits: &Limits) -> bool {
    let branch = match to {
        To::Branches => true,
        To::Saved { branch } => named(branch, limits),
    };
    let Some(bytes) = message.title.len().checked_add(message.body.len()) else {
        return false;
    };
    let within = match u32::try_from(bytes) {
        Ok(bytes) => bytes <= limits.message_bytes,
        Err(_) => false,
    };
    branch && within && !message.title.is_empty()
}

fn named(bytes: &[u8], limits: &Limits) -> bool {
    let within = match u32::try_from(bytes.len()) {
        Ok(len) => len <= limits.name_bytes,
        Err(_) => false,
    };
    within && !bytes.is_empty()
}

/// Whether the repository at `repository` starts from a base branch that is
/// created if the forge does not have it: only for one that may be written.
fn creates_base(holding: &Holding, repository: u32) -> bool {
    let spec = nth(holding, repository);
    let base = match spec.start {
        Start::Base { .. } => true,
        Start::Branch { .. } | Start::Commit { .. } | Start::Saved { .. } => false,
    };
    base && spec.push.is_some()
}

fn nth(holding: &Holding, repository: u32) -> &Repository {
    let index = usize::try_from(repository).expect("a u32 fits in a usize");
    holding.repositories.get(index).expect("a repository of the spec")
}

fn tips(holding: &Holding, repository: u32) -> Tips {
    *holding.tips.get(repository).expect("a repository checked out")
}

fn tips_mut(holding: &mut Holding, repository: u32) -> &mut Tips {
    holding.tips.get_mut(repository).expect("a repository checked out")
}

fn place(workspace: Token, repository: &Repository) -> Place {
    Place { workspace, repository: copy_of(&repository.name) }
}

/// The branch a push to `to` of `repository` goes to.
fn branch_of(repository: &Repository, to: &To) -> Box<[u8]> {
    match to {
        To::Branches => copy_of(repository.push.as_deref().expect("only a writable repository is pushed")),
        To::Saved { branch } => copy_of(branch),
    }
}

fn after(repository: u32) -> u32 {
    repository.checked_add(1).expect("fewer repositories than a u32 counts")
}
