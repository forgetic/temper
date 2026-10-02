//! Memory stays within the worst case (programming-model.md, 6.4), measured by
//! a counting allocator: the checkout with every workspace held for a spec at
//! its limits, pushing and saving with a message and a branch at theirs; then
//! every hold released and, before the reclaim point, every workspace held
//! again for other workstreams, so that the hold slab holds as many released
//! holds as live ones.

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
    Pushed,
    Saved,
    Refused { refusal: Refusal },
    Released,
    Cancel,
}

/// A spec at the limits for the workstream `n`: its key and every name,
/// remote, branch and identity at the longest, and as many repositories as
/// there may be, each starting from a base branch and pushed back.
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

fn message(limits: &Limits) -> Message {
    Message { title: bytes(1), body: bytes(limits.message_bytes - 1) }
}

/// A model under its limits, stepped with each step's peak checked against
/// the worst case. The requests are the parent's to route and their
/// receivers' to count: each is dropped, keeping only what it asked for, and
/// the step's peak checked less them. Nothing is reclaimed until the test
/// says so.
struct Fill {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
}

impl Fill {
    fn new(limits: Limits) -> Fill {
        let bound = worst_case(&limits).expect("the test limits fit");
        let env = Env { now: Time::ZERO, limits };
        Fill { model: Model::new(&limits), env, out: Queue::with_capacity(MAX_OUT), meter: Meter::new(), bound }
    }

    fn step(&mut self, event: Event) -> Vec<Asked> {
        self.meter.start();
        step(&mut self.model, &self.env, event, &mut self.out);
        let measured = self.meter.end();
        let mut asked = Vec::new();
        while let Some(request) = self.out.pop() {
            asked.push(match request {
                Request::Held { hold, .. } => Asked::Held { hold },
                Request::Io { op, .. } => Asked::Io { op: op.kind() },
                Request::Prepared { prepared, .. } => Asked::Prepared { prepared },
                Request::Pushed { outcome: Outcome::Pushed { .. }, .. } => Asked::Pushed,
                Request::Saved { outcome: Outcome::Pushed { .. }, .. } => Asked::Saved,
                Request::Pushed { outcome: Outcome::Refused { refusal }, .. }
                | Request::Saved { outcome: Outcome::Refused { refusal }, .. } => Asked::Refused { refusal },
                Request::Released { .. } => Asked::Released,
                Request::Cancel { .. } => Asked::Cancel,
            });
        }
        self.meter.check(measured, self.bound, self.env.limits);
        asked
    }

    fn done(&mut self, hold: Token, done: Done) -> Vec<Asked> {
        self.step(Event::Done { owner: hold, done })
    }

    /// Prepares the workstream `n` at the limits into the workspace it is
    /// given, made empty, each base branch created: returns the hold.
    fn prepare(&mut self, n: u32) -> Token {
        let limits = self.env.limits;
        let prepare = Event::Prepare { client: Token::new(u64::from(n)), spec: full_spec(&limits, n) };
        let [Asked::Held { hold }, Asked::Io { op: Kind::Make }] = self.step(prepare)[..] else {
            panic!("a spec at the limits is admitted, into a workspace made empty");
        };
        for _ in 0..limits.repositories {
            assert_eq!(self.done(hold, Done::Succeeded)[..], [Asked::Io { op: Kind::Clone }]);
        }
        assert_eq!(self.done(hold, Done::Succeeded)[..], [Asked::Io { op: Kind::Fetch }]);
        for place in 0..limits.repositories {
            let missing = Done::Failed { fault: Fault::Missing { missing: Missing::Branch } };
            assert_eq!(self.done(hold, missing)[..], [Asked::Io { op: Kind::Fetch }], "the default branch");
            assert_eq!(self.done(hold, Done::Fetched { commit: commit(place) })[..], [Asked::Io { op: Kind::Create }]);
            assert_eq!(self.done(hold, Done::Succeeded)[..], [Asked::Io { op: Kind::CheckOut }]);
            let next = self.done(hold, Done::Succeeded);
            if place + 1 < limits.repositories {
                assert_eq!(next[..], [Asked::Io { op: Kind::Fetch }]);
            } else {
                let [Asked::Prepared { prepared: Prepared::Ready { .. } }] = next[..] else {
                    panic!("the workspace is ready: {next:?}");
                };
            }
        }
        hold
    }

