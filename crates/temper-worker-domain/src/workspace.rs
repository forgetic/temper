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
//! What a transition asks of the child domains is routed once the record has
//! moved, and a record is retired as it closes, in one place ([`follow`]). It
//! closes in the step its run's answer is made in, or before: the host
//! releases a workspace with nothing under way, which the checkout releases at
//! once, and so is one whose prepare has ended.

use alloc::boxed::Box;
use core::mem;

use crate::wire;
use jig_worker_host as host;
use skein_lib::bytes::copy_of;
use skein_lib::{Env, Id, List, Map, Token};
use temper_worker_domain_agent as agent;
use temper_worker_domain_checkout as checkout;

use crate::domain::Domain;
use crate::limits::Limits;
use crate::route;
use crate::translate;

/// Application items and outcomes named by a token across the host boundary.
#[derive(Debug)]
pub(crate) struct Items {
    spec: Option<wire::Workspace>,
    save: Option<Box<[u8]>>,
    tags: Box<[u32]>,
    landed: Map<u32, [u8; 32]>,
    saved: Option<Box<[wire::Landing]>>,
    last: Option<wire::Push>,
    preparation: Option<wire::Preparation>,
}

/// Translate an engine assignment, retaining its workspace items at the root.
pub(crate) fn stage(
    domain: &mut Domain,
    env: &Env<Limits>,
    assignment: wire::Assignment,
    next: bool,
) -> Result<host::Assignment, wire::Refusal> {
    match crate::assignment::check(&assignment, &env.limits, next) {
        Ok(()) => {}
        Err(invalid) => return Err(wire::Refusal::Invalid(invalid)),
    }
    let wire::Assignment { run, attempt, workspace, save, charter, snapshot, grants } = assignment;
    let mut merging = false;
    for repository in &workspace.repositories {
        match repository.start {
            wire::Start::Merge { .. } => merging = true,
            wire::Start::Base { .. }
            | wire::Start::Branch { .. }
            | wire::Start::Commit { .. }
            | wire::Start::Saved { .. } => {}
        }
    }
    let save = if next && merging { None } else { save };
    let mut tags = List::with_capacity(u32::try_from(workspace.repositories.len()).expect("checked item count"));
    for repository in &workspace.repositories {
        tags.push(repository.tag).expect("room for each item tag");
    }
    let count = u32::try_from(workspace.repositories.len()).expect("checked item count");
    let entry = Items {
        spec: Some(workspace),
        save,
        tags: tags.into_boxed(),
        landed: Map::with_capacity(count),
        saved: None,
        last: None,
        preparation: None,
    };
    let Ok(id) = domain.items.insert(entry) else {
        return Err(wire::Refusal::Busy);
    };
    let old = domain.items_by_run.insert(run, id).expect("room for each staged run");
    assert!(old.is_none(), "the link and host fenced an existing run");
    Ok(host::Assignment {
        run,
        attempt,
        workspace: Some(host::Workspace { workstream: run.raw(), items: id.token() }),
        save: domain.items.get(id).expect("inserted above").save.is_some(),
        charter,
        snapshot,
        grants,
    })
}

/// Finish the application's side of a hosted assignment.
pub(crate) fn finish(domain: &mut Domain, run: Token) -> wire::Work {
    let id = domain.items_by_run.remove(&run).expect("one staged assignment for each answer");
    let entry = domain.items.get_mut(id).expect("staged items live through the answer");
    let mut landed = List::with_capacity(entry.landed.len());
    for (tag, commit) in &entry.landed {
        landed.push(wire::Landed { tag: *tag, commit: *commit }).expect("room for each landed item");
    }
    let saved = entry.saved.take();
    domain.items.retire(id);
    wire::Work { landed: landed.into_boxed(), saved }
}

/// The detailed result the application's workspace supplied for a delivery.
pub(crate) fn delivery_reply(domain: &Domain, left: Token) -> wire::Push {
    let id = Id::<Items>::from_token(left);
    let entry = domain.items.get(id).expect("a delivery belongs to live staged items");
    entry.last.clone().expect("a delivery has a recorded result")
}

/// The application's detailed preparation failure, if there was one.
pub(crate) fn preparation(domain: &Domain, run: Token) -> Option<wire::Preparation> {
    let id = *domain.items_by_run.get(&run).expect("one staged assignment for each answer");
    domain.items.get(id).expect("staged items live through the answer").preparation
}

