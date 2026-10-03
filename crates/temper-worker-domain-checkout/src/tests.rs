//! Feed the domain events, inspect the requests that come out.

use alloc::boxed::Box;
use core::mem::size_of;

use temper_lib::bytes::copy_of;
use temper_lib::{Duration, Env, List, Queue, Time, Token, Writer};

use crate::git::{Commit, Done, Fault, Kind, Missing, Op, Place, Want};
use crate::{
    Cached, Domain, Event, Fact, Failure, Landing, Limits, MAX_OUT, Message, Outcome, Prepared, Refusal, Repository,
    Request, Spec, Start, Tally, Target, step, worst_case,
};

const LIMITS: Limits = Limits {
    workspaces: 2,
    repositories: 2,
    name_bytes: 16,
    message_bytes: 64,
    remote_timeout: Duration::from_secs(60),
    local_timeout: Duration::from_secs(10),
    facts: 64,
};

const NOW: Time = Time::from_nanos(1_000);

/// The domain, its environment and room for one step's output.
struct Harness {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
}

impl Harness {
    fn new(limits: Limits) -> Harness {
        assert!(worst_case(&limits).is_some(), "the test limits fit");
        Harness { domain: Domain::new(&limits), env: Env { now: NOW, limits }, out: Queue::with_capacity(MAX_OUT) }
    }

    /// Steps the domain with `event`, and takes what it emitted, at most
    /// `MAX_OUT`, then reclaims, as the loop would at the end of the
    /// iteration.
    fn step(&mut self, event: Event) -> List<Request> {
        step(&mut self.domain, &self.env, event, &mut self.out);
        let mut requests = List::with_capacity(MAX_OUT);
        while let Some(request) = self.out.pop() {
            requests.push(request).expect("a step emits at most MAX_OUT requests");
        }
        self.domain.reclaim();
        requests
    }

    /// Steps the domain with `event`, which emits exactly one request.
    fn one(&mut self, event: Event) -> Request {
        step(&mut self.domain, &self.env, event, &mut self.out);
        let request = self.out.pop().expect("one request");
        assert!(self.out.is_empty(), "only one: {:?} after {request:?}", self.out);
        self.domain.reclaim();
        request
    }

    /// Steps the domain with `event`, which emits nothing.
    fn none(&mut self, event: Event) {
        let requests = self.step(event);
        assert!(requests.is_empty(), "no request: {requests:?}");
    }

    /// Ends the operation in flight for `hold` with `done`, which asks for the
    /// next operation: returns it.
    fn next(&mut self, hold: Token, done: Done) -> Op {
        io(self.one(Event::Done { owner: hold, done }), hold)
    }

    /// Prepares `spec` for `client`: returns the hold and its first
    /// operation.
    fn prepare(&mut self, client: u64, spec: Spec) -> (Token, Op) {
        step(&mut self.domain, &self.env, Event::Prepare { client: Token::new(client), spec }, &mut self.out);
        let Some(Request::Held { client: held, hold }) = self.out.pop() else {
            panic!("the prepare is admitted");
        };
        assert_eq!(held, Token::new(client), "the client's token is echoed");
        let op = io(self.out.pop().expect("and its first operation asked for"), hold);
        assert!(self.out.is_empty(), "nothing else");
        self.domain.reclaim();
        (hold, op)
    }

    /// Prepares `spec` for `client`, io doing everything asked of it, each
    /// repository's starting point at `commit(place + 1)`: returns the hold.
    fn ready(&mut self, client: u64, spec: Spec) -> Token {
        let (hold, mut op) = self.prepare(client, spec);
        for _ in 0..16_u32 {
            let done = match &op {
                Op::Fetch { at, .. } => Done::Fetched { commit: start_of(at) },
                Op::Make { .. } | Op::Clone { .. } | Op::Create { .. } | Op::CheckOut { .. } | Op::Push { .. } => {
                    Done::Succeeded
                }
                Op::Commit { .. } => panic!("a prepare commits nothing"),
            };
            match self.one(Event::Done { owner: hold, done }) {
                Request::Io { owner, op: next, deadline: _ } => {
                    assert_eq!(owner, hold, "an operation is the hold's");
                    op = next;
                }
                Request::Prepared { client: to, prepared: Prepared::Ready { workspace: _ } } => {
                    assert_eq!(to, Token::new(client), "the client's token is echoed");
                    return hold;
                }
                other @ (Request::Held { .. }
                | Request::Prepared { .. }
                | Request::Pushed { .. }
                | Request::Saved { .. }
                | Request::Released { .. }
                | Request::Cancel { .. }) => panic!("the prepare goes on: {other:?}"),
            }
        }
        panic!("the prepare ends");
    }

    /// The facts told so far.
    fn facts(&mut self) -> List<Fact> {
        let mut facts = List::with_capacity(LIMITS.facts);
        while let Some(fact) = self.domain.pop_fact() {
            facts.push(fact).expect("room for the facts");
        }
        facts
    }
}

/// The operation `request` asks for, for `hold`.
fn io(request: Request, hold: Token) -> Op {
    let Request::Io { owner, op, deadline: _ } = request else {
        panic!("an operation is asked for: {request:?}");
    };
    assert_eq!(owner, hold, "an operation is the hold's");
    op
}

fn commit(n: u8) -> Commit {
    Commit::new([n; 32])
}

