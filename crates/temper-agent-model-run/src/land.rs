//! Landing a change (agent-model.md, 4.4): when the LLM finishes with a
//! change, the run runs the checks of the repositories that have them, one
//! after another, and then asks the worker to push. The checks were looked
//! for when the run started, and only when the outcome spec has a change pass
//! them (`prepare`).
//!
//! Checked is pushed: nothing may write to the checkout from the first check
//! to the push. The finish call holds its conversation, which runs a write
//! alone, and the run has no other conversation yet. (A run with sub-agents
//! will have to close or hold the writable ones before it checks.)
//!
//! A failing check, or a push that finds the branch moved or fails, goes back
//! to the LLM as feedback, and it carries on. The call is its conversation's:
//! a withdraw stops what is in flight, and the call returns once that has
//! settled. A change is accepted only once it is pushed, and only while its
//! run may still finish.
//!
//! ```text
//! state      event                        next       emits
//! -          checks to run                Checking   check, checking
//!            no checks                    Pushing    push
//! Checking   checked, passed, more        Checking   check, checking
//!            checked, passed, the last    Pushing    push
//!            checked, passed, run ending  Closed     return: cancelled
//!            checked, failed              Closed     return: checks failed
//!            withdraw                     Aborting   abort
//! Aborting   checked, aborted             Closed     return: cancelled
//! Pushing    pushed                       Closed     return: accepted, moved or unpushed
//!            withdraw                     Unpushing  cancel the host call
//! Unpushing  pushed                       Closed     return: as for Pushing
//!            host cancelled               Closed     return: cancelled
//! ```
//!
//! Every other cell is unreachable: one terminal per request, a withdraw at
//! most once per call, and a cancel's terminal only after the cancel.

use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Queue, Slab, Token};

use crate::boundary::{Exit, Place, Push, Ran, Request, Returned};
use crate::charter::Repository;
use crate::limits::Limits;
use crate::outcome::Change;
use crate::prepare;
use crate::run::{Conversation, Run};

/// A finish call landing a change.
#[derive(Debug)]
pub(crate) struct Call {
    run: Id<Run>,
    conversation: Id<Conversation>,
    /// The conversation's token for the call.
    owner: Token,
    /// What is landing, kept for the outcome.
    change: Change,
    state: Landing,
}

#[derive(Debug)]
enum Landing {
    /// The checks at `check` among the run's are running.
    Checking { check: u32 },
    /// Withdrawn while its checks ran: they are being stopped.
    Aborting,
    /// The worker is pushing it.
    Pushing,
    /// Withdrawn while the worker pushed it: the host call is being cancelled.
    Unpushing,
    /// Terminal: holds nothing.
    Closed,
}

/// What became of a landing, for its run to act on.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Settled {
    /// It goes on.
    Going,
    /// The change is pushed, and the run finishes with it.
    Pushed(Change),
    /// The change did not land: the LLM was told why.
    Refused,
    /// The call returned with nothing decided.
    Cancelled,
}

/// The run and the conversation a call is of.
pub(crate) fn of(calls: &Slab<Call>, id: Id<Call>) -> (Id<Run>, Id<Conversation>) {
    let call = calls.get(id).expect("a call lives until it returns");
    (call.run, call.conversation)
}

