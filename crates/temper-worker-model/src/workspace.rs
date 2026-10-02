//! Workspaces: what the top level keeps of each workspace the host asks for,
//! from its `Prepare` to the checkout's `Released`, so that the host's names
//! and the checkout's meet. The host names a workspace by the token the top
//! level gives it (its record's), and a push or a save by its own owner; the
//! checkout names its hold, echoing the record's token as its client; and io
//! names the directory the workspace is in, where an agent is spawned.
//!
//! A run cancelled as its workspace is prepared abandons the prepare by
//! releasing the hold: the checkout cancels its operation in flight, and ends
//! the prepare aborted once that has ended, releasing the hold with it, and the
//! host is told the workspace was not prepared, with nothing of it held. A
//! prepare that failed keeps its hold, which the top level releases at once,
//! as the host is told nothing is held then too.
//!
//! The transition table. Every other cell is unreachable by the contracts: the
//! checkout's (a `Held` at once for an admitted prepare, then one `Prepared`;
//! one `Pushed` or `Saved` for each push or save; one `Released` for each
//! release, once nothing touches the workspace) and the host's (a start, a
//! push, a save or a release only of a workspace prepared, one push or save in
//! flight, and a release once nothing is).
//!
//! ```text
//! state      event                          next        emits
//! -          prepare (host)                 Preparing   prepare (checkout)
//! Preparing  held                           Preparing
//!            abort (host)                   Preparing   release (checkout): abandoned
//!            prepared: ready                Ready       prepared (host)
//!            prepared: refused              Closed      unprepared (host)
//!            prepared: failed               Releasing   release (checkout), unprepared (host)
//!            prepared: aborted, abandoned   Releasing   unprepared (host)
//! Ready      start (host)                   Ready       spawn (agent), in its directory
//!            push, save (host)              Ready       push, save (checkout)
//!            pushed, saved                  Ready       pushed, saved (host)
//!            release (host)                 Releasing   release (checkout)
//! Releasing  released                       Closed
//! ```
//!
//! A record is retired as it closes. It closes in the step its run's answer is
//! made in, or before: the host releases a workspace with nothing under way,
//! which the checkout releases at once, and so is one whose prepare has ended.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, Token};
use temper_worker_model_agent as agent;
use temper_worker_model_checkout as checkout;
use temper_worker_model_host as host;

use crate::limits::Limits;
use crate::model::Model;
use crate::route;
use crate::translate;

#[derive(Debug)]
pub(crate) struct Workspace {
    /// The hosted run it is for: the host's token for the run.
    run: Token,
    /// How many repositories it holds.
    repositories: u32,
    state: State,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    /// Its prepare is in flight: `hold` names it once the checkout has
    /// admitted it, and it is `abandoned` once its run is cancelled and the
    /// hold released.
    Preparing { hold: Option<Token>, abandoned: bool },
    /// Prepared in the directory io names `directory`. `asked` is the host's
    /// owner of the push or the save in flight, if there is one.
    Ready { hold: Token, directory: Token, asked: Option<Token> },
    /// Released, until the checkout says nothing touches it.
    Releasing,
    /// Terminal: holds nothing.
    Closed,
}

/// The host asks for `workspace` to be prepared for its run `owner`.
pub(crate) fn prepare(model: &mut Model, env: &Env<Limits>, owner: Token, workspace: host::Workspace) {
    let repositories = u32::try_from(workspace.repositories.len()).expect("the host checked the repositories");
    let preparing = State::Preparing { hold: None, abandoned: false };
    let record = Workspace { run: owner, repositories, state: preparing };
    let id = model.workspaces.insert(record).expect("a workspace for every slot");
    let fresh = model.preparing.insert(owner, id).expect("a prepare for every slot");
    assert!(fresh.is_none(), "a run's workspace is prepared once");
    let spec = translate::spec(workspace);
    route::checkout_step(model, env, checkout::Event::Prepare { client: id.token(), spec });
}

/// The host abandons the prepare of its run `owner`.
pub(crate) fn abort(model: &mut Model, env: &Env<Limits>, owner: Token) {
    let id = *model.preparing.get(&owner).expect("an abort is of a prepare in flight");
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let hold = match record.state {
        State::Preparing { hold: Some(hold), abandoned: false } => hold,
        State::Preparing { hold: None, .. } => unreachable!("an admitted prepare is held at once"),
        State::Preparing { hold: Some(_), abandoned: true } => unreachable!("a run is cancelled once"),
        State::Ready { .. } | State::Releasing | State::Closed => {
            unreachable!("the host abandons only a prepare in flight")
        }
    };
    record.state = State::Preparing { hold: Some(hold), abandoned: true };
    route::checkout_step(model, env, checkout::Event::Release { hold });
}

/// The host starts its run's agent in the prepared `workspace`.
pub(crate) fn start(
    model: &mut Model,
    env: &Env<Limits>,
    owner: Token,
    workspace: Token,
    charter: Box<[u8]>,
    snapshot: Option<Box<[u8]>>,
) {
    let record = model.workspaces.get(Id::from_token(workspace)).expect("a workspace lives until it is released");
    let directory = match record.state {
        State::Ready { directory, .. } => directory,
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("an agent starts in a prepared workspace")
        }
    };
    let spawn = agent::Spawn { workspace: directory, charter, snapshot };
    route::agent_step(model, env, agent::Event::Spawn { client: owner, spawn });
}