/// Where the test's io starts a repository: `commit(1)` for `a`, `commit(2)`
/// for `b`.
fn start_of(at: &Place) -> Commit {
    match &*at.repository {
        b"a" => commit(1),
        b"b" => commit(2),
        other => panic!("a repository of the tests: {other:?}"),
    }
}

fn bytes(text: &[u8]) -> Box<[u8]> {
    copy_of(text)
}

/// A repository starting from the branch `main`, pushed back to it if
/// `writable`.
fn repository(name: &[u8], writable: bool) -> Repository {
    let push = if writable { Some(bytes(b"main")) } else { None };
    Repository {
        name: bytes(name),
        remote: remote(name),
        start: Start::Branch { branch: bytes(b"main") },
        identity: bytes(b"bot"),
        push,
    }
}

/// Where the forge has the repository `name`.
fn remote(name: &[u8]) -> Box<[u8]> {
    let mut remote = Writer::new(name.len().checked_add(6).expect("a short name"));
    remote.put(b"forge:").expect("room for the prefix");
    remote.put(name).expect("room for the name");
    remote.finish()
}

fn spec(key: &[u8], repositories: Box<[Repository]>) -> Spec {
    Spec { key: bytes(key), repositories }
}

/// The workstream `key` with the writable `a` and the read-only `b`.
fn two(key: &[u8]) -> Spec {
    spec(key, Box::new([repository(b"a", true), repository(b"b", false)]))
}

/// The workstream `key` with the writable `a` only.
fn one(key: &[u8]) -> Spec {
    spec(key, Box::new([repository(b"a", true)]))
}

fn message() -> Message {
    Message { title: bytes(b"Fix it"), body: bytes(b"Because.") }
}

fn failed(fault: Fault) -> Done {
    Done::Failed { fault }
}

fn place(workspace: Token, repository: &[u8]) -> Place {
    Place { workspace, repository: bytes(repository) }
}

fn want_main() -> Want {
    Want::Branch { branch: bytes(b"main") }
}

fn prepared(request: Request) -> Prepared {
    let Request::Prepared { client: _, prepared } = request else {
        panic!("the prepare ends: {request:?}");
    };
    prepared
}

fn landings(request: Request) -> Box<[Landing]> {
    match request {
        Request::Pushed { client: _, outcome: Outcome::Pushed { landings } }
        | Request::Saved { client: _, outcome: Outcome::Pushed { landings } } => landings,
        other @ (Request::Held { .. }
        | Request::Prepared { .. }
        | Request::Pushed { .. }
        | Request::Saved { .. }
        | Request::Released { .. }
        | Request::Io { .. }
        | Request::Cancel { .. }) => panic!("the push ends: {other:?}"),
    }
}

fn refused(request: Request) -> Refusal {
    match request {
        Request::Pushed { client: _, outcome: Outcome::Refused { refusal } }
        | Request::Saved { client: _, outcome: Outcome::Refused { refusal } }
        | Request::Prepared { client: _, prepared: Prepared::Refused { refusal } } => refusal,
        other @ (Request::Held { .. }
        | Request::Prepared { .. }
        | Request::Pushed { .. }
        | Request::Saved { .. }
        | Request::Released { .. }
        | Request::Io { .. }
        | Request::Cancel { .. }) => panic!("refused: {other:?}"),
    }
}

/// The workspace a held workspace's operations name.
fn workspace_of(op: &Op) -> Token {
    match op {
        Op::Make { workspace } => *workspace,
        Op::Clone { at, .. }
        | Op::Fetch { at, .. }
        | Op::Create { at, .. }
        | Op::CheckOut { at, .. }
        | Op::Commit { at, .. }
        | Op::Push { at, .. } => at.workspace,
    }
}

#[test]
fn a_new_workspace_is_made_then_each_repository_cloned_fetched_and_checked_out() {
    let mut h = Harness::new(LIMITS);
    let (hold, op) = h.prepare(7, two(b"issue-1"));
    let Op::Make { workspace } = op else { panic!("a new workspace is made: {op:?}") };
    let identity = bytes(b"bot");
    let clone_a = Op::Clone { at: place(workspace, b"a"), remote: remote(b"a"), identity: identity.clone() };
    assert_eq!(h.next(hold, Done::Succeeded), clone_a);
    let clone_b = Op::Clone { at: place(workspace, b"b"), remote: remote(b"b"), identity: identity.clone() };
    assert_eq!(h.next(hold, Done::Succeeded), clone_b);
    let fetch_a =
        Op::Fetch { at: place(workspace, b"a"), remote: remote(b"a"), want: want_main(), identity: identity.clone() };
    assert_eq!(h.next(hold, Done::Succeeded), fetch_a);
    let check_out_a = Op::CheckOut { at: place(workspace, b"a"), commit: commit(1) };
    assert_eq!(h.next(hold, Done::Fetched { commit: commit(1) }), check_out_a);
    let fetch_b = Op::Fetch { at: place(workspace, b"b"), remote: remote(b"b"), want: want_main(), identity };
    assert_eq!(h.next(hold, Done::Succeeded), fetch_b);
    let check_out_b = Op::CheckOut { at: place(workspace, b"b"), commit: commit(2) };
    assert_eq!(h.next(hold, Done::Fetched { commit: commit(2) }), check_out_b);
    let ready = h.one(Event::Done { owner: hold, done: Done::Succeeded });
    assert_eq!(ready, Request::Prepared { client: Token::new(7), prepared: Prepared::Ready { workspace } });
    assert_eq!((h.domain.workspaces(), h.domain.idle(), h.domain.holds()), (1, 0, 1));
}

