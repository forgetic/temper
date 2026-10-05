#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    reason = "step tests are ordinary Rust (testing-strategy.md, section 4)"
)]
use crate::*;
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
const LIMITS: Limits = Limits {
    people: 4,
    sign_ins: 4,
    projects: 3,
    holdings: 4,
    initial_owners: 3,
    requests: 4,
    pending: 2,
    waiters: 2,
    identity_bytes: 16,
    words: 8,
    sign_in_lifetime: Duration::from_secs(60),
    facts: 8,
};
fn identity(forge: u32, user: u64) -> Identity {
    Identity { key: IdentityKey { forge, user }, login: Box::new([b'l']), name: Box::new([b'n']) }
}
fn ask(project: u32) -> Ask {
    Ask::StartChat { project, words: Box::new([1]) }
}
struct Test {
    d: Domain,
    env: Env<Limits>,
    serial: u64,
}
impl Test {
    fn new(limits: Limits) -> Test {
        let mut test = Test {
            d: Domain::new(&limits, Box::new([])),
            env: Env { limits, now: Time::ZERO, wall: Wall::EPOCH },
            serial: 0,
        };
        assert!(test.send(Event::Restored).is_empty());
        test
    }
    fn to(&mut self) -> ReplyTo {
        self.serial += 1;
        ReplyTo::new(Token::new(self.serial))
    }
    fn send(&mut self, event: Event) -> Vec<Request> {
        let mut out = Queue::with_capacity(max_out(&self.env.limits));
        step(&mut self.d, &self.env, event, &mut out);
        let mut requests = Vec::new();
        while let Some(request) = out.pop() {
            requests.push(request);
        }
        self.d.reclaim();
        requests
    }
    fn signin(&mut self, person: u64, sign_in: u64, identity: Identity) -> Vec<Request> {
        let reply_to = self.to();
        self.send(Event::SignedIn { reply_to, person, sign_in, identity })
    }
    fn request(&mut self, sign_in: u64, key: u8, ask: Ask) -> Vec<Request> {
        let reply_to = self.to();
        self.send(Event::Ask { reply_to, sign_in, key: [key; 16], ask })
    }
    fn member(&mut self) {
        self.signin(1, 10, identity(0, 1));
        self.send(Event::Roles { project: 1, holdings: Box::new([Holding { person: 1, role: Role::Member }]) });
    }
}
fn reply(rows: &[Request]) -> Reply {
    rows.iter()
        .find_map(|r| match r {
            Request::Reply { reply, .. } => Some(*reply),
            Request::Save { .. }
            | Request::Erase { .. }
            | Request::Route { .. }
            | Request::RolesRefused { .. }
            | Request::RestoreRefused { .. } => None,
        })
        .expect("reply emitted")
}
fn route(rows: &[Request]) -> Token {
    rows.iter()
        .find_map(|r| match r {
            Request::Route { request, .. } => Some(*request),
            Request::Save { .. }
            | Request::Erase { .. }
            | Request::Reply { .. }
            | Request::RolesRefused { .. }
            | Request::RestoreRefused { .. } => None,
        })
        .expect("route emitted")
}
#[test]
fn identities_are_forge_and_user_and_existing_people_keep_their_number() {
    let mut test = Test::new(LIMITS);
    assert_eq!(signed_person(reply(&test.signin(1, 10, identity(0, 1)))), Some(1));
    assert_eq!(signed_person(reply(&test.signin(2, 11, identity(0, 1)))), Some(1));
    assert_eq!(signed_person(reply(&test.signin(3, 12, identity(1, 1)))), Some(3));
    assert_eq!(reply(&test.signin(4, 10, identity(0, 4))), Reply::Refused(Refusal::SignIn));
}
#[test]
fn all_roles_are_checked_and_refusals_are_saved() {
    for role in [Role::Owner, Role::Maintainer, Role::Member, Role::Observer] {
        let mut test = Test::new(LIMITS);
        test.member();
        test.send(Event::Roles { project: 1, holdings: Box::new([Holding { person: 1, role }]) });
        let rows = test.request(10, 1, ask(1));
        if role == Role::Observer {
            assert_eq!(reply(&rows), Reply::Outcome(Outcome::Refused(Refusal::Role)));
            assert!(rows.iter().any(is_answer));
        } else {
            route(&rows);
        }
    }
    let mut test = Test::new(LIMITS);
    test.member();
    assert_eq!(reply(&test.request(10, 1, ask(2))), Reply::Outcome(Outcome::Refused(Refusal::Role)));
}
#[test]
fn pending_duplicates_share_one_route_and_reserve_answer_room() {
    let mut test = Test::new(Limits { requests: 1, ..LIMITS });
    test.member();
    let routed = test.request(10, 1, ask(1));
    let token = route(&routed);
    assert!(test.request(10, 1, ask(1)).is_empty());
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Refused(Refusal::Busy));
    assert_eq!(reply(&test.request(10, 2, ask(1))), Reply::Refused(Refusal::Busy));
    assert_eq!(reply(&test.request(10, 1, ask(2))), Reply::Refused(Refusal::KeyConflict));
    let completed = test.send(Event::Decided { request: token, outcome: Outcome::Started { task: 90 } });
    assert_eq!(completed.iter().filter(|r| is_reply(r)).count(), 2);
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Outcome(Outcome::Started { task: 90 }));
}
#[test]
fn answered_keys_follow_people_across_signins_and_role_changes() {
    let mut test = Test::new(LIMITS);
    test.member();
    let rows = test.request(10, 1, ask(1));
    let token = route(&rows);
    test.send(Event::Decided { request: token, outcome: Outcome::Started { task: 90 } });
    let reply_to = test.to();
    test.send(Event::SignOut { reply_to, sign_in: 10 });
    test.signin(2, 11, identity(0, 1));
    test.send(Event::Roles { project: 1, holdings: Box::new([Holding { person: 1, role: Role::Observer }]) });
    assert_eq!(reply(&test.request(11, 1, ask(1))), Reply::Outcome(Outcome::Started { task: 90 }));
    assert_eq!(reply(&test.request(11, 1, ask(2))), Reply::Refused(Refusal::KeyConflict));
}
#[test]
fn every_admission_point_refuses_without_partial_state() {
    let mut test = Test::new(Limits { people: 1, sign_ins: 1, projects: 1, holdings: 1, words: 1, ..LIMITS });
    test.member();
    assert_eq!(reply(&test.signin(2, 11, identity(0, 2))), Reply::Refused(Refusal::Busy));
    let reply_to = test.to();
    test.send(Event::SignOut { reply_to, sign_in: 10 });
    assert_eq!(reply(&test.signin(2, 11, identity(0, 2))), Reply::Refused(Refusal::Busy));
    assert_eq!(signed_person(reply(&test.signin(3, 11, identity(0, 1)))), Some(1));
    let big = Ask::StartChat { project: 1, words: Box::new([1, 2]) };
    assert_eq!(reply(&test.request(11, 1, big)), Reply::Refused(Refusal::Limit));
    let roles = test.send(Event::Roles { project: 2, holdings: Box::new([]) });
    assert_eq!(roles, [Request::RolesRefused { project: 2, refusal: Refusal::Busy }]);
    let roles = test.send(Event::Roles {
        project: 1,
        holdings: Box::new([Holding { person: 1, role: Role::Owner }, Holding { person: 2, role: Role::Member }]),
    });
    assert_eq!(roles, [Request::RolesRefused { project: 1, refusal: Refusal::Limit }]);
}
#[test]
fn expiry_is_monotonic_even_when_wall_clock_moves_back() {
    let mut test = Test::new(LIMITS);
    test.member();
    test.env.now = Time::from_nanos(Duration::from_secs(60).as_nanos());
    test.env.wall = Wall::EPOCH;
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Refused(Refusal::SignIn));
    let mut out = Queue::with_capacity(max_out(&LIMITS));
    fire(&mut test.d, &test.env, &mut out);
    assert_eq!(out.pop(), Some(Request::Erase { key: Key::SignIn(10) }));
}
#[test]
fn result_query_checks_monotonic_expiry_before_fire_after_backward_wall_jump() {
    let mut test = Test::new(LIMITS);
    test.env.wall = Wall::from_nanos(Duration::from_secs(100).as_nanos());
    test.member();
    assert_eq!(test.d.person(10, test.env.now, test.env.wall), Some(1));
    test.env.now = Time::from_nanos(Duration::from_secs(60).as_nanos());
    test.env.wall = Wall::EPOCH;
    assert_eq!(test.d.person(10, test.env.now, test.env.wall), None);
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Refused(Refusal::SignIn));
}
#[test]
fn restore_order_is_independent_and_bad_or_oversized_records_refuse_start() {
    let mut test = Test::new(LIMITS);
    test.d = Domain::new(&LIMITS, Box::new([]));
    test.send(Event::Restore {
        record: Stored::SignIn { number: 10, person: 1, expires: Wall::from_nanos(Duration::from_secs(60).as_nanos()) },
    });
    test.send(Event::Restore { record: Stored::Person { number: 1, identity: identity(0, 1) } });
    test.send(Event::Restored);
    assert!(test.d.ready());
    test.d = Domain::new(&LIMITS, Box::new([]));
    test.send(Event::Restore { record: Stored::SignIn { number: 10, person: 99, expires: Wall::EPOCH } });
    assert_eq!(
        test.send(Event::Restored),
        [Request::RestoreRefused { key: Key::SignIn(10), refusal: Refusal::Unknown }]
    );
    assert!(!test.d.ready());
    test.env.limits.people = 0;
    test.d = Domain::new(&test.env.limits, Box::new([]));
    assert_eq!(
        test.send(Event::Restore { record: Stored::Person { number: 1, identity: identity(0, 1) } }),
        [Request::RestoreRefused { key: Key::Person(1), refusal: Refusal::Limit }]
    );
}
#[test]
fn initial_owners_bootstrap_all_projects_atomically_and_never_regrant() {
    let owners = Box::new([
        InitialOwner { project: 1, identity: IdentityKey { forge: 0, user: 1 } },
        InitialOwner { project: 2, identity: IdentityKey { forge: 0, user: 1 } },
    ]);
    let mut test = Test::new(Limits { holdings: 1, ..LIMITS });
    test.d = Domain::new(&test.env.limits, owners);
    test.send(Event::Roles { project: 1, holdings: Box::new([]) });
    test.send(Event::Roles { project: 2, holdings: Box::new([Holding { person: 9, role: Role::Member }]) });
    test.send(Event::Restore { record: Stored::Person { number: 9, identity: identity(0, 9) } });
    test.send(Event::Restored);
    let refused = test.signin(1, 10, identity(0, 1));
    assert_eq!(reply(&refused), Reply::Refused(Refusal::Busy));
    assert_eq!(refused.len(), 1, "bootstrap refusal makes no record");
    test.send(Event::Roles { project: 2, holdings: Box::new([]) });
    let made = test.signin(2, 10, identity(0, 1));
    assert_eq!(made.iter().filter(|r| is_roles(r)).count(), 2);
    test.send(Event::Roles { project: 1, holdings: Box::new([Holding { person: 2, role: Role::Observer }]) });
    let again = test.signin(3, 11, identity(0, 1));
    assert!(!again.iter().any(is_roles));
    assert_eq!(reply(&test.request(11, 1, ask(1))), Reply::Outcome(Outcome::Refused(Refusal::Role)));
}