/// The host's push or save, the host's `owner`, of `workspace`.
pub(crate) fn write(model: &mut Model, env: &Env<Limits>, owner: Token, workspace: Token, write: Write) {
    let record = model.workspaces.get_mut(Id::from_token(workspace)).expect("a workspace lives until it is released");
    let hold = match record.state {
        State::Ready { hold, directory, asked: None } => {
            record.state = State::Ready { hold, directory, asked: Some(owner) };
            hold
        }
        State::Ready { asked: Some(_), .. } => unreachable!("one push or save at a time"),
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("only a prepared workspace pushes or saves")
        }
    };
    let event = match write {
        Write::Push { message } => checkout::Event::Push { hold, message: translate::message(message) },
        Write::Save { branch } => checkout::Event::Save { hold, branch, message: translate::saved() },
    };
    route::checkout_step(model, env, event);
}

/// A push or a save, as the host asks it.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum Write {
    Push { message: Box<[u8]> },
    Save { branch: Box<[u8]> },
}

/// The host is done with `workspace`.
pub(crate) fn release(model: &mut Model, env: &Env<Limits>, workspace: Token) {
    let record = model.workspaces.get_mut(Id::from_token(workspace)).expect("a workspace lives until it is released");
    let state = mem::replace(&mut record.state, State::Releasing);
    let hold = match state {
        State::Ready { hold, directory: _, asked: None } => hold,
        State::Ready { asked: Some(_), .. } => unreachable!("the host releases a workspace with nothing in flight"),
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("the host releases a workspace it was given, once")
        }
    };
    route::checkout_step(model, env, checkout::Event::Release { hold });
}

/// The checkout admitted the prepare for `client`, and names its hold `hold`.
pub(crate) fn held(model: &mut Model, client: Token, hold: Token) {
    let record = model.workspaces.get_mut(Id::from_token(client)).expect("a workspace lives until it is released");
    match record.state {
        State::Preparing { hold: None, abandoned } => record.state = State::Preparing { hold: Some(hold), abandoned },
        State::Preparing { hold: Some(_), .. } | State::Ready { .. } | State::Releasing | State::Closed => {
            unreachable!("a prepare is held once, as it is admitted")
        }
    }
}

/// The prepare for `client` ended.
pub(crate) fn prepared(model: &mut Model, env: &Env<Limits>, client: Token, prepared: checkout::Prepared) {
    let id = Id::<Workspace>::from_token(client);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let owner = record.run;
    let event = match record.state {
        State::Preparing { hold, abandoned } => match prepared {
            checkout::Prepared::Ready { workspace: directory } => {
                assert!(!abandoned, "a released hold's prepare ends aborted");
                let hold = hold.expect("a prepare that ran was held");
                record.state = State::Ready { hold, directory, asked: None };
                host::Event::Prepared { owner, workspace: client }
            }
            checkout::Prepared::Refused { refusal } => {
                record.state = State::Closed;
                model.workspaces.retire(id);
                unprepared(owner, translate::refusal(refusal))
            }
            checkout::Prepared::Failed { failure } => {
                assert!(!abandoned, "a released hold's prepare ends aborted");
                record.state = State::Releasing;
                let hold = hold.expect("a prepare that ran was held");
                route::checkout_step(model, env, checkout::Event::Release { hold });
                unprepared(owner, translate::failure(failure))
            }
            checkout::Prepared::Aborted => {
                assert!(abandoned, "only an abandoned prepare is aborted");
                record.state = State::Releasing;
                unprepared(owner, host::Preparation::Transient)
            }
        },
        State::Ready { .. } | State::Releasing | State::Closed => unreachable!("a prepare ends once"),
    };
    model.preparing.remove(&owner);
    route::host_step(model, env, event);
}

/// The push or the save for `client` ended, with `outcome`.
pub(crate) fn wrote(model: &mut Model, env: &Env<Limits>, client: Token, outcome: checkout::Outcome, push: bool) {
    let record = model.workspaces.get_mut(Id::from_token(client)).expect("a workspace lives until it is released");
    let owner = match record.state {
        State::Ready { hold, directory, asked: Some(owner) } => {
            record.state = State::Ready { hold, directory, asked: None };
            owner
        }
        State::Ready { asked: None, .. } | State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("a push or a save ends once, as the workspace stays held")
        }
    };
    let landings = translate::landings(outcome, record.repositories);
    let event =
        if push { host::Event::Pushed { owner, push: landings } } else { host::Event::Saved { owner, save: landings } };
    route::host_step(model, env, event);
}

/// The checkout released the workspace of `client`: nothing touches it.
pub(crate) fn released(model: &mut Model, client: Token) {
    let id = Id::<Workspace>::from_token(client);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    match record.state {
        State::Releasing => {
            record.state = State::Closed;
            model.workspaces.retire(id);
        }
        State::Preparing { .. } | State::Ready { .. } | State::Closed => {
            unreachable!("a workspace is released once, after its release")
        }
    }
}

fn unprepared(owner: Token, failure: host::Preparation) -> host::Event {
    host::Event::Unprepared { owner, failure, detail: Box::new([]) }
}
