//! Landing a change (agent-model.md, 4.4): when the LLM finishes with a
//! change, the run runs the checks of the repositories that have them, one
//! after another, and then asks the worker to push. The checks were looked
//! for when the run started, and only when the outcome spec has a change pass
//! them (`prepare`).
//!
//! Checked is pushed: nothing may write to the checkout from the first check
//! to the push, and nothing does, by construction. Only main may finish. A
//! finish is a write, which a conversation runs alone, so when it comes, no
//! other call of main is in flight; and a sub-agent lives only as long as the
//! call that asked for it, so every sub-agent main asked for, and theirs in
//! turn, has ended. The run asserts as much when a finish comes; while the
//! change lands, main waits on its finish and asks for nothing.
//!
//! A failing check, or a push that fails, goes back to the LLM as feedback,
//! and it carries on. A push that finds the branch moved ends the run: the
//! push is a fast-forward from where the run started, so no later one can
//! land, and the LLM cannot fix that. The call is its conversation's:
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
//! Pushing    pushed                       Closed     return: accepted, moved (the run ends) or unpushed
//!            withdraw                     Unpushing  cancel the host call
//! Unpushing  pushed                       Closed     return: as for Pushing
//!            host cancelled               Closed     return: cancelled
//! ```
//!
//! Every other cell is unreachable: one terminal per request, a withdraw at
//! most once per call, and a cancel's terminal only after the cancel.

use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Queue, Token};

use crate::boundary::{Exit, Place, Push, Ran, Request, Returned};
use crate::call::Call;
use crate::charter::Repository;
use crate::limits::Limits;
use crate::outcome::Change;
use crate::prepare;
use crate::run::Run;

/// A finish call's change, landing.
#[derive(Debug)]
pub(crate) struct Landing {
    /// What is landing, kept for the outcome.
    change: Change,
    stage: Stage,
}

#[derive(Debug)]
enum Stage {
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
    /// The change cannot land: its branch moved since the run started.
    Stale,
    /// The call returned with nothing decided.
    Cancelled,
}

/// `change`, to land once its call has a name.
pub(crate) fn landing(change: Change) -> Landing {
    Landing { change, stage: Stage::Closed }
}

/// Starts landing the change of the call `id`: its first check, or the push.
pub(crate) fn begin(landing: &mut Landing, id: Id<Call>, run: &Run, env: &Env<Limits>, out: &mut Queue<Request>) {
    landing.stage = next(landing, id, run, 0, env, out);
}

/// The checks of the call `id`, which its conversation names `owner`, ended as
/// `ran`. `may_finish` says whether its run may still finish.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the fields it touches")]
pub(crate) fn checked(
    landing: &mut Landing,
    id: Id<Call>,
    owner: Token,
    run: &Run,
    may_finish: bool,
    ran: Ran,
    env: &Env<Limits>,
    out: &mut Queue<Request>,
) -> Settled {
    let stage = mem::replace(&mut landing.stage, Stage::Closed);
    match stage {
        Stage::Checking { check } => match ran.exit {
            Exit::Code { code: 0 } if may_finish => {
                landing.stage = next(landing, id, run, check.saturating_add(1), env, out);
                Settled::Going
            }
            Exit::Code { code: 0 } => back(owner, Returned::Cancelled, Settled::Cancelled, out),
            Exit::Code { .. } | Exit::Signalled | Exit::TimedOut | Exit::Unstarted => {
                let repository = copy_of(&repository(run, check).name);
                back(owner, Returned::ChecksFailed { repository, ran }, Settled::Refused, out)
            }
        },
        Stage::Aborting => back(owner, Returned::Cancelled, Settled::Cancelled, out),
        Stage::Pushing | Stage::Unpushing | Stage::Closed => unreachable!("checks end only while they run"),
    }
}

pub(crate) fn aborted(landing: &mut Landing, owner: Token, out: &mut Queue<Request>) -> Settled {
    let stage = mem::replace(&mut landing.stage, Stage::Closed);
    match stage {
        Stage::Aborting => back(owner, Returned::Cancelled, Settled::Cancelled, out),
        Stage::Checking { .. } | Stage::Pushing | Stage::Unpushing | Stage::Closed => {
            unreachable!("an abort's terminal comes after the abort")
        }
    }
}

/// The push of a call its conversation names `owner` ended as `push`.
/// `may_finish` says whether its run may still finish.
pub(crate) fn pushed(
    landing: &mut Landing,
    owner: Token,
    may_finish: bool,
    push: Push,
    out: &mut Queue<Request>,
) -> Settled {
    let stage = mem::replace(&mut landing.stage, Stage::Closed);
    match stage {
        // A push that won the race with a withdraw has landed all the same.
        Stage::Pushing | Stage::Unpushing => match push {
            Push::Done if may_finish => {
                let change = landing.change.clone();
                back(owner, Returned::Accepted, Settled::Pushed(change), out)
            }
            Push::Done => back(owner, Returned::Cancelled, Settled::Cancelled, out),
            Push::Moved => back(owner, Returned::Moved, Settled::Stale, out),
            Push::Failed => back(owner, Returned::Unpushed, Settled::Refused, out),
        },
        Stage::Checking { .. } | Stage::Aborting | Stage::Closed => {
            unreachable!("a push ends only while it is in flight")
        }
    }
}

pub(crate) fn host_cancelled(landing: &mut Landing, owner: Token, out: &mut Queue<Request>) -> Settled {
    let stage = mem::replace(&mut landing.stage, Stage::Closed);
    match stage {
        Stage::Unpushing => back(owner, Returned::Cancelled, Settled::Cancelled, out),
        Stage::Checking { .. } | Stage::Aborting | Stage::Pushing | Stage::Closed => {
            unreachable!("a host call's cancel comes back only after the cancel")
        }
    }
}

/// The conversation withdraws the call `id`: stop what is in flight.
pub(crate) fn withdraw(landing: &mut Landing, id: Id<Call>, out: &mut Queue<Request>) {
    let stage = mem::replace(&mut landing.stage, Stage::Closed);
    landing.stage = match stage {
        Stage::Checking { check: _ } => {
            out.push(Request::Abort { owner: id.token() });
            Stage::Aborting
        }
        Stage::Pushing => {
            out.push(Request::CancelHost { owner: id.token() });
            Stage::Unpushing
        }
        Stage::Aborting | Stage::Unpushing | Stage::Closed => unreachable!("a call is withdrawn once"),
    };
}

/// The checks at `check` among the run's, or the push once there are none
/// left.
fn next(landing: &Landing, id: Id<Call>, run: &Run, check: u32, env: &Env<Limits>, out: &mut Queue<Request>) -> Stage {
    let Some(&index) = run.found.checks.get(check) else {
        out.push(Request::Push { worker: run.worker, owner: id.token(), change: landing.change.clone() });
        return Stage::Pushing;
    };
    let root = repository_at(run, index).root;
    let deadline = env.now.saturating_add(env.limits.check_timeout);
    let program = Place { root, path: copy_of(prepare::CHECKS) };
    out.push(Request::Check { owner: id.token(), program, deadline, tail: env.limits.check_tail });
    out.push(Request::Checking { worker: run.worker, deadline });
    Stage::Checking { check }
}

/// Returns the call its conversation names `owner` with `result`.
fn back(owner: Token, result: Returned, settled: Settled, out: &mut Queue<Request>) -> Settled {
    out.push(Request::Return { call: owner, result });
    settled
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