fn signed_person(reply: Reply) -> Option<u64> {
    match reply {
        Reply::SignedIn { person, .. } => Some(person),
        Reply::SignedOut | Reply::Outcome(_) | Reply::Refused(_) => None,
    }
}
fn is_reply(request: &Request) -> bool {
    match request {
        Request::Reply { .. } => true,
        Request::Save { .. }
        | Request::Erase { .. }
        | Request::Route { .. }
        | Request::RolesRefused { .. }
        | Request::RestoreRefused { .. } => false,
    }
}
fn is_answer(request: &Request) -> bool {
    match request {
        Request::Save { record } => match record {
            Stored::Answer { .. } => true,
            Stored::Person { .. } | Stored::SignIn { .. } | Stored::Roles { .. } => false,
        },
        Request::Reply { .. }
        | Request::Erase { .. }
        | Request::Route { .. }
        | Request::RolesRefused { .. }
        | Request::RestoreRefused { .. } => false,
    }
}
fn is_roles(request: &Request) -> bool {
    match request {
        Request::Save { record } => match record {
            Stored::Roles { .. } => true,
            Stored::Person { .. } | Stored::SignIn { .. } | Stored::Answer { .. } => false,
        },
        Request::Reply { .. }
        | Request::Erase { .. }
        | Request::Route { .. }
        | Request::RolesRefused { .. }
        | Request::RestoreRefused { .. } => false,
    }
}

