//! Bounded people state, request admission and restoration (domain/people.md,
//! sections 3–5 and 12.1). `step` handles root events; `fire` expires sign-ins.
//! The child keeps identities, roles, sign-ins and keyed outcomes, routes each
//! admitted flight once and waits for `Decided`. Root owns policy, task creation
//! and durability; this child never performs IO or retains credential secrets.

use crate::{
    Ask, Event, Fact, Holding, Identity, IdentityKey, InitialOwner, Key, Limits, Outcome, Refusal, Reply, Request,
    RequestKey, Role, Stored,
};
use alloc::boxed::Box;
use skein_lib::{Deadlines, Env, Id, List, Map, Queue, ReplyTo, Slab, Time, Wall};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SignIn {
    person: u64,
    expires: Wall,
    due: Time,
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Answered {
    ask: Ask,
    outcome: Outcome,
    at: Wall,
}

#[derive(Debug)]
pub(crate) struct Pending {
    key: RequestKey,
    ask: Ask,
    replies: List<ReplyTo>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    Restoring,
    Ready,
    Failed,
}

/// Bounded secret-free identities, sign-ins/deadlines, project holdings, completed keys and
/// volatile flights; knows tasks only by returned number and starts in restoring phase.
/// (domain/people.md, sections 2–5 and 12.1).
#[derive(Debug)]
pub struct Domain {
    phase: Phase,
    people: Map<u64, Identity>,
    identities: Map<IdentityKey, u64>,
    sign_ins: Map<u64, SignIn>,
    alarms: Deadlines<u64>,
    roles: Map<u32, Box<[Holding]>>,
    owners: Box<[InitialOwner]>,
    answers: Map<RequestKey, Answered>,
    pending: Slab<Pending>,
    flights: Map<RequestKey, Id<Pending>>,
    facts: Queue<Fact>,
    lost: u64,
}

impl Domain {
    /// Create restoring state with capacities from validated `limits` and at most `limits.initial_owners`
    /// owned bootstrap entries. Panics on unrepresentable/invalid limits, oversized owners or
    /// duplicate project/identity pairs; project roles must be initialized before sign-in.
    /// (domain/people.md, sections 2–5 and 12.1).
    #[must_use]
    pub fn new(limits: &Limits, owners: Box<[InitialOwner]>) -> Domain {
        assert!(crate::worst_case(limits).is_some(), "people limits are valid");
        assert!(
            owners.len() <= usize::try_from(limits.initial_owners).expect("u32 fits usize"),
            "configured owners fit"
        );
        for (at, owner) in owners.iter().enumerate() {
            for earlier in owners.iter().take(at) {
                assert!(
                    owner.project != earlier.project || owner.identity != earlier.identity,
                    "one initial owner identity per project"
                );
            }
        }
        Domain {
            phase: Phase::Restoring,
            people: Map::with_capacity(limits.people),
            identities: Map::with_capacity(limits.people),
            sign_ins: Map::with_capacity(limits.sign_ins),
            alarms: Deadlines::with_capacity(limits.sign_ins),
            roles: Map::with_capacity(limits.projects),
            owners,
            answers: Map::with_capacity(limits.requests),
            pending: Slab::with_capacity(limits.pending),
            flights: Map::with_capacity(limits.pending),
            facts: Queue::with_capacity(limits.facts),
            lost: 0,
        }
    }

    /// Pure scheduler query of the earliest armed monotonic sign-in expiry, or `None`; no
    /// expiration or output occurs here. (domain/people.md, sections 2–5 and 12.1).
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }

    /// Pure scheduler query returning whether any armed expiry is at or before supplied `now`; no
    /// state change or output. (domain/people.md, sections 2–5 and 12.1).
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Reclaim retired flight slots at the parent's iteration reclaim point after their outputs
    /// have been routed; no request or client reply is emitted. (domain/people.md, sections 2–5 and
    /// 12.1).
    pub fn reclaim(&mut self) {
        self.pending.reclaim();
    }

    /// Remove one optional content-free observation from the bounded fact queue; keeping/dropping
    /// facts changes no decision or persistence output. (domain/people.md, sections 2–5 and 12.1).
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }

    /// Return the saturating count of observations dropped when `Limits::facts` was full;
    /// diagnostic only, not a lifecycle acknowledgement. (domain/people.md, sections 2–5 and 12.1).
    #[must_use]
    pub const fn facts_lost(&self) -> u64 {
        self.lost
    }

    /// Pure bounded membership lookup used by root escalation routing and
    /// authenticated named reads; caller separately authenticates identities.
    /// Available during restore, allocating/emitting/mutating nothing
    /// (domain/people.md, sections 4 and 5.1.2).
    #[must_use]
    pub fn role(&self, person: u64, project: u32) -> Option<Role> {
        role(self, person, project)
    }

    /// Pure lookup of whether the bounded role table contains `project`, including an empty
    /// holdings set. Available during restoration so the root preserves restored roles and seeds
    /// only missing bootstrap projects; it implies neither readiness nor person membership.
    /// The table is bounded by `Limits::projects`; this query allocates nothing, emits no output
    /// and grants no role (domain/people.md, sections 3.1 and 4; domain/engine.md, section 5.7).
    #[must_use]
    pub fn has_project(&self, project: u32) -> bool {
        self.roles.contains_key(&project)
    }

    /// Pure authenticated lookup for the root-issued `sign_in` at supplied `now` and `wall`.
    /// Returns `None` before readiness, for an absent sign-in, or when either its projected
    /// monotonic deadline is at or before `now` or its saved wall expiry is at or before `wall`.
    /// A backward wall correction cannot revive a monotonic-expired session, even before fire.
    /// Otherwise returns the stable requester person number. The sign-in table is bounded
    /// by `Limits::sign_ins`; this read allocates nothing, emits no output and changes no
    /// sign-in or role state. Root uses it for named task result reads (domain/people.md,
    /// sections 3 and 6; domain/engine.md, sections 5.7 and 7.5).
    #[must_use]
    pub fn person(&self, sign_in: u64, now: Time, wall: Wall) -> Option<u64> {
        if !self.ready() {
            return None;
        }
        let session = self.sign_ins.get(&sign_in)?;
        if session.expires <= wall || session.due <= now {
            return None;
        }
        Some(session.person)
    }

    /// Pure query of completed, successful restoration; false during restoring and permanently
    /// after restore failure. (domain/people.md, sections 2–5 and 12.1).
    #[must_use]
    pub fn ready(&self) -> bool {
        match self.phase {
            Phase::Ready => true,
            Phase::Restoring | Phase::Failed => false,
        }
    }
}