#[derive(Debug)]
pub(crate) struct Workspace {
    /// The hosted run it is for: the host's token for the run.
    run: Token,
    items: Id<Items>,
    repositories: u32,
    roots: Box<[agent::channel::Repository]>,
    identities: Box<[u32]>,
    conflicts: Box<[checkout::Conflicts]>,
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

/// What a transition asks of the child domains: a record of the checkout's and
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
pub(crate) fn prepare(domain: &mut Domain, env: &Env<Limits>, owner: Token, workspace: host::Workspace) {
    let items = Id::<Items>::from_token(workspace.items);
    let spec =
        domain.items.get_mut(items).expect("the root staged the workspace items").spec.take().expect("prepared once");
    let repositories = u32::try_from(spec.repositories.len()).expect("the root checked the repositories");
    let mut roots = List::with_capacity(repositories);
    let mut identities = List::with_capacity(repositories);
    for repository in &spec.repositories {
        let writable = match repository.access {
            wire::Access::ReadOnly => false,
            wire::Access::Writable { .. } | wire::Access::WritableV2 { .. } => true,
        };
        identities.push(repository.identity).expect("room for every repository identity");
        roots
            .push(agent::channel::Repository { name: copy_of(&repository.name), writable })
            .expect("room for every repository");
    }
    let preparing = State::Preparing { hold: None, abandoned: false };
    let record = Workspace {
        run: owner,
        items,
        repositories,
        roots: roots.into_boxed(),
        identities: identities.into_boxed(),
        conflicts: Box::new([]),
        state: preparing,
    };
    let id = domain.workspaces.insert(record).expect("a workspace for every slot");
    let fresh = domain.preparing.insert(owner, id).expect("a prepare for every slot");
    assert!(fresh.is_none(), "a run's workspace is prepared once");
    let spec = translate::spec(spec);
    let then = Then { checkout: Some(checkout::Event::Prepare { client: id.token(), spec }), host: None };
    follow(domain, env, id, then);
}

/// The host abandons the prepare of its run `owner`.
pub(crate) fn abort(domain: &mut Domain, env: &Env<Limits>, owner: Token) {
    let id = *domain.preparing.get(&owner).expect("an abort is of a prepare in flight");
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
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
    follow(domain, env, id, then);
}

/// The host starts its run's agent in the prepared `workspace`: the agent
/// child domain spawns it in the workspace's directory.
pub(crate) fn start(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    workspace: Token,
    charter: Box<[u8]>,
    snapshot: Option<Box<[u8]>>,
    grants: Box<[wire::Grant]>,
) {
    let record = domain.workspaces.get(Id::from_token(workspace)).expect("a workspace lives until it is released");
    let directory = match record.state {
        State::Ready { directory, .. } => directory,
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("an agent starts in a prepared workspace")
        }
    };
    let mut repositories = List::with_capacity(record.repositories);
    for root in &record.roots {
        repositories
            .push(agent::channel::Repository { name: copy_of(&root.name), writable: root.writable })
            .expect("room for every repository");
    }
    let mut names = List::with_capacity(u32::try_from(grants.len()).expect("validated grants"));
    for grant in grants {
        if !record.identities.contains(&grant.account) {
            names.push(route::channel_grant(grant)).expect("room for every LLM grant");
        }
    }
    let spawn = agent::Spawn {
        workspace: directory,
        charter,
        snapshot,
        repositories: repositories.into_boxed(),
        grants: names.into_boxed(),
    };
    route::agent_step(domain, env, agent::Event::Spawn { client: owner, spawn });
}

/// The host's push or save, the host's `owner`, of `workspace`.
pub(crate) fn write(domain: &mut Domain, env: &Env<Limits>, owner: Token, workspace: Token, write: Write) {
    let id = Id::<Workspace>::from_token(workspace);
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Ready { hold, directory, asked: None } => asked(hold, directory, owner, write, &mut then),
        State::Ready { asked: Some(_), .. } => unreachable!("one push or save at a time"),
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("only a prepared workspace pushes or saves")
        }
    };
    follow(domain, env, id, then);
}

/// Save through the application's checkout using the branch kept with its items.
pub(crate) fn save(domain: &mut Domain, env: &Env<Limits>, owner: Token, workspace: Token) {
    let record = domain.workspaces.get(Id::<Workspace>::from_token(workspace)).expect("a prepared workspace is live");
    let items = domain.items.get_mut(record.items).expect("its items remain staged");
    let branch = items.save.take().expect("the host saves only when requested");
    write(domain, env, owner, workspace, Write::Save { branch });
}

/// A push or a save, as the host asks it.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum Write {
    Push { message: Box<[u8]> },
    PushV2 { title: Box<[u8]>, body: Box<[u8]> },
    Save { branch: Box<[u8]> },
}

/// The host is done with `workspace`.
pub(crate) fn release(domain: &mut Domain, env: &Env<Limits>, workspace: Token) {
    let id = Id::<Workspace>::from_token(workspace);
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Ready { hold, directory: _, asked: None } => releasing(hold, &mut then),
        State::Ready { asked: Some(_), .. } => unreachable!("the host releases a workspace with nothing in flight"),
        State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("the host releases a workspace it was given, once")
        }
    };
    follow(domain, env, id, then);
}