#[test]
fn every_operation_has_a_deadline_by_where_it_runs() {
    let local = NOW.saturating_add(LIMITS.local_timeout);
    let remote = NOW.saturating_add(LIMITS.remote_timeout);
    let mut h = Harness::new(LIMITS);
    let requests = h.step(Event::Prepare { client: Token::new(1), spec: one(b"w") });
    let Some(Request::Io { owner, op: Op::Make { .. }, deadline }) = requests.get(1) else {
        panic!("a new workspace is made");
    };
    assert_eq!(*deadline, local, "making a directory is local");
    let owner = *owner;
    let Request::Io { deadline, op: Op::Clone { .. }, .. } = h.one(Event::Done { owner, done: Done::Succeeded }) else {
        panic!("a clone follows");
    };
    assert_eq!(deadline, remote, "a clone reaches the forge");
}

#[test]
fn a_cached_workspace_is_reused_for_its_workstream_and_fetched_afresh() {
    let mut h = Harness::new(LIMITS);
    let first = h.ready(1, two(b"issue-1"));
    assert_eq!(h.one(Event::Release { hold: first }), Request::Released { client: Token::new(1) });
    assert_eq!(h.domain.idle(), 1, "the workspace is idle in the cache");
    h.facts();
    let (hold, op) = h.prepare(2, two(b"issue-1"));
    let Op::Fetch { at, want, .. } = op else { panic!("a reused workspace is fetched at once: {op:?}") };
    assert_eq!((&*at.repository, want), (&b"a"[..], want_main()));
    assert_ne!(hold, first, "a new hold, under a name of its own");
    let facts = h.facts();
    assert_eq!(facts.get(0), Some(&Fact::Held { client: Token::new(2), cached: Cached::Reused }));
    assert_eq!(h.domain.workspaces(), 1, "the same workspace");
}

#[test]
fn a_workspace_holding_other_repositories_is_rebuilt() {
    let mut h = Harness::new(LIMITS);
    let first = h.ready(1, two(b"issue-1"));
    h.one(Event::Release { hold: first });
    h.facts();
    let (_, op) = h.prepare(2, one(b"issue-1"));
    assert_eq!(op.kind(), Kind::Make, "made again, empty");
    assert_eq!(h.facts().get(0), Some(&Fact::Held { client: Token::new(2), cached: Cached::Rebuilt }));
}

#[test]
fn the_least_recently_used_idle_workspace_is_evicted_when_the_cache_is_full() {
    let mut h = Harness::new(LIMITS);
    let a = h.ready(1, one(b"a-work"));
    let b = h.ready(2, one(b"b-work"));
    assert_eq!((h.domain.workstream(0), h.domain.workstream(1)), (Some(&b"a-work"[..]), Some(&b"b-work"[..])));
    // Every workspace is held: nothing to evict.
    let refused_full = h.one(Event::Prepare { client: Token::new(3), spec: one(b"c-work") });
    assert_eq!(refused(refused_full), Refusal::Full);
    // b is released first, so it is the least recently used.
    h.one(Event::Release { hold: b });
    h.one(Event::Release { hold: a });
    h.facts();
    let (_, op) = h.prepare(3, one(b"c-work"));
    let Op::Make { workspace } = op else { panic!("the evicted workspace is made again: {op:?}") };
    assert_eq!(h.facts().get(0), Some(&Fact::Held { client: Token::new(3), cached: Cached::Evicted }));
    // c's is being built: it is not listed until it is cloned.
    assert_eq!((h.domain.workstream(0), h.domain.workstream(1)), (Some(&b"a-work"[..]), None));
    // a is still cached, and reused.
    let (_, op) = h.prepare(4, one(b"a-work"));
    assert_eq!(op.kind(), Kind::Fetch, "a's workspace is still cached");
    assert_ne!(workspace_of(&op), workspace, "and it is not the one evicted");
    assert_eq!((h.domain.workspaces(), h.domain.idle()), (2, 0));
}

#[test]
fn a_held_workstream_is_busy() {
    let mut h = Harness::new(LIMITS);
    h.prepare(1, one(b"w"));
    let busy = h.one(Event::Prepare { client: Token::new(2), spec: one(b"w") });
    assert_eq!(
        busy,
        Request::Prepared { client: Token::new(2), prepared: Prepared::Refused { refusal: Refusal::Busy } }
    );
    assert_eq!(h.facts().last(), Some(&Fact::Refused { client: Token::new(2), refusal: Refusal::Busy }));
}