/// Bootstrap writes one roles record per configured owner match, alongside
/// person/sign-in records and reply; completion answers every bounded waiter.
/// Required free Request slots for one step/fire under validated `limits`: maximum of bootstrap owner
/// matches plus three, waiters plus one and sign-ins plus one. `worst_case` validates those
/// additions before use; caller counts output payload bytes separately. (domain/people.md, sections
/// 2–5 and 12.1).
#[must_use]
pub fn max_out(limits: &Limits) -> u32 {
    limits.initial_owners.saturating_add(3).max(limits.waiters.saturating_add(1)).max(limits.sign_ins.saturating_add(1))
}

/// Apply one root-issued `event` with iteration clocks and immutable configured bounds in `env`;
/// caller reserves at least `max_out(&env.limits)` free `out` slots. Emits typed
/// routing/persistence/replies, never IO; root completes each `Route` once and withholds replies
/// until required atomic writes are durable. (domain/people.md, sections 2–5 and 12.1).
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::ApplyRoles { reply_to, request } => {
            let result = apply_roles(domain, env, request, out);
            out.push(Request::RolesApplied { reply_to, request, result });
        }
        Event::Restore { record } => restore(domain, env, record, out),
        Event::Restored => restored(domain, env, out),
        Event::SignedIn { reply_to, person, sign_in, identity } => {
            signin(domain, env, reply_to, person, sign_in, identity, out);
        }
        Event::SignOut { reply_to, sign_in } => {
            if !domain.ready() {
                return refused(reply_to, Refusal::NotReady, out);
            }
            end_signin(domain, sign_in, out);
            out.push(Request::Reply { to: reply_to, reply: Reply::SignedOut });
        }
        Event::Roles { project, holdings } => {
            if domain.phase == Phase::Failed {
                return;
            }
            match valid_roles(domain, &env.limits, project, &holdings) {
                Ok(()) => {
                    out.push(Request::Save { record: Stored::Roles { project, holdings: holdings.clone() } });
                    let saved = domain.roles.insert(project, holdings);
                    assert!(saved.is_ok(), "roles admitted before mutation");
                }
                Err(refusal) => out.push(Request::RolesRefused { project, refusal }),
            }
        }
        Event::Ask { reply_to, sign_in, key, ask } => admit_ask(domain, env, reply_to, sign_in, key, ask, out),
        Event::Decided { request, outcome } => decided(domain, env, Id::<Pending>::from_token(request), outcome, out),
    }
}

