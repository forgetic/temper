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
    #[must_use]
    pub fn new(l: &Limits, owners: Box<[InitialOwner]>) -> Domain {
        assert!(crate::worst_case(l).is_some(), "people limits are valid");
        assert!(owners.len() <= usize::try_from(l.initial_owners).expect("u32 fits usize"), "configured owners fit");
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
            people: Map::with_capacity(l.people),
            identities: Map::with_capacity(l.people),
            sign_ins: Map::with_capacity(l.sign_ins),
            alarms: Deadlines::with_capacity(l.sign_ins),
            roles: Map::with_capacity(l.projects),
            owners,
            answers: Map::with_capacity(l.requests),
            pending: Slab::with_capacity(l.pending),
            flights: Map::with_capacity(l.pending),
            facts: Queue::with_capacity(l.facts),
            lost: 0,
        }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.alarms.next()
    }
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.alarms.next() {
            Some(at) => at <= now,
            None => false,
        }
    }
    pub fn reclaim(&mut self) {
        self.pending.reclaim();
    }
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }
    #[must_use]
    pub const fn facts_lost(&self) -> u64 {
        self.lost
    }
    /// Root checks whether startup loaded a project's durable roles before
    /// seeding its empty bootstrap table (domain/people.md, sections 3.1 and 4).
    /// This read grants no role and changes nothing; sign-in applies owners.
    #[must_use]
    pub fn has_project(&self, project: u32) -> bool {
        self.roles.contains_key(&project)
    }

    /// Root reads an authenticated unexpired session to derive a task result
    /// for its requester (domain/people.md, sections 3 and 6). This read changes
    /// no session, role or inbox state; unknown/expired sign-ins are refused.
    #[must_use]
    pub fn person(&self, sign_in: u64, wall: Wall) -> Option<u64> {
        if !self.ready() {
            return None;
        }
        let session = self.sign_ins.get(&sign_in)?;
        if session.expires <= wall {
            return None;
        }
        Some(session.person)
    }
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
#[must_use]
pub fn max_out(l: &Limits) -> u32 {
    l.initial_owners.saturating_add(3).max(l.waiters.saturating_add(1)).max(l.sign_ins.saturating_add(1))
}
pub fn step(d: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Restore { record } => restore(d, env, record, out),
        Event::Restored => restored(d, env, out),
        Event::SignedIn { reply_to, person, sign_in, identity } => {
            signin(d, env, reply_to, person, sign_in, identity, out);
        }
        Event::SignOut { reply_to, sign_in } => {
            if !d.ready() {
                return refused(reply_to, Refusal::NotReady, out);
            }
            end_signin(d, sign_in, out);
            out.push(Request::Reply { to: reply_to, reply: Reply::SignedOut });
        }
        Event::Roles { project, holdings } => {
            if d.phase == Phase::Failed {
                return;
            }
            match valid_roles(d, &env.limits, project, &holdings) {
                Ok(()) => {
                    out.push(Request::Save { record: Stored::Roles { project, holdings: holdings.clone() } });
                    let saved = d.roles.insert(project, holdings);
                    assert!(saved.is_ok(), "roles admitted before mutation");
                }
                Err(refusal) => out.push(Request::RolesRefused { project, refusal }),
            }
        }
        Event::Ask { reply_to, sign_in, key, ask } => admit_ask(d, env, reply_to, sign_in, key, ask, out),
        Event::Decided { request, outcome } => decided(d, env, Id::<Pending>::from_token(request), outcome, out),
    }
}
/// One expiration per fire; other expirations stay due for later iterations.
pub fn fire(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if !d.ready() {
        return;
    }
    let Some(number) = d.alarms.expire(env.now) else {
        return;
    };
    end_signin(d, number, out);
}
fn fact(d: &mut Domain, observation: Fact) {
    if d.facts.try_push(observation).is_err() {
        d.lost = d.lost.saturating_add(1);
    }
}
fn refused(to: ReplyTo, refusal: Refusal, out: &mut Queue<Request>) {
    out.push(Request::Reply { to, reply: Reply::Refused(refusal) });
}
fn valid_identity(l: &Limits, identity: &Identity) -> bool {
    match identity.login.len().checked_add(identity.name.len()) {
        Some(bytes) => bytes <= usize::try_from(l.identity_bytes).expect("u32 fits usize"),
        None => false,
    }
}
fn valid_roles(d: &Domain, l: &Limits, project: u32, holdings: &[Holding]) -> Result<(), Refusal> {
    if holdings.len() > usize::try_from(l.holdings).expect("u32 fits usize") {
        return Err(Refusal::Limit);
    }
    if !d.roles.contains_key(&project) && d.roles.len() >= l.projects {
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
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    candidate: u64,
    number: u64,
    identity: Identity,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, Refusal::NotReady, out);
    }
    if !valid_identity(&env.limits, &identity) {
        return refused(to, Refusal::Limit, out);
    }
    let known = d.identities.get(&identity.key).copied();
    let person = known.unwrap_or(candidate);
    if let Some(old) = d.sign_ins.get(&number) {
        if old.person != person {
            return refused(to, Refusal::SignIn, out);
        }
        if old.expires <= env.wall || old.due <= env.now {
            return refused(to, Refusal::SignIn, out);
        }
        return out.push(Request::Reply { to, reply: Reply::SignedIn { person, expires: old.expires } });
    }
    if d.sign_ins.len() >= env.limits.sign_ins {
        return refused(to, Refusal::Busy, out);
    }
    if known.is_none() {
        if d.people.len() >= env.limits.people {
            return refused(to, Refusal::Busy, out);
        }
        assert!(!d.people.contains_key(&person), "root gives a fresh candidate person number");
        // Reserve all bootstrap role slots before making either record.
        for owner in &d.owners {
            if owner.identity == identity.key {
                let Some(holdings) = d.roles.get(&owner.project) else {
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
        let indexed = d.identities.insert(identity.key, person);
        assert!(indexed == Ok(None), "new identity has room");
        // Each project occurs at most once for this identity in configuration.
        for owner in &d.owners {
            if owner.identity != identity.key {
                continue;
            }
            let old = d.roles.get(&owner.project).expect("bootstrap project admitted");
            let mut amended = List::with_capacity(env.limits.holdings);
            for holding in &**old {
                if holding.person != person {
                    amended.push(*holding).expect("bounded list room checked");
                }
            }
            amended.push(Holding { person, role: Role::Owner }).expect("bounded list room checked");
            let holdings = amended.into_boxed();
            out.push(Request::Save { record: Stored::Roles { project: owner.project, holdings: holdings.clone() } });
            let saved = d.roles.insert(owner.project, holdings);
            assert!(saved.is_ok(), "existing bootstrap project");
        }
    }
    let changed = match d.people.get(&person) {
        Some(old) => old != &identity,
        None => true,
    };
    if changed {
        out.push(Request::Save { record: Stored::Person { number: person, identity: identity.clone() } });
        let saved = d.people.insert(person, identity);
        assert!(saved.is_ok(), "person admitted before mutation");
    }
    let saved = d.sign_ins.insert(number, SignIn { person, expires, due: deadline(env, expires) });
    assert!(saved == Ok(None), "sign-in admitted before mutation");
    arm(d, env, number, expires);
    out.push(Request::Save { record: Stored::SignIn { number, person, expires } });
    out.push(Request::Reply { to, reply: Reply::SignedIn { person, expires } });
    fact(d, Fact::SignedIn { person, sign_in: number });
}
fn deadline(env: &Env<Limits>, expires: Wall) -> Time {
    let left = skein_lib::Duration::from_nanos(expires.as_nanos().saturating_sub(env.wall.as_nanos()));
    env.now.saturating_add(left)
}
fn arm(d: &mut Domain, env: &Env<Limits>, number: u64, expires: Wall) {
    let armed = d.alarms.arm(number, deadline(env, expires));
    assert!(armed.is_ok(), "one alarm per admitted sign-in");
}
fn end_signin(d: &mut Domain, number: u64, out: &mut Queue<Request>) {
    if d.sign_ins.remove(&number).is_some() {
        d.alarms.cancel(number);
        out.push(Request::Erase { key: Key::SignIn(number) });
        fact(d, Fact::SignedOut { sign_in: number });
    }
}
fn project(ask: &Ask) -> u32 {
    match ask {
        Ask::StartChat { project, .. } => *project,
    }
}
fn valid_ask(l: &Limits, ask: &Ask) -> bool {
    match ask {
        Ask::StartChat { words, .. } => words.len() <= usize::try_from(l.words).expect("u32 fits usize"),
    }
}
fn role(d: &Domain, person: u64, project: u32) -> Option<Role> {
    for holding in &**d.roles.get(&project)? {
        if holding.person == person {
            return Some(holding.role);
        }
    }
    None
}
fn admit_ask(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    sign_in: u64,
    bytes: [u8; 16],
    ask: Ask,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, Refusal::NotReady, out);
    }
    let Some(session) = d.sign_ins.get(&sign_in) else {
        return refused(to, Refusal::SignIn, out);
    };
    if session.expires <= env.wall || session.due <= env.now {
        return refused(to, Refusal::SignIn, out);
    }
    let key = RequestKey { person: session.person, key: bytes };
    if !valid_ask(&env.limits, &ask) {
        return refused(to, Refusal::Limit, out);
    }
    if let Some(answer) = d.answers.get(&key) {
        if answer.ask != ask {
            return refused(to, Refusal::KeyConflict, out);
        }
        return out.push(Request::Reply { to, reply: Reply::Outcome(answer.outcome) });
    }
    if let Some(id) = d.flights.get(&key).copied() {
        let flight = d.pending.get_mut(id).expect("key index names live pending request");
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
    if d.answers.len().saturating_add(d.flights.len()) >= env.limits.requests {
        return refused(to, Refusal::Busy, out);
    }
    let project = project(&ask);
    let role = role(d, key.person, project);
    let refusal = match role {
        Some(Role::Owner | Role::Maintainer | Role::Member) => None,
        Some(Role::Observer) | None => Some(Refusal::Role),
    };
    if let Some(refusal) = refusal {
        let outcome = Outcome::Refused(refusal);
        save_answer(d, env, key, ask, outcome, out);
        return out.push(Request::Reply { to, reply: Reply::Outcome(outcome) });
    }
    if d.pending.is_full() {
        return refused(to, Refusal::Busy, out);
    }
    let mut replies = List::with_capacity(env.limits.waiters);
    replies.push(to).expect("bounded list room checked");
    let flight = Pending { key, ask: ask.clone(), replies };
    let id = d.pending.insert(flight).expect("pending request admitted");
    let indexed = d.flights.insert(key, id);
    assert!(indexed == Ok(None), "one key per flight within pending capacity");
    out.push(Request::Route {
        request: id.token(),
        person: key.person,
        project,
        role: role.expect("role checked"),
        ask,
    });
    fact(d, Fact::Routed { person: key.person });
}
fn save_answer(
    d: &mut Domain,
    env: &Env<Limits>,
    key: RequestKey,
    ask: Ask,
    outcome: Outcome,
    out: &mut Queue<Request>,
) {
    out.push(Request::Save { record: Stored::Answer { key, ask: ask.clone(), outcome, at: env.wall } });
    let saved = d.answers.insert(key, Answered { ask, outcome, at: env.wall });
    assert!(saved.is_ok(), "answer room reserved at request entrance");
    fact(d, Fact::Answered { person: key.person });
}
fn decided(d: &mut Domain, env: &Env<Limits>, id: Id<Pending>, outcome: Outcome, out: &mut Queue<Request>) {
    let flight = d.pending.get_mut(id).expect("root returns each routed request exactly once");
    let key = flight.key;
    let ask = flight.ask.clone();
    let replies = core::mem::replace(&mut flight.replies, List::with_capacity(0));
    d.pending.retire(id);
    let removed = d.flights.remove(&key);
    assert!(removed == Some(id), "flight has one key index");
    match outcome {
        // A refused admission is retryable with the same key, including
        // pressure reported by tasks or another child through the root.
        Outcome::Refused(Refusal::Busy | Refusal::NotReady) => {}
        Outcome::Started { .. }
        | Outcome::Refused(
            Refusal::SignIn
            | Refusal::Role
            | Refusal::Authority
            | Refusal::Unknown
            | Refusal::Ended
            | Refusal::Limit
            | Refusal::KeyConflict,
        ) => save_answer(d, env, key, ask, outcome, out),
    }
    for to in replies.into_boxed() {
        out.push(Request::Reply { to, reply: Reply::Outcome(outcome) });
    }
}
fn restore(d: &mut Domain, env: &Env<Limits>, record: Stored, out: &mut Queue<Request>) {
    match d.phase {
        Phase::Ready => return restore_failed(d, record.key(), Refusal::NotReady, out),
        Phase::Failed => return,
        Phase::Restoring => {}
    }
    let key = record.key();
    let valid = match &record {
        Stored::Person { number, identity } => {
            valid_identity(&env.limits, identity)
                && !d.people.contains_key(number)
                && !d.identities.contains_key(&identity.key)
                && d.people.len() < env.limits.people
        }
        Stored::SignIn { number, .. } => !d.sign_ins.contains_key(number) && d.sign_ins.len() < env.limits.sign_ins,
        Stored::Roles { project, holdings } => {
            valid_roles(d, &env.limits, *project, holdings).is_ok() && !d.roles.contains_key(project)
        }
        Stored::Answer { key, ask, .. } => {
            valid_ask(&env.limits, ask) && !d.answers.contains_key(key) && d.answers.len() < env.limits.requests
        }
    };
    if !valid {
        return restore_failed(d, key, Refusal::Limit, out);
    }
    match record {
        Stored::Person { number, identity } => {
            let indexed = d.identities.insert(identity.key, number);
            assert!(indexed == Ok(None), "restored identity admitted");
            let saved = d.people.insert(number, identity);
            assert!(saved.is_ok(), "restored person admitted");
        }
        Stored::SignIn { number, person, expires } => {
            let saved = d.sign_ins.insert(number, SignIn { person, expires, due: deadline(env, expires) });
            assert!(saved == Ok(None), "restored sign-in admitted");
        }
        Stored::Roles { project, holdings } => {
            let saved = d.roles.insert(project, holdings);
            assert!(saved.is_ok(), "restored roles admitted");
        }
        Stored::Answer { key, ask, outcome, at } => {
            let saved = d.answers.insert(key, Answered { ask, outcome, at });
            assert!(saved.is_ok(), "restored answer admitted");
        }
    }
}
fn restore_failed(d: &mut Domain, key: Key, refusal: Refusal, out: &mut Queue<Request>) {
    d.phase = Phase::Failed;
    out.push(Request::RestoreRefused { key, refusal });
}
fn restored(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    match d.phase {
        Phase::Ready | Phase::Failed => return,
        Phase::Restoring => {}
    }
    // Restore order is immaterial; validate references once all rows arrived.
    for (number, signin) in &d.sign_ins {
        if !d.people.contains_key(&signin.person) {
            return restore_failed(d, Key::SignIn(*number), Refusal::Unknown, out);
        }
    }
    for (project, holdings) in &d.roles {
        for holding in &**holdings {
            if !d.people.contains_key(&holding.person) {
                return restore_failed(d, Key::Roles(*project), Refusal::Unknown, out);
            }
        }
    }
    for (key, _) in &d.answers {
        if !d.people.contains_key(&key.person) {
            return restore_failed(d, Key::Answer(*key), Refusal::Unknown, out);
        }
    }
    d.phase = Phase::Ready;
    // A bounded snapshot lets cleanup and projection mutate the table.
    let mut recovered = List::with_capacity(env.limits.sign_ins);
    for (number, signin) in &d.sign_ins {
        recovered.push((*number, *signin)).expect("one snapshot row per admitted sign-in");
    }
    for (number, mut signin) in recovered.into_boxed() {
        if signin.expires <= env.wall {
            end_signin(d, number, out);
        } else {
            signin.due = deadline(env, signin.expires);
            let saved = d.sign_ins.insert(number, signin);
            assert!(saved.is_ok(), "restored sign-in already exists");
            arm(d, env, number, signin.expires);
        }
    }
}