#[test]
fn a_spec_beyond_the_limits_is_refused_at_the_entrance() {
    let long = [b'x'; 17];
    let invalid = [
        spec(b"w", Box::new([])),
        spec(b"w", Box::new([repository(b"a", true), repository(b"b", true), repository(b"c", true)])),
        spec(b"w", Box::new([repository(b"a", true), repository(b"a", false)])),
        spec(b"", Box::new([repository(b"a", true)])),
        spec(&long, Box::new([repository(b"a", true)])),
        spec(b"w", Box::new([repository(&long, true)])),
        spec(b"w", Box::new([repository(b"", true)])),
        spec(b"w", Box::new([Repository { identity: bytes(&long), ..repository(b"a", true) }])),
        spec(b"w", Box::new([Repository { push: Some(bytes(b"")), ..repository(b"a", true) }])),
        spec(b"w", Box::new([Repository { start: Start::Base { branch: bytes(&long) }, ..repository(b"a", true) }])),
        spec(b"w", Box::new([Repository { remote: bytes(b""), ..repository(b"a", true) }])),
        spec(b"w", Box::new([Repository { remote: bytes(&long), ..repository(b"a", true) }])),
        // Each repository's directory is one safe path component.
        spec(b"w", Box::new([repository(b".", true)])),
        spec(b"w", Box::new([repository(b"..", true)])),
        spec(b"w", Box::new([repository(b"a/b", true)])),
        spec(b"w", Box::new([repository(b"/", true)])),
        spec(b"w", Box::new([repository(b"a\0", true)])),
        spec(b"w", Box::new([repository(b".git", true)])),
        spec(b"w", Box::new([repository(b".GiT", true)])),
    ];
    let mut h = Harness::new(LIMITS);
    for spec in invalid {
        let request = h.one(Event::Prepare { client: Token::new(1), spec });
        assert_eq!(refused(request), Refusal::Invalid);
    }
    assert_eq!((h.domain.workspaces(), h.domain.holds()), (0, 0), "nothing is held");
    // At the limits, it is admitted; and a name may look like a git
    // directory's or have dots, so long as it is not one.
    let at = Repository { identity: bytes(&long[..16]), remote: bytes(&long[..16]), ..repository(&long[..16], true) };
    h.prepare(1, spec(&long[..16], Box::new([at, repository(b".gitx", false)])));
    h.prepare(2, spec(b"v", Box::new([repository(b"...", true), repository(b".git.", false)])));
}

#[test]
fn a_missing_base_branch_is_created_from_the_default_branch() {
    let mut h = Harness::new(LIMITS);
    let base = Repository { start: Start::Base { branch: bytes(b"temper/1") }, ..repository(b"a", true) };
    let (hold, _) = h.prepare(1, spec(b"w", Box::new([base])));
    h.next(hold, Done::Succeeded);
    let fetch = h.next(hold, Done::Succeeded);
    let Op::Fetch { at, want: Want::Branch { branch }, .. } = fetch else { panic!("the base branch is fetched") };
    assert_eq!(&*branch, b"temper/1");
    let default = h.next(hold, failed(Fault::Missing { missing: Missing::Branch }));
    let Op::Fetch { want: Want::Default, .. } = default else { panic!("then the default branch: {default:?}") };
    let create = h.next(hold, Done::Fetched { commit: commit(9) });
    let expected = Op::Create {
        at: at.clone(),
        remote: remote(b"a"),
        branch: bytes(b"temper/1"),
        commit: commit(9),
        identity: bytes(b"bot"),
    };
    assert_eq!(create, expected, "created at the default branch's tip");
    let check_out = h.next(hold, Done::Succeeded);
    assert_eq!(check_out, Op::CheckOut { at, commit: commit(9) });
    let ready = prepared(h.one(Event::Done { owner: hold, done: Done::Succeeded }));
    let Prepared::Ready { .. } = ready else { panic!("ready: {ready:?}") };
}

#[test]
fn a_base_branch_created_meanwhile_is_fetched_again_and_not_moved() {
    let mut h = Harness::new(LIMITS);
    let base = Repository { start: Start::Base { branch: bytes(b"temper/1") }, ..repository(b"a", true) };
    let (hold, _) = h.prepare(1, spec(b"w", Box::new([base])));
    h.next(hold, Done::Succeeded);
    h.next(hold, Done::Succeeded);
    h.next(hold, failed(Fault::Missing { missing: Missing::Branch }));
    h.next(hold, Done::Fetched { commit: commit(9) });
    let refetch = h.next(hold, Done::Exists);
    let Op::Fetch { want: Want::Branch { branch }, .. } = refetch else { panic!("fetched again: {refetch:?}") };
    assert_eq!(&*branch, b"temper/1");
    let check_out = h.next(hold, Done::Fetched { commit: commit(5) });
    let Op::CheckOut { commit: at, .. } = check_out else { panic!("checked out: {check_out:?}") };
    assert_eq!(at, commit(5), "where the other party put it");
    // Gone again by the time it is fetched: transient.
    let mut h = Harness::new(LIMITS);
    let base = Repository { start: Start::Base { branch: bytes(b"temper/1") }, ..repository(b"a", true) };
    let (hold, _) = h.prepare(1, spec(b"w", Box::new([base])));
    h.next(hold, Done::Succeeded);
    h.next(hold, Done::Succeeded);
    h.next(hold, failed(Fault::Missing { missing: Missing::Branch }));
    h.next(hold, Done::Fetched { commit: commit(9) });
    h.next(hold, Done::Exists);
    let gone = h.one(Event::Done { owner: hold, done: failed(Fault::Missing { missing: Missing::Branch }) });
    assert_eq!(prepared(gone), Prepared::Failed { failure: Failure::Transient });
}

#[test]
fn a_commit_start_is_fetched_by_its_hash() {
    let mut h = Harness::new(LIMITS);
    let exact = Repository { start: Start::Commit { commit: commit(4) }, ..repository(b"a", false) };
    let (hold, _) = h.prepare(1, spec(b"w", Box::new([exact])));
    h.next(hold, Done::Succeeded);
    let fetch = h.next(hold, Done::Succeeded);
    let Op::Fetch { want: Want::Commit { commit: wanted }, .. } = fetch else { panic!("by its hash: {fetch:?}") };
    assert_eq!(wanted, commit(4));
}