/// One expiration per fire; other expirations stay due for later iterations.
/// Expire at most one due sign-in using `env.now`, emitting its `Erase`; no work before ready. Caller
/// reserves `max_out(&env.limits)` free slots and calls again in later iterations while due; parent
/// owns durability and secret cleanup. (domain/people.md, sections 2–5 and 12.1).
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if !domain.ready() {
        return;
    }
    let Some(number) = domain.alarms.expire(env.now) else {
        return;
    };
    end_signin(domain, number, out);
}

fn fact(domain: &mut Domain, observation: Fact) {
    if domain.facts.try_push(observation).is_err() {
        domain.lost = domain.lost.saturating_add(1);
    }
}

fn refused(to: ReplyTo, refusal: Refusal, out: &mut Queue<Request>) {
    out.push(Request::Reply { to, reply: Reply::Refused(refusal) });
}

fn valid_identity(limits: &Limits, identity: &Identity) -> bool {
    match identity.login.len().checked_add(identity.name.len()) {
        Some(bytes) => bytes <= usize::try_from(limits.identity_bytes).expect("u32 fits usize"),
        None => false,
    }
}

fn apply_roles(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    out: &mut Queue<Request>,
) -> Result<(), Refusal> {
    if !domain.ready() {
        return Err(Refusal::NotReady);
    }
    let flight = domain.pending.get(Id::from_token(request)).ok_or(Refusal::Unknown)?;
    let project = match &flight.ask {
        Ask::SetRoles { project, .. } => *project,
        Ask::StartChat { .. } | Ask::DecideEscalation { .. } => return Err(Refusal::Unknown),
    };
    if !domain.roles.contains_key(&project) {
        return Err(Refusal::Unknown);
    }
    if role(domain, flight.key.person, project) != Some(Role::Owner) {
        return Err(Refusal::Role);
    }
    let holdings = match &flight.ask {
        Ask::SetRoles { holdings, .. } => {
            valid_roles(domain, &env.limits, project, holdings)?;
            for holding in holdings {
                if holding.person == 0 {
                    return Err(Refusal::Limit);
                }
                if !domain.people.contains_key(&holding.person) {
                    return Err(Refusal::Unknown);
                }
            }
            holdings.clone()
        }
        Ask::StartChat { .. } | Ask::DecideEscalation { .. } => unreachable!("validated roster flight"),
    };
    out.push(Request::Save { record: Stored::Roles { project, holdings: holdings.clone() } });
    let saved = domain.roles.insert(project, holdings);
    assert!(saved.is_ok(), "existing role row replaces without consuming capacity");
    Ok(())
}

fn valid_roles(domain: &Domain, limits: &Limits, project: u32, holdings: &[Holding]) -> Result<(), Refusal> {
    if holdings.len() > usize::try_from(limits.holdings).expect("u32 fits usize") {
        return Err(Refusal::Limit);
    }
    if !domain.roles.contains_key(&project) && domain.roles.len() >= limits.projects {
        return Err(Refusal::Busy);
    }
    for (at, holding) in holdings.iter().enumerate() {
        for earlier in holdings.iter().take(at) {
            if earlier.person == holding.person {
                return Err(Refusal::Limit);
            }
        }
    }
    Ok(())
}

fn has_person(holdings: &[Holding], person: u64) -> bool {
    for holding in holdings {
        if holding.person == person {
            return true;
        }
    }
    false
}