#[test]
fn equal_keys_of_different_people_are_independent() {
    let mut test = Test::new(LIMITS);
    test.member();
    test.signin(2, 20, identity(0, 2));
    test.send(Event::Roles {
        project: 1,
        holdings: Box::new([Holding { person: 1, role: Role::Member }, Holding { person: 2, role: Role::Member }]),
    });
    let first = route(&test.request(10, 1, ask(1)));
    let second = route(&test.request(20, 1, ask(1)));
    assert_ne!(first, second);
    test.send(Event::Decided { request: first, outcome: Outcome::Started { task: 90 } });
    test.send(Event::Decided { request: second, outcome: Outcome::Started { task: 91 } });
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Outcome(Outcome::Started { task: 90 }));
    assert_eq!(reply(&test.request(20, 1, ask(1))), Reply::Outcome(Outcome::Started { task: 91 }));
}
#[test]
fn pending_capacity_refuses_before_routing_and_the_key_can_retry() {
    let mut test = Test::new(Limits { pending: 1, ..LIMITS });
    test.member();
    let first = route(&test.request(10, 1, ask(1)));
    assert_eq!(reply(&test.request(10, 2, ask(1))), Reply::Refused(Refusal::Busy));
    test.send(Event::Decided { request: first, outcome: Outcome::Refused(Refusal::Authority) });
    route(&test.request(10, 2, ask(1)));
}