#[test]
fn preparation_failures_are_typed() {
    let cases = [
        (
            2,
            failed(Fault::Missing { missing: Missing::Repository }),
            Failure::Missing { repository: 1, missing: Missing::Repository },
        ),
        (2, failed(Fault::Unreachable), Failure::Transient),
        (2, failed(Fault::Refused), Failure::Refused { repository: 1 }),
        (
            3,
            failed(Fault::Missing { missing: Missing::Branch }),
            Failure::Missing { repository: 0, missing: Missing::Branch },
        ),
        (3, failed(Fault::TimedOut), Failure::Transient),
        (0, failed(Fault::Broken), Failure::Transient),
        (4, failed(Fault::Broken), Failure::Transient),
    ];
    for (after, fault, failure) in cases {
        let mut h = Harness::new(LIMITS);
        let (hold, _) = h.prepare(1, two(b"w"));
        // Make, clone a, clone b, fetch a, check out a.
        let answers = [Done::Succeeded, Done::Succeeded, Done::Succeeded, Done::Fetched { commit: commit(1) }];
        for done in answers.into_iter().take(after) {
            h.next(hold, done);
        }
        let end = h.one(Event::Done { owner: hold, done: fault });
        assert_eq!(prepared(end), Prepared::Failed { failure }, "after {after} operations");
        // It may only be released.
        assert_eq!(refused(h.one(Event::Push { hold, message: message() })), Refusal::Busy);
        h.none(Event::Abort { hold });
        assert_eq!(h.one(Event::Release { hold }), Request::Released { client: Token::new(1) });
    }
}

#[test]
fn a_build_that_failed_is_rebuilt_and_a_fetch_that_failed_is_not() {
    let mut h = Harness::new(LIMITS);
    let (hold, _) = h.prepare(1, one(b"w"));
    h.next(hold, Done::Succeeded);
    h.one(Event::Done { owner: hold, done: failed(Fault::Unreachable) });
    h.one(Event::Release { hold });
    let (hold, op) = h.prepare(2, one(b"w"));
    assert_eq!(op.kind(), Kind::Make, "what a failed clone left is not known");
    h.next(hold, Done::Succeeded);
    h.next(hold, Done::Succeeded);
    h.one(Event::Done { owner: hold, done: failed(Fault::Unreachable) });
    h.one(Event::Release { hold });
    let (_, op) = h.prepare(3, one(b"w"));
    assert_eq!(op.kind(), Kind::Fetch, "a failed fetch leaves the clones");
}

#[test]
fn a_push_commits_the_tree_on_its_start_and_pushes_it_to_the_push_branch() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, two(b"w"));
    let commit_op = io(h.one(Event::Push { hold, message: message() }), hold);
    let Op::Commit { at, parent, title, body, identity } = commit_op else { panic!("a commit: {commit_op:?}") };
    assert_eq!((&*at.repository, parent), (&b"a"[..], commit(1)), "the writable one, on its start");
    assert_eq!((&*title, &*body, &*identity), (&b"Fix it"[..], &b"Because."[..], &b"bot"[..]));
    let push = h.next(hold, Done::Committed { commit: commit(11) });
    let expected =
        Op::Push { at, remote: remote(b"a"), commit: commit(11), branch: bytes(b"main"), identity: bytes(b"bot") };
    assert_eq!(push, expected);
    let end = h.one(Event::Done { owner: hold, done: Done::Succeeded });
    assert_eq!(
        end,
        Request::Pushed {
            client: Token::new(1),
            outcome: Outcome::Pushed {
                landings: Box::new([Landing::Landed { commit: commit(11) }, Landing::Unchanged])
            },
        }
    );
    let told = Tally { landed: 1, moved: 0, failed: 0, refused: 0, unchanged: 1, aborted: 0 };
    assert_eq!(h.facts().last(), Some(&Fact::Pushed { client: Token::new(1), to: Target::Push, tally: told }));
}

#[test]
fn a_push_builds_on_what_the_last_one_landed() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.one(Event::Push { hold, message: message() });
    h.next(hold, Done::Committed { commit: commit(11) });
    h.one(Event::Done { owner: hold, done: Done::Succeeded });
    // The next commits on the one that landed.
    let commit_op = io(h.one(Event::Push { hold, message: message() }), hold);
    let Op::Commit { parent, .. } = commit_op else { panic!("a commit: {commit_op:?}") };
    assert_eq!(parent, commit(11));
    // With nothing new, there is nothing to push.
    let end = h.one(Event::Done { owner: hold, done: Done::Unchanged });
    assert_eq!(&*landings(end), &[Landing::Unchanged]);
}

#[test]
fn an_unchanged_tree_has_nothing_to_push() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, two(b"w"));
    h.one(Event::Push { hold, message: message() });
    let end = h.one(Event::Done { owner: hold, done: Done::Unchanged });
    assert_eq!(&*landings(end), &[Landing::Unchanged, Landing::Unchanged]);
}

