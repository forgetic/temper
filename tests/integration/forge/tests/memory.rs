//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the forge filled to its limits, every repository
//! holding as many items, comments, reviews, branches, statuses and wiki pages
//! as it may, each as large as it may be, the store as many commits, with
//! calls held, webhooks in flight and observations kept; every step checked.

use temper_forge_model::api::{
    Answer, Check, Checks, Cue, Error, File, Git, Op, Permission, Protection, Read, Setup, Verdict, Write,
};
use temper_forge_model::{Config, Event, Limits, MAX_OUT, Model, Request, Skew, fire, step, worst_case};
use temper_lib::{Duration, Env, Queue, ReplyTo, Time, Token};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    repositories: 2,
    users: 4,
    labels: 2,
    items: 4,
    comments: 2,
    dependencies: 2,
    reviews: 2,
    branches: 4,
    commits: 24,
    files: 2,
    statuses: 4,
    contexts: 2,
    pages: 2,
    name_bytes: 16,
    title_bytes: 16,
    body_bytes: 48,
    content_bytes: 64,
    page_size: 2,
    calls: 3,
    hooks: 8,
    observations: 8,
};

/// Calls answered at once; CI and webhooks so slow that they pile up.
const CONFIG: Config = Config {
    limits: LIMITS,
    latency_min: Duration::from_millis(1),
    latency_max: Duration::from_millis(1),
    late: 0,
    late_min: Duration::from_millis(1),
    late_max: Duration::from_millis(1),
    unavailable: 0,
    timeouts: 0,
    landing: 0,
    land_min: Duration::from_millis(1),
    land_max: Duration::from_millis(1),
    rate_limit: 1000,
    rate_window: Duration::from_secs(3600),
    ci: 9,
    hook_min: Duration::from_secs(3600),
    hook_max: Duration::from_secs(3600),
    hooks_late: 0,
    hooks_lost: 0,
    resolution: Duration::from_secs(1),
    skew: Skew::None,
    status_updates: false,
    edit_updates: false,
};

/// The users of every repository: an admin, then writers. The second opens
/// everything; the others comment and review.
const ADMIN: u64 = 1;
const AUTHOR: u64 = 2;

/// A name as long as names may be: `tag`, then `index`.
fn name(tag: u8, index: u64) -> Box<[u8]> {
    let mut bytes = vec![tag; usize::try_from(LIMITS.name_bytes).expect("small")];
    let at = bytes.len() - 8;
    bytes[at..].copy_from_slice(&index.to_be_bytes());
    bytes.into_boxed_slice()
}

/// Text `len` bytes long.
fn text(len: u32, tag: u8) -> Box<[u8]> {
    vec![tag; usize::try_from(len).expect("small")].into_boxed_slice()
}

fn names(tag: u8, count: u32) -> Box<[Box<[u8]>]> {
    (0..u64::from(count)).map(|index| name(tag, index)).collect()
}

/// A full tree, its contents tagged with `tag`.
fn tree(tag: u8) -> Box<[File]> {
    (0..u64::from(LIMITS.files))
        .map(|index| File { path: name(b'p', index), content: text(LIMITS.content_bytes, tag) })
        .collect()
}

fn setup(index: u64) -> Setup {
    Setup {
        name: name(b'r', index),
        default: name(b'b', 0),
        tree: tree(b'0'),
        labels: names(b'l', LIMITS.labels),
        checks: Checks {
            contexts: names(b'c', LIMITS.contexts),
            latency_min: Duration::from_secs(3600),
            latency_max: Duration::from_secs(3600),
            silent: 0,
            passes: 1000,
            reruns: 1000,
            cue: Some(Cue { path: name(b'p', 0), green: text(LIMITS.content_bytes, b'g') }),
        },
        protection: Some(Protection {
            branch: name(b'z', 0),
            contexts: names(b'c', LIMITS.contexts),
            approvals: 1,
            dismiss_stale: false,
        }),
        hooked: true,
    }
}