#[test]
fn root_pressure_answers_every_waiter_and_releases_the_key_for_retry() {
    for refusal in [Refusal::Busy, Refusal::NotReady] {
        let mut test = Test::new(Limits { requests: 1, ..LIMITS });
        test.member();
        let first = route(&test.request(10, 1, ask(1)));
        assert!(test.request(10, 1, ask(1)).is_empty());
        let rows = test.send(Event::Decided { request: first, outcome: Outcome::Refused(refusal) });
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(is_reply), "retryable pressure saves no completed answer");
        assert_eq!(reply(&rows), Reply::Outcome(Outcome::Refused(refusal)));
        let retry = route(&test.request(10, 1, ask(1)));
        assert_ne!(first, retry, "retired route tokens fence earlier completions");
        test.send(Event::Decided { request: retry, outcome: Outcome::Started { task: 90 } });
        assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Outcome(Outcome::Started { task: 90 }));
    }
}
#[test]
fn identity_limits_clock_overflow_and_duplicate_holdings_leave_no_records() {
    let mut test = Test::new(Limits { identity_bytes: 1, ..LIMITS });
    assert_eq!(test.signin(1, 10, identity(0, 1)).len(), 1);
    test.env.limits = LIMITS;
    test.d = Domain::new(&LIMITS, Box::new([]));
    test.send(Event::Restored);
    test.env.wall = Wall::from_nanos(u64::MAX);
    assert_eq!(reply(&test.signin(1, 10, identity(0, 1))), Reply::Refused(Refusal::Limit));
    test.env.wall = Wall::EPOCH;
    test.member();
    let rows = test.send(Event::Roles {
        project: 1,
        holdings: Box::new([Holding { person: 1, role: Role::Owner }, Holding { person: 1, role: Role::Member }]),
    });
    assert_eq!(rows, [Request::RolesRefused { project: 1, refusal: Refusal::Limit }]);
    route(&test.request(10, 1, ask(1)));
}
#[test]
fn calls_before_restore_and_after_failed_restore_are_answered_not_ready() {
    let mut test = Test::new(LIMITS);
    test.d = Domain::new(&LIMITS, Box::new([]));
    assert_eq!(reply(&test.signin(1, 10, identity(0, 1))), Reply::Refused(Refusal::NotReady));
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Refused(Refusal::NotReady));
    test.send(Event::Restore {
        record: Stored::Answer {
            key: RequestKey { person: 99, key: [1; 16] },
            ask: ask(1),
            outcome: Outcome::Started { task: 90 },
            at: Wall::EPOCH,
        },
    });
    test.send(Event::Restored);
    assert!(!test.d.ready());
    assert_eq!(reply(&test.signin(1, 10, identity(0, 1))), Reply::Refused(Refusal::NotReady));
}

#[test]
fn expiry_projection_uses_the_environment_when_restore_finishes() {
    let mut test = Test::new(LIMITS);
    test.d = Domain::new(&LIMITS, Box::new([]));
    test.send(Event::Restore { record: Stored::Person { number: 1, identity: identity(0, 1) } });
    test.send(Event::Restore {
        record: Stored::SignIn { number: 10, person: 1, expires: Wall::from_nanos(Duration::from_secs(60).as_nanos()) },
    });
    test.env.now = Time::from_nanos(Duration::from_secs(30).as_nanos());
    test.env.wall = Wall::from_nanos(Duration::from_secs(50).as_nanos());
    test.send(Event::Restored);
    test.env.now = Time::from_nanos(Duration::from_secs(45).as_nanos());
    test.env.wall = Wall::from_nanos(Duration::from_secs(55).as_nanos());
    assert_eq!(reply(&test.request(10, 1, ask(1))), Reply::Refused(Refusal::SignIn));
}