#[test]
fn a_push_that_is_not_a_fast_forward_is_moved_and_others_fail_by_kind() {
    let cases = [
        (Done::Rejected, Landing::Moved),
        (failed(Fault::Refused), Landing::Refused),
        (failed(Fault::Missing { missing: Missing::Repository }), Landing::Failed),
    ];
    for (done, landing) in cases {
        let mut h = Harness::new(LIMITS);
        let hold = h.ready(1, one(b"w"));
        h.one(Event::Push { hold, message: message() });
        h.next(hold, Done::Committed { commit: commit(11) });
        let end = h.one(Event::Done { owner: hold, done });
        assert_eq!(&*landings(end), &[landing]);
        // The hold is ready again: the next push tries the same commit.
        h.one(Event::Push { hold, message: message() });
        let again = h.next(hold, Done::Unchanged);
        let Op::Push { commit: tried, .. } = again else { panic!("a push: {again:?}") };
        assert_eq!(tried, commit(11));
    }
    // A commit that fails is not pushed.
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.one(Event::Push { hold, message: message() });
    let end = h.one(Event::Done { owner: hold, done: failed(Fault::Broken) });
    assert_eq!(&*landings(end), &[Landing::Failed]);
}

#[test]
fn pushing_several_repositories_is_not_atomic() {
    let mut h = Harness::new(LIMITS);
    let both = spec(b"w", Box::new([repository(b"a", true), repository(b"b", true)]));
    let hold = h.ready(1, both);
    h.one(Event::Push { hold, message: message() });
    h.next(hold, Done::Committed { commit: commit(11) });
    let commit_b = h.next(hold, Done::Rejected);
    let Op::Commit { parent, .. } = commit_b else { panic!("b is tried: {commit_b:?}") };
    assert_eq!(parent, commit(2));
    h.next(hold, Done::Committed { commit: commit(12) });
    let end = h.one(Event::Done { owner: hold, done: Done::Succeeded });
    assert_eq!(&*landings(end), &[Landing::Moved, Landing::Landed { commit: commit(12) }]);
}

#[test]
fn a_save_pushes_to_the_saved_work_branch_what_changed_since_the_start() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    // Nothing changed: nothing to save.
    h.one(Event::Save { hold, branch: bytes(b"saved/1"), message: message() });
    let end = h.one(Event::Done { owner: hold, done: Done::Unchanged });
    assert_eq!(
        end,
        Request::Saved { client: Token::new(1), outcome: Outcome::Pushed { landings: Box::new([Landing::Unchanged]) } }
    );
    h.one(Event::Save { hold, branch: bytes(b"saved/1"), message: message() });
    let push = h.next(hold, Done::Committed { commit: commit(11) });
    let Op::Push { commit: pushed, branch, .. } = push else { panic!("a push: {push:?}") };
    assert_eq!((pushed, &*branch), (commit(11), &b"saved/1"[..]));
    let end = h.one(Event::Done { owner: hold, done: Done::Succeeded });
    assert_eq!(&*landings(end), &[Landing::Landed { commit: commit(11) }]);
    // Saving leaves the push branch as it was: a push still has it to push.
    h.one(Event::Push { hold, message: message() });
    let push = h.next(hold, Done::Unchanged);
    let Op::Push { commit: pushed, branch, .. } = push else { panic!("a push: {push:?}") };
    assert_eq!((pushed, &*branch), (commit(11), &b"main"[..]));
}

#[test]
fn a_push_or_a_save_beyond_the_limits_is_refused() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    let long = Message { title: bytes(b"t"), body: bytes(&[b'x'; 64]) };
    assert_eq!(refused(h.one(Event::Push { hold, message: long })), Refusal::Invalid);
    let untitled = Message { title: bytes(b""), body: bytes(b"x") };
    assert_eq!(refused(h.one(Event::Push { hold, message: untitled })), Refusal::Invalid);
    let save = Event::Save { hold, branch: bytes(&[b'x'; 17]), message: message() };
    assert_eq!(refused(h.one(save)), Refusal::Invalid);
    // Still ready.
    assert_eq!(io(h.one(Event::Push { hold, message: message() }), hold).kind(), Kind::Commit);
}

#[test]
fn a_push_while_another_operation_is_under_way_is_refused() {
    let mut h = Harness::new(LIMITS);
    let (hold, _) = h.prepare(1, one(b"w"));
    assert_eq!(refused(h.one(Event::Push { hold, message: message() })), Refusal::Busy);
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.one(Event::Push { hold, message: message() });
    let save = Event::Save { hold, branch: bytes(b"saved/1"), message: message() };
    assert_eq!(refused(h.one(save)), Refusal::Busy);
    // The push goes on.
    assert_eq!(h.next(hold, Done::Committed { commit: commit(11) }).kind(), Kind::Push);
}

#[test]
fn an_abort_ends_a_prepare_once_its_operation_has_settled() {
    for done in [failed(Fault::Cancelled), Done::Succeeded] {
        let mut h = Harness::new(LIMITS);
        let (hold, _) = h.prepare(1, two(b"w"));
        h.next(hold, Done::Succeeded);
        assert_eq!(h.one(Event::Abort { hold }), Request::Cancel { owner: hold });
        h.none(Event::Abort { hold });
        // Whichever won the race, nothing more is asked of io.
        assert_eq!(prepared(h.one(Event::Done { owner: hold, done })), Prepared::Aborted);
        assert_eq!(refused(h.one(Event::Push { hold, message: message() })), Refusal::Busy);
        assert_eq!(h.one(Event::Release { hold }), Request::Released { client: Token::new(1) });
        // What the build left is not known.
        let (_, op) = h.prepare(2, two(b"w"));
        assert_eq!(op.kind(), Kind::Make);
    }
}