fn signin(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    candidate: u64,
    number: u64,
    identity: Identity,
    out: &mut Queue<Request>,
) {
    if !domain.ready() {
        return refused(to, Refusal::NotReady, out);
    }
    if !valid_identity(&env.limits, &identity) {
        return refused(to, Refusal::Limit, out);
    }
    let known = domain.identities.get(&identity.key).copied();
    let person = known.unwrap_or(candidate);
    if let Some(old) = domain.sign_ins.get(&number) {
        if old.person != person {
            return refused(to, Refusal::SignIn, out);
        }
        if old.expires <= env.wall || old.due <= env.now {
            return refused(to, Refusal::SignIn, out);
        }
        return out.push(Request::Reply { to, reply: Reply::SignedIn { person, expires: old.expires } });
    }
    if domain.sign_ins.len() >= env.limits.sign_ins {
        return refused(to, Refusal::Busy, out);
    }
    if known.is_none() {
        if domain.people.len() >= env.limits.people {
            return refused(to, Refusal::Busy, out);
        }
        assert!(!domain.people.contains_key(&person), "root gives a fresh candidate person number");
        // Reserve all bootstrap role slots before making either record.
        for owner in &domain.owners {
            if owner.identity == identity.key {
                let Some(holdings) = domain.roles.get(&owner.project) else {
                    return refused(to, Refusal::Unknown, out);
                };
                if holdings.len() >= usize::try_from(env.limits.holdings).expect("u32 fits usize")
                    && !has_person(holdings, person)
                {
                    return refused(to, Refusal::Busy, out);
                }
            }
        }
    }
    let Some(expires_nanos) = env.wall.as_nanos().checked_add(env.limits.sign_in_lifetime.as_nanos()) else {
        return refused(to, Refusal::Limit, out);
    };
    let expires = Wall::from_nanos(expires_nanos);
    if known.is_none() {
        let indexed = domain.identities.insert(identity.key, person);
        assert!(indexed == Ok(None), "new identity has room");
        // Each project occurs at most once for this identity in configuration.
        for owner in &domain.owners {
            if owner.identity != identity.key {
                continue;
            }
            let old = domain.roles.get(&owner.project).expect("bootstrap project admitted");
            let mut amended = List::with_capacity(env.limits.holdings);
            for holding in &**old {
                if holding.person != person {
                    amended.push(*holding).expect("bounded list room checked");
                }
            }
            amended.push(Holding { person, role: Role::Owner }).expect("bounded list room checked");
            let holdings = amended.into_boxed();
            out.push(Request::Save { record: Stored::Roles { project: owner.project, holdings: holdings.clone() } });
            let saved = domain.roles.insert(owner.project, holdings);
            assert!(saved.is_ok(), "existing bootstrap project");
        }
    }
    let changed = match domain.people.get(&person) {
        Some(old) => old != &identity,
        None => true,
    };
    if changed {
        out.push(Request::Save { record: Stored::Person { number: person, identity: identity.clone() } });
        let saved = domain.people.insert(person, identity);
        assert!(saved.is_ok(), "person admitted before mutation");
    }
    let saved = domain.sign_ins.insert(number, SignIn { person, expires, due: deadline(env, expires) });
    assert!(saved == Ok(None), "sign-in admitted before mutation");
    arm(domain, env, number, expires);
    out.push(Request::Save { record: Stored::SignIn { number, person, expires } });
    out.push(Request::Reply { to, reply: Reply::SignedIn { person, expires } });
    fact(domain, Fact::SignedIn { person, sign_in: number });
}

fn deadline(env: &Env<Limits>, expires: Wall) -> Time {
    let left = skein_lib::Duration::from_nanos(expires.as_nanos().saturating_sub(env.wall.as_nanos()));
    env.now.saturating_add(left)
}

fn arm(domain: &mut Domain, env: &Env<Limits>, number: u64, expires: Wall) {
    let armed = domain.alarms.arm(number, deadline(env, expires));
    assert!(armed.is_ok(), "one alarm per admitted sign-in");
}

fn end_signin(domain: &mut Domain, number: u64, out: &mut Queue<Request>) {
    if domain.sign_ins.remove(&number).is_some() {
        domain.alarms.cancel(number);
        out.push(Request::Erase { key: Key::SignIn(number) });
        fact(domain, Fact::SignedOut { sign_in: number });
    }
}

fn project(ask: &Ask) -> u32 {
    match ask {
        Ask::SetRoles { project, .. } | Ask::StartChat { project, .. } | Ask::DecideEscalation { project, .. } => {
            *project
        }
    }
}

fn valid_ask(limits: &Limits, ask: &Ask) -> bool {
    match ask {
        Ask::SetRoles { holdings, .. } => holdings.len() <= usize::try_from(limits.holdings).expect("u32 fits usize"),
        Ask::StartChat { words, .. } => words.len() <= usize::try_from(limits.words).expect("u32 fits usize"),
        Ask::DecideEscalation { task, revision, decision, .. } => {
            *task != 0
                && *revision != 0
                && match decision {
                    crate::EscalationDecision::Release | crate::EscalationDecision::Pass => true,
                    crate::EscalationDecision::Reject { reason } => {
                        reason.len() <= usize::try_from(limits.words).expect("u32 fits usize")
                    }
                }
        }
    }
}