/// The checkout admitted the prepare for `client`, and names its hold `hold`.
pub(crate) fn held(domain: &mut Domain, env: &Env<Limits>, client: Token, hold: Token) {
    let id = Id::<Workspace>::from_token(client);
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Preparing { hold: None, abandoned } => State::Preparing { hold: Some(hold), abandoned },
        State::Preparing { hold: Some(_), .. } | State::Ready { .. } | State::Releasing | State::Closed => {
            unreachable!("a prepare is held once, as it is admitted")
        }
    };
    follow(domain, env, id, Then::NOTHING);
}

/// The prepare for `client` ended.
pub(crate) fn prepared(domain: &mut Domain, env: &Env<Limits>, client: Token, prepared: checkout::Prepared) {
    let id = Id::<Workspace>::from_token(client);
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let items = record.items;
    let failure = match &prepared {
        checkout::Prepared::Refused { refusal } => Some(translate::refusal(*refusal)),
        checkout::Prepared::Failed { failure } => Some(translate::failure(*failure)),
        checkout::Prepared::Ready { .. } | checkout::Prepared::Aborted => None,
    };
    if let Some(failure) = failure {
        domain.items.get_mut(items).expect("the assignment lives through prepare").preparation = Some(failure);
    }
    let owner = record.run;
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Preparing { hold, abandoned } => match prepared {
            checkout::Prepared::Ready { workspace, conflicts } => {
                record.conflicts = conflicts;
                ended(
                    owner,
                    client,
                    items.token(),
                    hold,
                    abandoned,
                    checkout::Prepared::Ready { workspace, conflicts: Box::new([]) },
                    &mut then,
                )
            }
            checkout::Prepared::Refused { .. } | checkout::Prepared::Failed { .. } | checkout::Prepared::Aborted => {
                ended(owner, client, items.token(), hold, abandoned, prepared, &mut then)
            }
        },
        State::Ready { .. } | State::Releasing | State::Closed => unreachable!("a prepare ends once"),
    };
    domain.preparing.remove(&owner);
    follow(domain, env, id, then);
}

/// The push or the save for `client` ended, with `outcome`.
pub(crate) fn wrote(domain: &mut Domain, env: &Env<Limits>, client: Token, outcome: checkout::Outcome, push: bool) {
    let id = Id::<Workspace>::from_token(client);
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let repositories = record.repositories;
    let items = record.items;
    let landings = translate::landings(outcome, repositories);
    let next = domain.items.get_mut(items).expect("workspace items live until the answer");
    let event = if push {
        let mut changed = false;
        for (index, landing) in landings.iter().enumerate() {
            match landing {
                wire::Landing::Landed { commit } => {
                    changed = true;
                    let tag = *next.tags.get(index).expect("every item has a tag");
                    next.landed.insert(tag, *commit).expect("room for every item");
                }
                wire::Landing::Conflicted { .. }
                | wire::Landing::Explained { .. }
                | wire::Landing::Moved
                | wire::Landing::Failed
                | wire::Landing::Refused
                | wire::Landing::Unchanged => {}
            }
        }
        let result = translate::summarize(landings);
        let outcome = translate::delivery_outcome(&result);
        next.last = Some(result);
        host::Event::Delivered {
            owner: Token::new(0),
            delivery: host::Delivery { outcome, left: items.token(), changed },
        }
    } else {
        next.saved = Some(landings);
        host::Event::Saved { owner: Token::new(0), at: Some(items.token()) }
    };
    let mut then = Then::NOTHING;
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Ready { hold, directory, asked: Some(owner) } => written(hold, directory, owner, event, &mut then),
        State::Ready { asked: None, .. } | State::Preparing { .. } | State::Releasing | State::Closed => {
            unreachable!("a push or a save ends once, as the workspace stays held")
        }
    };
    follow(domain, env, id, then);
}

/// The checkout released the workspace of `client`: nothing touches it.
pub(crate) fn released(domain: &mut Domain, env: &Env<Limits>, client: Token) {
    let id = Id::<Workspace>::from_token(client);
    let record = domain.workspaces.get_mut(id).expect("a workspace lives until it is released");
    let state = mem::replace(&mut record.state, State::Closed);
    record.state = match state {
        State::Releasing => State::Closed,
        State::Preparing { .. } | State::Ready { .. } | State::Closed => {
            unreachable!("a workspace is released once, after its release")
        }
    };
    follow(domain, env, id, Then::NOTHING);
}

