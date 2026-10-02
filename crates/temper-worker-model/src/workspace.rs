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
//! What a transition asks of the sub-models is routed once the record has
//! moved, and a record is retired as it closes, in one place ([`follow`]). It
//! closes in the step its run's answer is made in, or before: the host
//! releases a workspace with nothing under way, which the checkout releases at
//! once, and so is one whose prepare has ended.

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

/// What a transition asks of the sub-models: a record of the checkout's and
/// one of the host's, routed in that order once the record has moved.
#[derive(Debug)]
struct Then {
    checkout: Option<checkout::Event>,
    host: Option<host::Event>,
}

impl Then {
    const NOTHING: Then = Then { checkout: None, host: None };
}

// Entry points, one per record: look the workspace up, take its state out,
// run the cell's handler, follow from the new state.

/// The host asks for `workspace` to be prepared for its run `owner`.
pub(crate) fn prepare(model: &mut Model, env: &Env<Limits>, owner: Token, workspace: host::Workspace) {
    let repositories = u32::try_from(workspace.repositories.len()).expect("the host checked the repositories");
    let preparing = State::Preparing { hold: None, abandoned: false };
    let record = Workspace { run: owner, repositories, state: preparing };
    let id = model.workspaces.insert(record).expect("a workspace for every slot");
    let fresh = model.preparing.insert(owner, id).expect("a prepare for every slot");
    assert!(fresh.is_none(), "a run's workspace is prepared once");
    let spec = translate::spec(workspace);
    let then = Then { checkout: Some(checkout::Event::Prepare { client: id.token(), spec }), host: None };
    follow(model, env, id, then);
}

/// The host abandons the prepare of its run `owner`.
pub(crate) fn abort(model: &mut Model, env: &Env<Limits>, owner: Token) {
    let id = *model.preparing.get(&owner).expect("an abort is of a prepare in flight");
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Preparing { hold: Some(hold), abandoned: false } => abandon(hold, &mut then),
        State::Preparing { hold: None, .. } => unreachable!("an admitted prepare is held at once"),
        State::Preparing { hold: Some(_), abandoned: true } => unreachable!("a run is cancelled once"),
        State::Ready { .. } | State::Releasing | State::Closed => {
            unreachable!("the host abandons only a prepare in flight")
        }
    };
    follow(model, env, id, then);
}

/// The host starts its run's agent in the prepared `workspace`: the agent
/// sub-model spawns it in the workspace's directory.
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
    let id = Id::<Workspace>::from_token(workspace);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Ready { hold, directory, asked: None } => asked(hold, directory, owner, write, &mut then),
        State::Ready { asked: Some(_), .. } => unreachable!("one push or save at a time"),
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("only a prepared workspace pushes or saves")
        }
    };
    follow(model, env, id, then);
}

/// A push or a save, as the host asks it.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum Write {
    Push { message: Box<[u8]> },
    Save { branch: Box<[u8]> },
}

/// The host is done with `workspace`.
pub(crate) fn release(model: &mut Model, env: &Env<Limits>, workspace: Token) {
    let id = Id::<Workspace>::from_token(workspace);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Ready { hold, directory: _, asked: None } => releasing(hold, &mut then),
        State::Ready { asked: Some(_), .. } => unreachable!("the host releases a workspace with nothing in flight"),
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("the host releases a workspace it was given, once")
        }
    };
    follow(model, env, id, then);
}

/// The checkout admitted the prepare for `client`, and names its hold `hold`.
pub(crate) fn held(model: &mut Model, env: &Env<Limits>, client: Token, hold: Token) {
    let id = Id::<Workspace>::from_token(client);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Preparing { hold: None, abandoned } => State::Preparing { hold: Some(hold), abandoned },
        State::Preparing { hold: Some(_), .. } | State::Ready { .. } | State::Releasing | State::Closed => {
            unreachable!("a prepare is held once, as it is admitted")
        }
    };
    follow(model, env, id, Then::NOTHING);
}

/// The prepare for `client` ended.
pub(crate) fn prepared(model: &mut Model, env: &Env<Limits>, client: Token, prepared: checkout::Prepared) {
    let id = Id::<Workspace>::from_token(client);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let owner = record.run;
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Preparing { hold, abandoned } => ended(owner, client, hold, abandoned, prepared, &mut then),
        State::Ready { .. } | State::Releasing | State::Closed => unreachable!("a prepare ends once"),
    };
    model.preparing.remove(&owner);
    follow(model, env, id, then);
}