#[test]
fn an_abort_mid_push_waits_for_the_push_keeps_what_landed_and_reports_the_rest_aborted() {
    let mut h = Harness::new(LIMITS);
    let both = spec(b"w", Box::new([repository(b"a", true), repository(b"b", true)]));
    let hold = h.ready(1, both);
    h.one(Event::Push { hold, message: message() });
    h.next(hold, Done::Committed { commit: commit(11) });
    // The push in flight is not cancelled.
    h.none(Event::Abort { hold });
    h.none(Event::Abort { hold });
    let end = h.one(Event::Done { owner: hold, done: Done::Succeeded });
    assert_eq!(&*landings(end), &[Landing::Landed { commit: commit(11) }, Landing::Aborted]);
    // Ready again: a save goes ahead.
    let save = h.one(Event::Save { hold, branch: bytes(b"saved/1"), message: message() });
    assert_eq!(io(save, hold).kind(), Kind::Commit);
    // An abort while committing pushes nothing, whether the commit was made
    // or cancelled.
    h.none(Event::Abort { hold });
    let end = h.one(Event::Done { owner: hold, done: Done::Committed { commit: commit(13) } });
    assert_eq!(&*landings(end), &[Landing::Aborted, Landing::Aborted]);
    h.one(Event::Push { hold, message: message() });
    h.none(Event::Abort { hold });
    let end = h.one(Event::Done { owner: hold, done: failed(Fault::Cancelled) });
    assert_eq!(&*landings(end), &[Landing::Aborted, Landing::Aborted]);
}

#[test]
fn a_push_that_may_have_landed_is_verified() {
    let cases = [
        (failed(Fault::TimedOut), Done::Fetched { commit: commit(11) }, Landing::Landed { commit: commit(11) }),
        (failed(Fault::Broken), Done::Fetched { commit: commit(1) }, Landing::Failed),
        (failed(Fault::Unreachable), failed(Fault::Unreachable), Landing::Failed),
        (failed(Fault::TimedOut), failed(Fault::Missing { missing: Missing::Branch }), Landing::Failed),
    ];
    for (ended, found, landing) in cases {
        let mut h = Harness::new(LIMITS);
        let hold = h.ready(1, one(b"w"));
        h.one(Event::Push { hold, message: message() });
        h.next(hold, Done::Committed { commit: commit(11) });
        // Verified even when the client asked to abort meanwhile.
        h.none(Event::Abort { hold });
        let verify = h.next(hold, ended);
        let Op::Fetch { at, remote: from, want: Want::Branch { branch }, .. } = verify else {
            panic!("the push branch is fetched: {verify:?}");
        };
        assert_eq!((&*at.repository, &*from, &*branch), (&b"a"[..], &*remote(b"a"), &b"main"[..]));
        let end = h.one(Event::Done { owner: hold, done: found });
        assert_eq!(&*landings(end), &[landing]);
    }
    // One that landed is the base of the next push.
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.one(Event::Push { hold, message: message() });
    h.next(hold, Done::Committed { commit: commit(11) });
    h.next(hold, failed(Fault::TimedOut));
    h.one(Event::Done { owner: hold, done: Done::Fetched { commit: commit(11) } });
    h.one(Event::Push { hold, message: message() });
    let end = h.one(Event::Done { owner: hold, done: Done::Unchanged });
    assert_eq!(&*landings(end), &[Landing::Unchanged]);
    // A push refused, or not a fast-forward, is not verified.
    for (ended, landing) in [(failed(Fault::Refused), Landing::Refused), (Done::Rejected, Landing::Moved)] {
        let mut h = Harness::new(LIMITS);
        let hold = h.ready(1, one(b"w"));
        h.one(Event::Push { hold, message: message() });
        h.next(hold, Done::Committed { commit: commit(11) });
        let end = h.one(Event::Done { owner: hold, done: ended });
        assert_eq!(&*landings(end), &[landing]);
    }
}

#[test]
fn an_operation_that_broke_ran_out_of_time_or_was_cancelled_leaves_the_workspace_untrusted() {
    for fault in [Fault::Broken, Fault::TimedOut, Fault::Cancelled, Fault::Unreachable, Fault::Refused] {
        // A fetch, in a workspace already cloned.
        let mut h = Harness::new(LIMITS);
        let hold = h.ready(1, one(b"w"));
        h.one(Event::Release { hold });
        let (hold, _) = h.prepare(2, one(b"w"));
        h.one(Event::Done { owner: hold, done: failed(fault) });
        h.one(Event::Release { hold });
        let (_, op) = h.prepare(3, one(b"w"));
        let damaged = match fault {
            Fault::Broken | Fault::TimedOut | Fault::Cancelled => true,
            Fault::Missing { .. } | Fault::Refused | Fault::Unreachable => false,
        };
        let expected = if damaged { Kind::Make } else { Kind::Fetch };
        assert_eq!(op.kind(), expected, "after a fetch that ended {fault:?}");
        // A commit.
        let mut h = Harness::new(LIMITS);
        let hold = h.ready(1, one(b"w"));
        h.one(Event::Push { hold, message: message() });
        h.one(Event::Done { owner: hold, done: failed(fault) });
        h.one(Event::Release { hold });
        let (_, op) = h.prepare(2, one(b"w"));
        assert_eq!(op.kind(), expected, "after a commit that ended {fault:?}");
    }
}

#[test]
fn a_missing_base_branch_of_a_read_only_repository_is_missing() {
    let mut h = Harness::new(LIMITS);
    let base = Repository { start: Start::Base { branch: bytes(b"temper/1") }, ..repository(b"a", false) };
    let (hold, _) = h.prepare(1, spec(b"w", Box::new([base])));
    h.next(hold, Done::Succeeded);
    h.next(hold, Done::Succeeded);
    let end = h.one(Event::Done { owner: hold, done: failed(Fault::Missing { missing: Missing::Branch }) });
    assert_eq!(
        prepared(end),
        Prepared::Failed { failure: Failure::Missing { repository: 0, missing: Missing::Branch } }
    );
}