/// What a record's new state implies, applied after every transition: a
/// Closed record is retired; then what the transition asks of the child domains
/// is routed, the checkout's first.
fn follow(domain: &mut Domain, env: &Env<Limits>, id: Id<Workspace>, then: Then) {
    let record = domain.workspaces.get(id).expect("a workspace lives until it is retired");
    let closed = match record.state {
        State::Closed => true,
        State::Preparing { .. } | State::Ready { .. } | State::Releasing => false,
    };
    if closed {
        domain.workspaces.retire(id);
    }
    let Then { checkout, host } = then;
    if let Some(event) = checkout {
        route::checkout_step(domain, env, event);
    }
    if let Some(event) = host {
        route::host_step(domain, env, event);
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
        Write::PushV2 { title, body } => checkout::Event::Push { hold, message: checkout::Message { title, body } },
        Write::Push { message } => checkout::Event::Push { hold, message: translate::message(message) },
        Write::Save { branch } => checkout::Event::Save { hold, branch, message: translate::saved() },
    });
    State::Ready { hold, directory, asked: Some(owner) }
}

/// Ready, pushed or saved: the host's `owner` is told what became of each
/// repository.
fn written(hold: Token, directory: Token, owner: Token, event: host::Event, then: &mut Then) -> State {
    then.host = Some(match event {
        host::Event::Delivered { owner: _, delivery } => host::Event::Delivered { owner, delivery },
        host::Event::Saved { owner: _, at } => host::Event::Saved { owner, at },
        host::Event::AssignV2 { .. }
        | host::Event::Turn { .. }
        | host::Event::FinishedV2 { .. }
        | host::Event::Assign { .. }
        | host::Event::Inbound { .. }
        | host::Event::Cancel { .. }
        | host::Event::Grant { .. }
        | host::Event::Relayed { .. }
        | host::Event::RelayCancelled { .. }
        | host::Event::CancelAll { .. }
        | host::Event::Report
        | host::Event::Unacknowledged { .. }
        | host::Event::Prepared { .. }
        | host::Event::Unprepared { .. }
        | host::Event::Started { .. }
        | host::Event::Called { .. }
        | host::Event::Withdrawn { .. }
        | host::Event::Bounced { .. }
        | host::Event::Yielded { .. }
        | host::Event::Finished { .. }
        | host::Event::Faulted { .. }
        | host::Event::Gone { .. } => unreachable!("a workspace write ends as delivery or save"),
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
    items: Token,
    hold: Option<Token>,
    abandoned: bool,
    prepared: checkout::Prepared,
    then: &mut Then,
) -> State {
    match prepared {
        checkout::Prepared::Ready { workspace: directory, conflicts } => {
            assert!(conflicts.is_empty(), "conflicts moved to the workspace record before the transition");
            assert!(!abandoned, "a released hold's prepare ends aborted");
            then.host = Some(host::Event::Prepared { owner, workspace: client });
            State::Ready { hold: hold.expect("a prepare that ran was held"), directory, asked: None }
        }
        checkout::Prepared::Refused { refusal } => {
            then.host = Some(unprepared(owner, translate::preparation(translate::refusal(refusal), items)));
            State::Closed
        }
        checkout::Prepared::Failed { failure } => {
            assert!(!abandoned, "a released hold's prepare ends aborted");
            let hold = hold.expect("a prepare that ran was held");
            then.checkout = Some(checkout::Event::Release { hold });
            then.host = Some(unprepared(owner, translate::preparation(translate::failure(failure), items)));
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

pub(crate) fn start_v2(
    domain: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    workspace: Token,
    charter: Box<[u8]>,
    transcript: Option<Box<[u8]>>,
    grants: Box<[wire::Grant]>,
) {
    let record = domain.workspaces.get_mut(Id::from_token(workspace)).expect("a workspace lives until released");
    let directory = match record.state {
        State::Ready { directory, .. } => directory,
        State::Preparing { .. } | State::Releasing | State::Closed => unreachable!("only a prepared workspace starts"),
    };
    let mut repositories = List::with_capacity(record.repositories);
    for root in &record.roots {
        repositories
            .push(agent::channel::RepositoryV2 {
                name: copy_of(&root.name),
                writable: root.writable,
                conflicts: Box::new([]),
            })
            .expect("one descriptor per repository");
    }
    let mut names = List::with_capacity(u32::try_from(grants.len()).expect("validated grants"));
    for grant in grants {
        if !record.identities.contains(&grant.account) {
            names.push(route::channel_grant(grant)).expect("room for every LLM grant");
        }
    }
    for conflicts in mem::replace(&mut record.conflicts, Box::new([])) {
        let root = repositories.get_mut(conflicts.repository).expect("the checkout returns a known repository");
        root.conflicts = conflicts.files;
    }
    let spawn = agent::SpawnV2 {
        workspace: directory,
        charter,
        transcript,
        repositories: repositories.into_boxed(),
        grants: names.into_boxed(),
    };
    route::agent_step(domain, env, agent::Event::SpawnV2 { client: owner, spawn });
}