/// The forge, measured: each step's peak is checked against the worst case,
/// less what it handed out in requests, which their receivers count.
struct Measured {
    model: Model,
    env: Env<Config>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    calls: u64,
}

impl Measured {
    fn new() -> Measured {
        let bound = worst_case(&LIMITS).expect("the test limits fit");
        let meter = Meter::new();
        let model = Model::new(&CONFIG, 7);
        let out = Queue::with_capacity(MAX_OUT);
        Measured { model, env: Env { now: Time::ZERO, limits: CONFIG }, out, meter, bound, calls: 0 }
    }

    /// Steps a call by `user` on `repository`, measured, without answering
    /// it.
    fn send(&mut self, user: u64, repository: &[u8], op: Op) -> Token {
        self.calls += 1;
        let token = Token::new(self.calls);
        let event = Event::Call { reply_to: ReplyTo::new(token), user, repository: repository.into(), op };
        self.meter.start();
        step(&mut self.model, &self.env, event, &mut self.out);
        let measured = self.meter.end();
        assert!(self.out.is_empty(), "a call is held");
        self.meter.check(measured, self.bound, "a call");
        token
    }

    /// Fires the next timer, measured: what it emits is dropped, but for a
    /// reply's outcome.
    fn fire(&mut self) -> Option<(Token, Result<Option<u64>, Error>)> {
        self.env.now = self.model.next_deadline().expect("a timer is armed");
        self.meter.start();
        fire(&mut self.model, &self.env, &mut self.out);
        let measured = self.meter.end();
        let outcome = match self.out.pop() {
            Some(Request::Reply { to, result }) => Some((to.into_token(), outcome(result))),
            Some(Request::Hook { .. }) | None => None,
        };
        self.meter.check(measured, self.bound, "a timer");
        self.model.reclaim();
        outcome
    }

    /// Calls and waits for the answer.
    fn call(&mut self, user: u64, repository: &[u8], op: Op) -> Result<Option<u64>, Error> {
        let token = self.send(user, repository, op);
        for _ in 0..64 {
            if let Some((to, outcome)) = self.fire()
                && to == token
            {
                return outcome;
            }
        }
        unreachable!("the call is answered");
    }

    fn ok(&mut self, user: u64, repository: &[u8], op: Op) -> Option<u64> {
        self.call(user, repository, op).expect("the call succeeds")
    }
}