#[test]
fn a_workspace_cloned_from_another_remote_is_rebuilt() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.one(Event::Release { hold });
    let moved = Repository { remote: remote(b"elsewhere"), ..repository(b"a", true) };
    let (_, op) = h.prepare(2, spec(b"w", Box::new([moved])));
    assert_eq!(op.kind(), Kind::Make, "the same directory, but another repository");
}

#[test]
fn only_workspaces_whose_disk_is_known_are_listed() {
    let mut h = Harness::new(LIMITS);
    let (hold, _) = h.prepare(1, one(b"w"));
    assert_eq!(h.domain.workstream(0), None, "being built");
    h.next(hold, Done::Succeeded);
    h.next(hold, Done::Succeeded);
    assert_eq!(h.domain.workstream(0), Some(&b"w"[..]), "cloned");
    h.one(Event::Done { owner: hold, done: failed(Fault::TimedOut) });
    assert_eq!(h.domain.workstream(0), None, "damaged");
    h.one(Event::Release { hold });
    h.ready(2, one(b"v"));
    assert_eq!((h.domain.workstream(0), h.domain.workstream(1)), (Some(&b"v"[..]), None));
}

#[test]
fn a_release_under_way_aborts_first_and_releases_once_settled() {
    let mut h = Harness::new(LIMITS);
    let (hold, _) = h.prepare(1, one(b"w"));
    // An abort, then a release: one cancel.
    assert_eq!(h.one(Event::Abort { hold }), Request::Cancel { owner: hold });
    h.none(Event::Release { hold });
    h.none(Event::Release { hold });
    let requests = h.step(Event::Done { owner: hold, done: failed(Fault::Cancelled) });
    let expected = [
        Request::Prepared { client: Token::new(1), prepared: Prepared::Aborted },
        Request::Released { client: Token::new(1) },
    ];
    assert_eq!(requests.as_slice(), &expected);
    assert_eq!((h.domain.holds(), h.domain.idle()), (0, 1), "reclaimed, and the workspace idle");
    // A release while pushing waits for the push.
    let hold = h.ready(2, one(b"w"));
    h.one(Event::Push { hold, message: message() });
    h.next(hold, Done::Committed { commit: commit(11) });
    h.none(Event::Release { hold });
    let requests = h.step(Event::Done { owner: hold, done: Done::Succeeded });
    let landed = Box::new([Landing::Landed { commit: commit(11) }]);
    let expected = [
        Request::Pushed { client: Token::new(2), outcome: Outcome::Pushed { landings: landed } },
        Request::Released { client: Token::new(2) },
    ];
    assert_eq!(requests.as_slice(), &expected);
}

#[test]
fn a_released_hold_is_stale() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.one(Event::Release { hold });
    h.none(Event::Push { hold, message: message() });
    h.none(Event::Save { hold, branch: bytes(b"s"), message: message() });
    h.none(Event::Abort { hold });
    h.none(Event::Release { hold });
    // Not even once the workspace is held again.
    let again = h.ready(2, one(b"w"));
    assert_ne!(again, hold);
    h.none(Event::Release { hold });
    h.none(Event::Push { hold, message: message() });
}

#[test]
fn an_abort_with_nothing_under_way_does_nothing() {
    let mut h = Harness::new(LIMITS);
    let hold = h.ready(1, one(b"w"));
    h.none(Event::Abort { hold });
    assert_eq!(io(h.one(Event::Push { hold, message: message() }), hold).kind(), Kind::Commit);
}

#[test]
fn the_facts_tell_what_happened_and_drop_what_does_not_fit() {
    let mut h = Harness::new(LIMITS);
    h.facts();
    let (hold, _) = h.prepare(1, one(b"w"));
    h.next(hold, Done::Succeeded);
    let client = Token::new(1);
    let facts = h.facts();
    let expected = [
        Fact::Held { client, cached: Cached::New },
        Fact::Started { client, op: Kind::Make },
        Fact::Ended { client, done: Done::Succeeded },
        Fact::Started { client, op: Kind::Clone },
    ];
    assert_eq!(facts.as_slice(), &expected);
    let mut h = Harness::new(Limits { facts: 1, ..LIMITS });
    h.prepare(1, one(b"w"));
    assert_eq!(h.domain.facts_lost(), 1);
}

#[test]
fn the_worst_case_is_bounded_or_refused() {
    let bytes = worst_case(&LIMITS).expect("the test limits fit");
    let named = u64::from(LIMITS.name_bytes);
    // Every workspace's key, twice, and every hold's spec at its limits.
    assert!(bytes > u64::from(LIMITS.workspaces) * 6 * named, "the payloads are counted");
    let told = worst_case(&Limits { facts: LIMITS.facts + 1, ..LIMITS }).expect("the test limits fit");
    let fact = u64::try_from(size_of::<Fact>()).expect("a size fits");
    assert_eq!(told - bytes, fact, "the facts' queue is counted, and nothing else of theirs");
    assert_eq!(worst_case(&Limits { workspaces: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { repositories: 0, ..LIMITS }), None);
    assert_eq!(worst_case(&Limits { workspaces: u32::MAX, name_bytes: u32::MAX, ..LIMITS }), None);
}