/// The push or the save for `client` ended, with `outcome`.
pub(crate) fn wrote(model: &mut Model, env: &Env<Limits>, client: Token, outcome: checkout::Outcome, push: bool) {
    let id = Id::<Workspace>::from_token(client);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let repositories = record.repositories;
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Ready { hold, directory, asked: Some(owner) } => {
            written(hold, directory, owner, translate::landings(outcome, repositories), push, &mut then)
        }
        State::Ready { asked: None, .. } | State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("a push or a save ends once, as the workspace stays held")
        }
    };
    follow(model, env, id, then);
}

/// The checkout released the workspace of `client`: nothing touches it.
pub(crate) fn released(model: &mut Model, env: &Env<Limits>, client: Token) {
    let id = Id::<Workspace>::from_token(client);
    let record = model.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Releasing => State::Closed,
        State::Preparing { .. } | State::Ready { .. } | State::Closed => {
            unreachable!("a workspace is released once, after its release")
        }
    };
    follow(model, env, id, Then::NOTHING);
}

/// What a record's new state implies, applied after every transition: a
/// Closed record is retired; then what the transition asks of the sub-models
/// is routed, the checkout's first.
fn follow(model: &mut Model, env: &Env<Limits>, id: Id<Workspace>, then: Then) {
    let record = model.workspaces.get(id).expect("a workspace lives until it is retired");
    let closed = match record.state {
        State::Closed => true,
        State::Preparing { .. } | State::Ready { .. } | State::Releasing => false,
    };
    if closed {
        model.workspaces.retire(id);
    }
    let Then { checkout, host } = then;
    if let Some(event) = checkout {
        route::checkout_step(model, env, event);
    }
    if let Some(event) = host {
        route::host_step(model, env, event);
    }
}

// Cell handlers.

/// Preparing, abort: the hold is released, which ends the prepare aborted.
fn abandon(hold: Token, then: &mut Then) -> State {
    then.checkout = Some(checkout::Event::Release { hold });
    State::Preparing { hold: Some(hold), abandoned: true }
}

/// Ready, push or save: asked of the checkout, for the host's `owner`.
fn asked(hold: Token, directory: Token, owner: Token, write: Write, then: &mut Then) -> State {
    then.checkout = Some(match write {
        Write::Push { message } => checkout::Event::Push { hold, message: translate::message(message) },
        Write::Save { branch } => checkout::Event::Save { hold, branch, message: translate::saved() },
    });
    State::Ready { hold, directory, asked: Some(owner) }
}

/// Ready, pushed or saved: the host's `owner` is told what became of each
/// repository.
fn written(
    hold: Token,
    directory: Token,
    owner: Token,
    landings: Box<[host::Landing]>,
    push: bool,
    then: &mut Then,
) -> State {
    then.host = Some(if push {
        host::Event::Pushed { owner, push: landings }
    } else {
        host::Event::Saved { owner, save: landings }
    });
    State::Ready { hold, directory, asked: None }
}

/// Ready, release.
fn releasing(hold: Token, then: &mut Then) -> State {
    then.checkout = Some(checkout::Event::Release { hold });
    State::Releasing
}

/// Preparing, prepared: ready, or the host told it was not prepared. A
/// prepare refused at the entrance holds nothing; one that failed holds its
/// workspace, released now; one abandoned released it already.
fn ended(
    owner: Token,
    client: Token,
    hold: Option<Token>,
    abandoned: bool,
    prepared: checkout::Prepared,
    then: &mut Then,
) -> State {
    match prepared {
        checkout::Prepared::Ready { workspace: directory } => {
            assert!(!abandoned, "a released hold's prepare ends aborted");
            then.host = Some(host::Event::Prepared { owner, workspace: client });
            State::Ready { hold: hold.expect("a prepare that ran was held"), directory, asked: None }
        }
        checkout::Prepared::Refused { refusal } => {
            then.host = Some(unprepared(owner, translate::refusal(refusal)));
            State::Closed
        }
        checkout::Prepared::Failed { failure } => {
            assert!(!abandoned, "a released hold's prepare ends aborted");
            let hold = hold.expect("a prepare that ran was held");
            then.checkout = Some(checkout::Event::Release { hold });
            then.host = Some(unprepared(owner, translate::failure(failure)));
            State::Releasing
        }
        checkout::Prepared::Aborted => {
            assert!(abandoned, "only an abandoned prepare is aborted");
            then.host = Some(unprepared(owner, host::Preparation::Transient));
            State::Releasing
        }
    }
}

fn unprepared(owner: Token, failure: host::Preparation) -> host::Event {
    host::Event::Unprepared { owner, failure, detail: Box::new([]) }
}