fn role(domain: &Domain, person: u64, project: u32) -> Option<Role> {
    for holding in &**domain.roles.get(&project)? {
        if holding.person == person {
            return Some(holding.role);
        }
    }
    None
}

fn admit_ask(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    sign_in: u64,
    bytes: [u8; 16],
    ask: Ask,
    out: &mut Queue<Request>,
) {
    if !domain.ready() {
        return refused(to, Refusal::NotReady, out);
    }
    let Some(session) = domain.sign_ins.get(&sign_in) else {
        return refused(to, Refusal::SignIn, out);
    };
    if session.expires <= env.wall || session.due <= env.now {
        return refused(to, Refusal::SignIn, out);
    }
    let key = RequestKey { person: session.person, key: bytes };
    if !valid_ask(&env.limits, &ask) {
        return refused(to, Refusal::Limit, out);
    }
    if let Some(answer) = domain.answers.get(&key) {
        if answer.ask != ask {
            return refused(to, Refusal::KeyConflict, out);
        }
        return out.push(Request::Reply { to, reply: Reply::Outcome(answer.outcome) });
    }
    if let Some(id) = domain.flights.get(&key).copied() {
        let flight = domain.pending.get_mut(id).expect("key index names live pending request");
        if flight.ask != ask {
            return refused(to, Refusal::KeyConflict, out);
        }
        if flight.replies.len() >= env.limits.waiters {
            return refused(to, Refusal::Busy, out);
        }
        flight.replies.push(to).expect("bounded list room checked");
        return;
    }
    // An accepted flight reserves one answered record, even while pending.
    if domain.answers.len().saturating_add(domain.flights.len()) >= env.limits.requests {
        return refused(to, Refusal::Busy, out);
    }
    let project = project(&ask);
    let role = role(domain, key.person, project);
    let refusal = match &ask {
        Ask::SetRoles { .. } => {
            if !domain.roles.contains_key(&project) {
                Some(Refusal::Unknown)
            } else if role == Some(Role::Owner) {
                None
            } else {
                Some(Refusal::Role)
            }
        }
        Ask::StartChat { .. } => match role {
            Some(Role::Owner | Role::Maintainer | Role::Member) => None,
            Some(Role::Observer) | None => Some(Refusal::Role),
        },
        Ask::DecideEscalation { .. } => None,
    };
    if let Some(refusal) = refusal {
        let outcome = Outcome::Refused(refusal);
        save_answer(domain, env, key, ask, outcome, out);
        return out.push(Request::Reply { to, reply: Reply::Outcome(outcome) });
    }
    if domain.pending.is_full() {
        return refused(to, Refusal::Busy, out);
    }
    let mut replies = List::with_capacity(env.limits.waiters);
    replies.push(to).expect("bounded list room checked");
    let flight = Pending { key, ask: ask.clone(), replies };
    let id = domain.pending.insert(flight).expect("pending request admitted");
    let indexed = domain.flights.insert(key, id);
    assert!(indexed == Ok(None), "one key per flight within pending capacity");
    out.push(Request::Route { request: id.token(), person: key.person, project, role, ask });
    fact(domain, Fact::Routed { person: key.person });
}

fn save_answer(
    domain: &mut Domain,
    env: &Env<Limits>,
    key: RequestKey,
    ask: Ask,
    outcome: Outcome,
    out: &mut Queue<Request>,
) {
    out.push(Request::Save { record: Stored::Answer { key, ask: ask.clone(), outcome, at: env.wall } });
    let saved = domain.answers.insert(key, Answered { ask, outcome, at: env.wall });
    assert!(saved.is_ok(), "answer room reserved at request entrance");
    fact(domain, Fact::Answered { person: key.person });
}

fn decided(domain: &mut Domain, env: &Env<Limits>, id: Id<Pending>, outcome: Outcome, out: &mut Queue<Request>) {
    let flight = domain.pending.get_mut(id).expect("root returns each routed request exactly once");
    let key = flight.key;
    let ask = flight.ask.clone();
    let replies = core::mem::replace(&mut flight.replies, List::with_capacity(0));
    domain.pending.retire(id);
    let removed = domain.flights.remove(&key);
    assert!(removed == Some(id), "flight has one key index");
    match outcome {
        // A refused admission is retryable with the same key, including
        // pressure reported by tasks or another child through the root.
        Outcome::Refused(Refusal::Busy | Refusal::NotReady) => {}
        Outcome::RolesSet { .. }
        | Outcome::Started { .. }
        | Outcome::EscalationDecided { .. }
        | Outcome::Refused(
            Refusal::NoFurther
            | Refusal::NeedsAmend
            | Refusal::Standing
            | Refusal::SignIn
            | Refusal::Role
            | Refusal::Authority
            | Refusal::Unknown
            | Refusal::Ended
            | Refusal::Limit
            | Refusal::KeyConflict,
        ) => save_answer(domain, env, key, ask, outcome, out),
    }
    for to in replies.into_boxed() {
        out.push(Request::Reply { to, reply: Reply::Outcome(outcome) });
    }
}