    /// Pushes every repository of `hold`, each push running out of time and
    /// verified as landed, its landings held until the last.
    fn push(&mut self, hold: Token, n: u32) {
        let limits = self.env.limits;
        let push = Event::Push { hold, message: message(&limits) };
        assert_eq!(self.step(push)[..], [Asked::Io { op: Kind::Commit }]);
        for place in 0..limits.repositories {
            let head = commit(1000 * (n + 1) + place);
            assert_eq!(self.done(hold, Done::Committed { commit: head })[..], [Asked::Io { op: Kind::Push }]);
            let timed_out = Done::Failed { fault: Fault::TimedOut };
            assert_eq!(self.done(hold, timed_out)[..], [Asked::Io { op: Kind::Fetch }], "verified");
            let next = self.done(hold, Done::Fetched { commit: head });
            let expected = if place + 1 < limits.repositories { Asked::Io { op: Kind::Commit } } else { Asked::Pushed };
            assert_eq!(next[..], [expected]);
        }
    }

    /// Saves what `hold` holds, with a message and a branch at their limits,
    /// and leaves the save's first push in flight.
    fn save(&mut self, hold: Token, n: u32) {
        let limits = self.env.limits;
        let save = Event::Save { hold, branch: bytes(limits.name_bytes), message: message(&limits) };
        assert_eq!(self.step(save)[..], [Asked::Io { op: Kind::Commit }]);
        let committed = Done::Committed { commit: commit(1_000_000 + n) };
        assert_eq!(self.done(hold, committed)[..], [Asked::Io { op: Kind::Push }], "the save's push is in flight");
    }
}

/// Fills every workspace of a model under `limits` as the module says. The
/// peak of the heap in every step is checked against the worst case.
fn fill(limits: Limits) {
    let mut fill = Fill::new(limits);
    let workspaces = limits.workspaces;
    let mut holds = Vec::new();
    for n in 0..workspaces {
        let hold = fill.prepare(n);
        fill.push(hold, n);
        fill.save(hold, n);
        holds.push(hold);
    }
    // Every workspace is held: another prepare is refused.
    let another = Event::Prepare { client: Token::new(1 << 40), spec: full_spec(&limits, 1 << 20) };
    let [Asked::Prepared { prepared: Prepared::Refused { refusal: Refusal::Full } }] = fill.step(another)[..] else {
        panic!("every workspace is held");
    };
    // Released, each once its save has landed: no reclaim point yet.
    for hold in holds {
        assert!(fill.step(Event::Abort { hold }).is_empty(), "a save in flight is not cancelled");
        assert!(fill.step(Event::Release { hold }).is_empty(), "released once the save has ended");
        assert_eq!(fill.done(hold, Done::Succeeded)[..], [Asked::Saved, Asked::Released]);
    }
    // Every workspace held again, for other workstreams, each evicting one.
    for n in workspaces..workspaces * 2 {
        let hold = fill.prepare(n);
        fill.save(hold, n);
    }
    assert_eq!(fill.model.holds(), workspaces * 2, "as many released holds as live ones");
    let held = fill.meter.held();
    let name = u64::from(limits.name_bytes);
    let repositories = u64::from(limits.repositories);
    // Each workspace's key, twice, and its repositories' names and remotes;
    // each hold's spec, five names a repository; and each live hold's save's
    // message and branch.
    let cache = 2 * name + repositories * 2 * name;
    let spec = repositories * 5 * name;
    let save = u64::from(limits.message_bytes) + name;
    let least = u64::from(workspaces) * (cache + 2 * spec + save);
    assert!(held >= least, "{limits:?}: every workspace and hold holds its limits: {held} < {least}");
    fill.model.reclaim();
    assert_eq!(fill.model.holds(), workspaces, "the released ones are reclaimed");

    // A byte more is refused.
    let mut model = Model::new(&limits);
    let mut spec = full_spec(&limits, 0);
    spec.key = bytes(limits.name_bytes + 1);
    let mut out = Queue::with_capacity(MAX_OUT);
    let env = Env { now: Time::ZERO, limits };
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