/// What a test needs of an answer: the number it gives, if it gives one.
fn outcome(result: Result<Answer, Error>) -> Result<Option<u64>, Error> {
    match result? {
        Answer::Created(number)
        | Answer::Commented(number)
        | Answer::Reviewed(number)
        | Answer::Merged(number)
        | Answer::Revision(number) => Ok(Some(number)),
        Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Comment { .. }
        | Answer::Dependencies(_)
        | Answer::Labels(_)
        | Answer::Commit(_)
        | Answer::Tree(_)
        | Answer::File(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Done
        | Answer::Cloned { .. }
        | Answer::Pushed(_)
        | Answer::Branch(_) => Ok(None),
    }
}

/// Fills the repository `index` with all it may hold.
fn fill(forge: &mut Measured, index: u64, first: u64) {
    let repository = name(b'r', index);
    let config = forge.env.limits;
    // Three branches, each a commit of a full tree ahead of the default.
    for branch in 0..3_u64 {
        let content = b'a' + u8::try_from(branch + 3 * index).expect("small");
        let commit = temper_forge_model::commit(&mut forge.model, &config, first, tree(content))
            .expect("room")
            .expect("a change");
        let push = Op::Git(Git::Push { branch: name(b'w', branch), commit });
        forge.ok(AUTHOR, &repository, push);
    }
    // Two pull requests, and two issues carrying every label.
    for branch in 0..2_u64 {
        let open = Write::OpenPull {
            title: text(LIMITS.title_bytes, b't'),
            body: text(LIMITS.body_bytes, b'b'),
            head: name(b'w', branch),
            base: name(b'b', 0),
        };
        forge.ok(AUTHOR, &repository, Op::Write(open));
    }
    for _ in 0..2 {
        let create = Write::CreateIssue {
            title: text(LIMITS.title_bytes, b't'),
            body: text(LIMITS.body_bytes, b'b'),
            labels: names(b'l', LIMITS.labels),
        };
        forge.ok(AUTHOR, &repository, Op::Write(create));
    }
    for number in 1..=u64::from(LIMITS.items) {
        for user in 3..3 + u64::from(LIMITS.comments) {
            let comment = Write::Comment { number, body: text(LIMITS.body_bytes, b'c') };
            forge.ok(user, &repository, Op::Write(comment));
        }
        let dependencies: Box<[u64]> = (number + 1..=u64::from(LIMITS.items)).take(2).collect();
        forge.ok(AUTHOR, &repository, Op::Write(Write::SetDependencies { number, dependencies }));
    }
    for number in 1..=2 {
        for user in 3..3 + u64::from(LIMITS.reviews) {
            let review = Write::Review { number, verdict: Some(Verdict::Approve), body: text(LIMITS.body_bytes, b'v') };
            forge.ok(user, &repository, Op::Write(review));
        }
        let reviewers = Box::new([ADMIN, 3, 4]);
        forge.ok(ADMIN, &repository, Op::Write(Write::SetReviewers { number, reviewers }));
    }
    for page in 0..u64::from(LIMITS.pages) {
        let put = Write::PutPage { name: name(b'n', page), content: text(LIMITS.content_bytes, b'w') };
        forge.ok(AUTHOR, &repository, Op::Write(put));
    }
    // The default branch's commit has statuses too, of every context.
    for context in 0..u64::from(LIMITS.contexts) {
        let status = Write::Status { commit: first, context: name(b'c', context), state: Check::Passed };
        forge.ok(AUTHOR, &repository, Op::Write(status));
    }
}

#[test]
fn a_forge_filled_to_its_limits_stays_within_its_worst_case() {
    let mut forge = Measured::new();
    let config = forge.env.limits;
    let mut firsts = Vec::new();
    for index in 0..u64::from(LIMITS.repositories) {
        firsts.push(temper_forge_model::repository(&mut forge.model, &config, setup(index)));
        for user in 1..=u64::from(LIMITS.users) {
            let permission = if user == ADMIN { Permission::Admin } else { Permission::Write };
            temper_forge_model::grant(&mut forge.model, &name(b'r', index), user, permission);
        }
    }
    for (index, &first) in firsts.iter().enumerate() {
        fill(&mut forge, u64::try_from(index).expect("small"), first);
    }
    let room = forge.model.room();
    assert_eq!(room.statuses, 0, "every repository holds as many statuses as it may");
    // The store holds as many commits as it may.
    for content in 0..u8::MAX {
        let made = temper_forge_model::commit(&mut forge.model, &config, firsts[0], tree(content));
        if made == Err(Error::Full) {
            break;
        }
    }
    assert_eq!(forge.model.room().commits, 0);
    let tally = forge.model.tally();
    assert_eq!((tally.full, tally.unreported), (0, 0), "nothing was refused for room while filling");
    assert!(forge.model.observations_lost() > 0, "the observations kept are as many as may be");
    assert!(tally.hooks_dropped > 0, "the webhooks in flight are as many as may be");
    // Calls held, each with as large an answer as there is.
    let repository = name(b'r', 0);
    let big = [
        Op::Read(Read::Tree { commit: firsts[0] }),
        Op::Read(Read::Items {
            state: None,
            kind: None,
            labels: Box::new([]),
            author: None,
            since: Time::ZERO,
            page: 2,
            limit: 0,
        }),
        Op::Read(Read::Pull { number: 1 }),
    ];
    for op in big {
        forge.send(ADMIN, &repository, op);
    }
    assert_eq!(forge.model.calls(), LIMITS.calls);
    let held = forge.meter.held();
    assert!(held * 3 >= forge.bound * 2, "the fill reaches two thirds of the worst case: {held} of {}", forge.bound);
}