fn restore(domain: &mut Domain, env: &Env<Limits>, record: Stored, out: &mut Queue<Request>) {
    match domain.phase {
        Phase::Ready => return restore_failed(domain, record.key(), Refusal::NotReady, out),
        Phase::Failed => return,
        Phase::Restoring => {}
    }
    let key = record.key();
    let valid = match &record {
        Stored::Person { number, identity } => {
            valid_identity(&env.limits, identity)
                && !domain.people.contains_key(number)
                && !domain.identities.contains_key(&identity.key)
                && domain.people.len() < env.limits.people
        }
        Stored::SignIn { number, .. } => {
            !domain.sign_ins.contains_key(number) && domain.sign_ins.len() < env.limits.sign_ins
        }
        Stored::Roles { project, holdings } => {
            valid_roles(domain, &env.limits, *project, holdings).is_ok() && !domain.roles.contains_key(project)
        }
        Stored::Answer { key, ask, .. } => {
            valid_ask(&env.limits, ask)
                && !domain.answers.contains_key(key)
                && domain.answers.len() < env.limits.requests
        }
    };
    if !valid {
        return restore_failed(domain, key, Refusal::Limit, out);
    }
    match record {
        Stored::Person { number, identity } => {
            let indexed = domain.identities.insert(identity.key, number);
            assert!(indexed == Ok(None), "restored identity admitted");
            let saved = domain.people.insert(number, identity);
            assert!(saved.is_ok(), "restored person admitted");
        }
        Stored::SignIn { number, person, expires } => {
            let saved = domain.sign_ins.insert(number, SignIn { person, expires, due: deadline(env, expires) });
            assert!(saved == Ok(None), "restored sign-in admitted");
        }
        Stored::Roles { project, holdings } => {
            let saved = domain.roles.insert(project, holdings);
            assert!(saved.is_ok(), "restored roles admitted");
        }
        Stored::Answer { key, ask, outcome, at } => {
            let saved = domain.answers.insert(key, Answered { ask, outcome, at });
            assert!(saved.is_ok(), "restored answer admitted");
        }
    }
}

fn restore_failed(domain: &mut Domain, key: Key, refusal: Refusal, out: &mut Queue<Request>) {
    domain.phase = Phase::Failed;
    out.push(Request::RestoreRefused { key, refusal });
}

fn restored(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    match domain.phase {
        Phase::Ready | Phase::Failed => return,
        Phase::Restoring => {}
    }
    // Restore order is immaterial; validate references once all rows arrived.
    for (number, signin) in &domain.sign_ins {
        if !domain.people.contains_key(&signin.person) {
            return restore_failed(domain, Key::SignIn(*number), Refusal::Unknown, out);
        }
    }
    for (project, holdings) in &domain.roles {
        for holding in &**holdings {
            if !domain.people.contains_key(&holding.person) {
                return restore_failed(domain, Key::Roles(*project), Refusal::Unknown, out);
            }
        }
    }
    for (key, _) in &domain.answers {
        if !domain.people.contains_key(&key.person) {
            return restore_failed(domain, Key::Answer(*key), Refusal::Unknown, out);
        }
    }
    domain.phase = Phase::Ready;
    // A bounded snapshot lets cleanup and projection mutate the table.
    let mut recovered = List::with_capacity(env.limits.sign_ins);
    for (number, signin) in &domain.sign_ins {
        recovered.push((*number, *signin)).expect("one snapshot row per admitted sign-in");
    }
    for (number, mut signin) in recovered.into_boxed() {
        if signin.expires <= env.wall {
            end_signin(domain, number, out);
        } else {
            signin.due = deadline(env, signin.expires);
            let saved = domain.sign_ins.insert(number, signin);
            assert!(saved.is_ok(), "restored sign-in already exists");
            arm(domain, env, number, signin.expires);
        }
    }
}
