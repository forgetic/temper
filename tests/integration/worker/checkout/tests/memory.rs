//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the checkout with every workspace held for a spec at
//! its limits, each saving with a message and a branch at theirs.

use temper_lib::{Duration, Env, Queue, Time, Token};
use temper_worker_model_checkout::git::{Commit, Done, Fault, Kind, Missing};
use temper_worker_model_checkout::{
    Event, Limits, MAX_OUT, Message, Model, Outcome, Prepared, Refusal, Repository, Request, Spec, Start, step,
    worst_case,
};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    workspaces: 2,
    repositories: 2,
    name_bytes: 32,
    message_bytes: 128,
    remote_timeout: Duration::from_secs(60),
    local_timeout: Duration::from_secs(10),
    facts: 16,
};

fn bytes(len: u32) -> Box<[u8]> {
    vec![b'x'; usize::try_from(len).expect("a test length fits")].into_boxed_slice()
}

/// A name of `len` bytes, its first eight `n` in hexadecimal, so that names
/// differ and each is a safe path component.
fn name(len: u32, n: u32) -> Box<[u8]> {
    let mut name = bytes(len);
    name[..8].copy_from_slice(format!("{n:08x}").as_bytes());
    name
}

fn commit(n: u32) -> Commit {
    let mut raw = [0; 32];
    raw[..4].copy_from_slice(&n.to_be_bytes());
    Commit::new(raw)
}

/// What a step asked for, without the payload.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    Held { hold: Token },
    Io { op: Kind },
    Prepared { prepared: Prepared },
    Saved,
    Refused { refusal: Refusal },
    Released,
    Cancel,
}

/// A spec at the limits: its key and every name, branch and identity at the
/// longest, and as many repositories as there may be, each starting from a
/// base branch and pushed back.
fn full_spec(limits: &Limits, n: u32) -> Spec {
    let mut repositories = Vec::new();
    for place in 0..limits.repositories {
        repositories.push(Repository {
            name: name(limits.name_bytes, place),
            remote: name(limits.name_bytes, place),
            start: Start::Base { branch: name(limits.name_bytes, place) },
            identity: bytes(limits.name_bytes),
            push: Some(bytes(limits.name_bytes)),
        });
    }
    Spec { key: name(limits.name_bytes, n), repositories: repositories.into_boxed_slice() }
}