/// Lands `change` for the conversation's call `owner`: the first check, or the
/// push. The call's slot is its conversation's.
#[expect(clippy::too_many_arguments, reason = "a call starts from everything it is about")]
pub(crate) fn begin(
    calls: &mut Slab<Call>,
    run: &Run,
    run_id: Id<Run>,
    conversation: Id<Conversation>,
    owner: Token,
    change: Change,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> Id<Call> {
    let call = Call { run: run_id, conversation, owner, change, state: Landing::Closed };
    // A returned call is reclaimed before its conversation can call again,
    // which takes another completion.
    let id = calls.insert(call).expect("a slot for each conversation's call");
    let call = calls.get_mut(id).expect("inserted above");
    call.state = next(call, id, run, 0, env, out);
    id
}

/// The checks of `id` ended as `ran`. `may_finish` says whether its run may
/// still finish.
pub(crate) fn checked(
    calls: &mut Slab<Call>,
    id: Id<Call>,
    run: &Run,
    may_finish: bool,
    ran: Ran,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> Settled {
    let call = calls.get_mut(id).expect("a call lives until it returns");
    let state = mem::replace(&mut call.state, Landing::Closed);
    let settled = match state {
        Landing::Checking { check } => match ran.exit {
            Exit::Code { code: 0 } if may_finish => {
                call.state = next(call, id, run, check.saturating_add(1), env, out);
                Settled::Going
            }
            Exit::Code { code: 0 } => back(call, Returned::Cancelled, Settled::Cancelled, out),
            Exit::Code { .. } | Exit::Signalled | Exit::TimedOut | Exit::Unstarted => {
                let repository = copy_of(&repository(run, check).name);
                back(call, Returned::ChecksFailed { repository, ran }, Settled::Refused, out)
            }
        },
        Landing::Aborting => back(call, Returned::Cancelled, Settled::Cancelled, out),
        Landing::Pushing | Landing::Unpushing | Landing::Closed => {
            unreachable!("checks end only while they run")
        }
    };
    retire(calls, id, &settled);
    settled
}

pub(crate) fn aborted(calls: &mut Slab<Call>, id: Id<Call>, out: &mut Queue<Request>) -> Settled {
    let call = calls.get_mut(id).expect("a call lives until it returns");
    let state = mem::replace(&mut call.state, Landing::Closed);
    let settled = match state {
        Landing::Aborting => back(call, Returned::Cancelled, Settled::Cancelled, out),
        Landing::Checking { .. } | Landing::Pushing | Landing::Unpushing | Landing::Closed => {
            unreachable!("an abort's terminal comes after the abort")
        }
    };
    retire(calls, id, &settled);
    settled
}

/// The push of `id` ended as `push`. `may_finish` says whether its run may
/// still finish.
pub(crate) fn pushed(
    calls: &mut Slab<Call>,
    id: Id<Call>,
    may_finish: bool,
    push: Push,
    out: &mut Queue<Request>,
) -> Settled {
    let call = calls.get_mut(id).expect("a call lives until it returns");
    let state = mem::replace(&mut call.state, Landing::Closed);
    let settled = match state {
        // A push that won the race with a withdraw has landed all the same.
        Landing::Pushing | Landing::Unpushing => match push {
            Push::Done if may_finish => {
                let change = call.change.clone();
                back(call, Returned::Accepted, Settled::Pushed(change), out)
            }
            Push::Done => back(call, Returned::Cancelled, Settled::Cancelled, out),
            Push::Moved => back(call, Returned::Moved, Settled::Refused, out),
            Push::Failed => back(call, Returned::Unpushed, Settled::Refused, out),
        },
        Landing::Checking { .. } | Landing::Aborting | Landing::Closed => {
            unreachable!("a push ends only while it is in flight")
        }
    };
    retire(calls, id, &settled);
    settled
}

pub(crate) fn host_cancelled(calls: &mut Slab<Call>, id: Id<Call>, out: &mut Queue<Request>) -> Settled {
    let call = calls.get_mut(id).expect("a call lives until it returns");
    let state = mem::replace(&mut call.state, Landing::Closed);
    let settled = match state {
        Landing::Unpushing => back(call, Returned::Cancelled, Settled::Cancelled, out),
        Landing::Checking { .. } | Landing::Aborting | Landing::Pushing | Landing::Closed => {
            unreachable!("a host call's cancel comes back only after the cancel")
        }
    };
    retire(calls, id, &settled);
    settled
}

/// The conversation withdraws its call `owner`, which is `id` if it is still
/// landing: stop what is in flight. A withdraw for a call that has already
/// returned is stale, and changes nothing.
pub(crate) fn withdraw(calls: &mut Slab<Call>, id: Id<Call>, owner: Token, out: &mut Queue<Request>) {
    let call = calls.get_mut(id).expect("a conversation's landing call lives until it returns");
    if call.owner != owner {
        return;
    }
    let state = mem::replace(&mut call.state, Landing::Closed);
    call.state = match state {
        Landing::Checking { check: _ } => {
            out.push(Request::Abort { owner: id.token() });
            Landing::Aborting
        }
        Landing::Pushing => {
            out.push(Request::CancelHost { owner: id.token() });
            Landing::Unpushing
        }
        Landing::Aborting | Landing::Unpushing | Landing::Closed => unreachable!("a call is withdrawn once"),
    };
}

/// The checks at `check` among the run's, or the push once there are none
/// left.
fn next(call: &Call, id: Id<Call>, run: &Run, check: u32, env: &Env<Limits>, out: &mut Queue<Request>) -> Landing {
    let Some(&index) = run.found.checks.get(check) else {
        out.push(Request::Push { worker: run.worker, owner: id.token(), change: call.change.clone() });
        return Landing::Pushing;
    };
    let root = repository_at(run, index).root;
    let deadline = env.now.saturating_add(env.limits.check_timeout);
    let program = Place { root, path: copy_of(prepare::CHECKS) };
    out.push(Request::Check { owner: id.token(), program, deadline, tail: env.limits.check_tail });
    out.push(Request::Checking { worker: run.worker, deadline });
    Landing::Checking { check }
}

/// Returns the call with `result`, closing it.
fn back(call: &Call, result: Returned, settled: Settled, out: &mut Queue<Request>) -> Settled {
    out.push(Request::Return { call: call.owner, result });
    settled
}

fn retire(calls: &mut Slab<Call>, id: Id<Call>, settled: &Settled) {
    match settled {
        Settled::Going => {}
        Settled::Pushed(_) | Settled::Refused | Settled::Cancelled => calls.retire(id),
    }
}

/// The repository whose checks are at `check` among the run's.
fn repository(run: &Run, check: u32) -> &Repository {
    let index = run.found.checks.get(check).expect("a check is of a repository with checks");
    repository_at(run, *index)
}

fn repository_at(run: &Run, index: u32) -> &Repository {
    let index = usize::try_from(index).expect("a u32 fits in a usize");
    run.charter.checkout.repositories.get(index).expect("checks are of the checkout's repositories")
}