/// Fills every workspace of a model under `limits` with a hold for a spec at
/// the limits, built from scratch (each base branch created from the default
/// branch), then has each save, with a message and a branch of exactly their
/// limits, its save in flight; then aborts and releases each. The peak of the
/// heap in every step is checked against the worst case.
fn fill(limits: Limits) {
    let bound = worst_case(&limits).expect("the test limits fit");
    let env = Env { now: Time::ZERO, limits };
    let mut out = Queue::with_capacity(MAX_OUT);
    let meter = Meter::new();
    let mut model = Model::new(&limits);
    // The requests are the parent's to route and their receivers' to count:
    // each is dropped, keeping only what it asked for, and the step's peak
    // checked less them.
    let mut step = |event: Event| -> Vec<Asked> {
        meter.start();
        step(&mut model, &env, event, &mut out);
        let measured = meter.end();
        let mut asked = Vec::new();
        while let Some(request) = out.pop() {
            asked.push(match request {
                Request::Held { hold, .. } => Asked::Held { hold },
                Request::Io { op, .. } => Asked::Io { op: op.kind() },
                Request::Prepared { prepared, .. } => Asked::Prepared { prepared },
                Request::Saved { outcome: Outcome::Pushed { .. }, .. } => Asked::Saved,
                Request::Pushed { outcome: Outcome::Refused { refusal }, .. }
                | Request::Saved { outcome: Outcome::Refused { refusal }, .. } => Asked::Refused { refusal },
                Request::Released { .. } => Asked::Released,
                Request::Cancel { .. } => Asked::Cancel,
                Request::Pushed { outcome: Outcome::Pushed { .. }, .. } => panic!("nothing is pushed here"),
            });
        }
        meter.check(measured, bound, limits);
        asked
    };
    let mut holds = Vec::new();
    for n in 0..limits.workspaces {
        let prepare = Event::Prepare { client: Token::new(u64::from(n)), spec: full_spec(&limits, n) };
        let [Asked::Held { hold }, Asked::Io { op: Kind::Make }] = step(prepare)[..] else {
            panic!("a spec at the limits is admitted, into a new workspace");
        };
        let mut done = |done: Done| step(Event::Done { owner: hold, done });
        for _ in 0..limits.repositories {
            assert_eq!(done(Done::Succeeded)[..], [Asked::Io { op: Kind::Clone }]);
        }
        assert_eq!(done(Done::Succeeded)[..], [Asked::Io { op: Kind::Fetch }]);
        for place in 0..limits.repositories {
            let missing = Done::Failed { fault: Fault::Missing { missing: Missing::Branch } };
            assert_eq!(done(missing)[..], [Asked::Io { op: Kind::Fetch }], "the default branch");
            assert_eq!(done(Done::Fetched { commit: commit(place) })[..], [Asked::Io { op: Kind::Create }]);
            assert_eq!(done(Done::Succeeded)[..], [Asked::Io { op: Kind::CheckOut }]);
            let next = done(Done::Succeeded);
            if place + 1 < limits.repositories {
                assert_eq!(next[..], [Asked::Io { op: Kind::Fetch }]);
            } else {
                let [Asked::Prepared { prepared: Prepared::Ready { .. } }] = next[..] else {
                    panic!("the workspace is ready: {next:?}");
                };
            }
        }
        let message = Message { title: bytes(1), body: bytes(limits.message_bytes - 1) };
        let save = Event::Save { hold, branch: bytes(limits.name_bytes), message };
        assert_eq!(step(save)[..], [Asked::Io { op: Kind::Commit }]);
        let committed = Event::Done { owner: hold, done: Done::Committed { commit: commit(1000 + n) } };
        assert_eq!(step(committed)[..], [Asked::Io { op: Kind::Push }], "the save's push is in flight");
        holds.push(hold);
    }
    let held = meter.held();
    let name = u64::from(limits.name_bytes);
    let repositories = u64::from(limits.repositories);
    // Each workspace's key, twice, and its repositories' names and remotes;
    // each hold's spec, five names a repository, and its save's message and
    // branch.
    let each = 2 * name + repositories * 2 * name + repositories * 5 * name + u64::from(limits.message_bytes) + name;
    let full = u64::from(limits.workspaces) * each;
    assert!(held >= full, "{limits:?}: every workspace and hold holds its limits: {held} < {full}");
    // Every workspace is held: another prepare is refused.
    let another = Event::Prepare { client: Token::new(1 << 40), spec: full_spec(&limits, 1 << 20) };
    let [Asked::Prepared { prepared: Prepared::Refused { refusal: Refusal::Full } }] = step(another)[..] else {
        panic!("every workspace is held");
    };
    for hold in holds {
        assert!(step(Event::Abort { hold }).is_empty(), "a save in flight is not cancelled");
        assert!(step(Event::Release { hold }).is_empty(), "released once the save has ended");
        let pushed = Event::Done { owner: hold, done: Done::Succeeded };
        assert_eq!(step(pushed)[..], [Asked::Saved, Asked::Released]);
    }

    // A byte more is refused.
    let mut model = Model::new(&limits);
    let mut spec = full_spec(&limits, 0);
    spec.key = bytes(limits.name_bytes + 1);
    temper_worker_model_checkout::step(&mut model, &env, Event::Prepare { client: Token::new(0), spec }, &mut out);
    let Some(Request::Prepared { prepared, .. }) = out.pop() else { panic!("expected an answer") };
    assert_eq!(prepared, Prepared::Refused { refusal: Refusal::Invalid });
}

#[test]
fn a_model_with_every_workspace_held_at_its_limits_stays_within_its_worst_case() {
    fill(LIMITS);
    fill(Limits { workspaces: 64, repositories: 8, name_bytes: 256, message_bytes: 65_536, ..LIMITS });
    fill(Limits { workspaces: 1000, repositories: 1, name_bytes: 16, message_bytes: 16, ..LIMITS });
}
