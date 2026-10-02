//! Memory stays within the worst case (programming-style.md, 6.4), measured by
//! a counting allocator: the working set full, every inbox full, every read
//! and write in hand at its limits; and every entry point on the way, under
//! answers as large as the limits allow and failures of every kind.

use temper_engine_model_forge::api::{
    Answer, Body, Check, Comment, Error, Kind, Mark, Op, Page, PageName, Permission, Pull, Remark, Review, State,
    Status, Summary, Verdict,
};
use temper_engine_model_forge::{
    Cause, Config, Content, Event, Item, Limits, Model, Position, Read, Request, Write, fire, max_out, resume, step,
    worst_case,
};
use temper_lib::{Duration, Env, Queue, Rng, Time, Token};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const ENGINE: u64 = 1;

const LIMITS: Limits = Limits {
    repositories: 2,
    items: 4,
    labels: 3,
    members: 3,
    inbox: 3,
    reviewers: 3,
    reads: 2,
    writes: 3,
    calls: 4,
    page: 3,
    name_bytes: 24,
    title_bytes: 24,
    body_bytes: 64,
    rate: 1_000,
    window: Duration::from_secs(60),
    reserve: 100,
    poll: Duration::from_secs(30),
    hinted: Duration::from_secs(2),
    resolution: Duration::from_secs(1),
    slow: Duration::from_secs(90),
    probes: 2,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(4),
    attempts: 3,
    lifetime: Duration::from_secs(4),
    facts: 16,
};

/// A call the sub-model made, as the test answers it: what it asked, without
/// its payload, which is the protocol layer's to count and is dropped at once.
#[derive(Clone, Copy, Debug)]
struct Call {
    call: Token,
    op: Asked,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    Items { page: u32 },
    Item { number: u64, after: u64 },
    Comment { id: u64 },
    Pull { number: u64 },
    Reviews { number: u64, page: u32 },
    Statuses,
    Permission,
    Branch,
    Pages,
    Page,
    Created,
    Commented,
    Edited,
    Merged,
    Reviewed,
    Remarks,
    Revision { put: bool },
    Done,
}

fn asked(op: &Op) -> Asked {
    match op {
        Op::Items { page, .. } => Asked::Items { page: *page },
        Op::Item { number, after } => Asked::Item { number: *number, after: *after },
        Op::Comment { id, .. } => Asked::Comment { id: *id },
        Op::Pull { number } => Asked::Pull { number: *number },
        Op::Reviews { number, page } => Asked::Reviews { number: *number, page: *page },
        Op::Remarks { .. } => Asked::Remarks,
        Op::PullFor { .. } => Asked::Pull { number: 4 },
        Op::Statuses { .. } => Asked::Statuses,
        Op::Permission { .. } => Asked::Permission,
        Op::Branch { .. } => Asked::Branch,
        Op::Pages { .. } => Asked::Pages,
        Op::Page { .. } => Asked::Page,
        Op::CreateIssue { .. } | Op::OpenPull { .. } => Asked::Created,
        Op::Post { .. } => Asked::Commented,
        Op::EditComment { .. } => Asked::Edited,
        Op::Merge { .. } => Asked::Merged,
        Op::Review { .. } => Asked::Reviewed,
        Op::PutPage { content, .. } => Asked::Revision { put: matches!(content, Body::Payload(_)) },
        Op::AddLabels { .. }
        | Op::RemoveLabels { .. }
        | Op::SetReviewers { .. }
        | Op::SetDependencies { .. }
        | Op::Reopen { .. }
        | Op::Close { .. }
        | Op::DeleteBranch { .. }
        | Op::DeletePage { .. } => Asked::Done,
    }
}

/// The sub-model under `limits`, measured: each step's peak is checked
/// against the worst case, less what it handed out in requests, which their
/// receivers count.
struct Measured {
    model: Model,
    env: Env<Limits>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    owners: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        let meter = Meter::new();
        let config = Config {
            engine: ENGINE,
            tracking: name(&limits, b't'),
            hand_in: name(&limits, b'h'),
            projected: labels(&limits),
        };
        let model = Model::new(&limits, config, 7);
        let out = Queue::with_capacity(max_out(&limits));
        Measured { model, env: Env { now: Time::ZERO, limits }, out, meter, bound, owners: 0 }
    }

    fn step(&mut self, event: Event) -> Vec<Call> {
        self.meter.start();
        step(&mut self.model, &self.env, event, &mut self.out);
        self.drain()
    }

    /// Fires the alarms due, and sends the calls ready.
    fn turn(&mut self) -> Vec<Call> {
        let mut calls = Vec::new();
        while self.model.is_due(self.env.now) {
            self.meter.start();
            fire(&mut self.model, &self.env, &mut self.out);
            calls.extend(self.drain());
        }
        while self.model.is_ready() {
            self.meter.start();
            resume(&mut self.model, &self.env, &mut self.out);
            calls.extend(self.drain());
        }
        calls
    }

    fn drain(&mut self) -> Vec<Call> {
        let measured = self.meter.end();
        let mut calls = Vec::new();
        while let Some(request) = self.out.pop() {
            if let Request::Call { call, repository: _, op } = request {
                calls.push(Call { call, op: asked(&op) });
            }
        }
        self.meter.check(measured, self.bound, self.env.limits);
        // The iteration ends: the reclaim point.
        self.model.reclaim();
        calls
    }

    fn owner(&mut self) -> Token {
        self.owners += 1;
        Token::new(self.owners)
    }
}

fn bytes(len: u32, byte: u8) -> Box<[u8]> {
    vec![byte; usize::try_from(len).expect("fits")].into_boxed_slice()
}

fn name(limits: &Limits, byte: u8) -> Box<[u8]> {
    bytes(limits.name_bytes, byte)
}

fn labels(limits: &Limits) -> Box<[Box<[u8]>]> {
    (0..limits.labels).map(|nth| name(limits, b'a' + u8::try_from(nth).expect("few"))).collect()
}

/// The tracking label first, and others, as many as an item carries.
fn tracked(limits: &Limits) -> Box<[Box<[u8]>]> {
    let mut labels = labels(limits).into_vec();
    labels[0] = name(limits, b't');
    labels.into_boxed_slice()
}

fn summary(limits: &Limits, number: u64, updated: u64) -> Summary {
    Summary {
        number,
        kind: if number.is_multiple_of(2) { Kind::Pull } else { Kind::Issue },
        state: State::Open,
        author: 9,
        key: Some(name(limits, b'k')),
        labels: tracked(limits),
        title: bytes(limits.title_bytes, b'T'),
        body: bytes(limits.body_bytes, b'B'),
        updated: Time::ZERO.saturating_add(Duration::from_secs(updated)),
    }
}

fn comments(limits: &Limits, after: u64, record: bool) -> Box<[Comment]> {
    (1..=u64::from(limits.page))
        .map(|nth| {
            let id = after + nth;
            let (author, mark) = if record && nth == 1 {
                (
                    ENGINE,
                    Mark::Record {
                        position: Position {
                            comment: after,
                            pull_comment: after,
                            reviews: 0,
                            head: Some([1; 32]),
                            ci: temper_engine_model_forge::Ci::Pending,
                        },
                        nonce: id,
                    },
                )
            } else {
                let person = if nth.is_multiple_of(2) { Some(8) } else { None };
                (ENGINE, Mark::Key { key: name(limits, b'k'), person })
            };
            Comment { id, author, created: Time::ZERO, revision: id, mark, body: bytes(limits.body_bytes, b'c') }
        })
        .collect()
}

fn pull(limits: &Limits, number: u64, now: u64) -> Pull {
    Pull {
        number,
        state: State::Open,
        head: name(limits, b'h'),
        base: name(limits, b'b'),
        commit: [u8::try_from(now % 200).expect("small"); 32],
        base_commit: Some([4; 32]),
        merged: None,
        mergeable: true,
        ci: temper_engine_model_forge::Ci::Pending,
    }
}

/// A page of reviews, as many as a page holds, by as many reviewers, on the
/// head a pull request read at `now` has.
fn reviews(limits: &Limits, page: u32, now: u64) -> Answer {
    Answer::Reviews {
        reviews: (0..limits.page)
            .map(|nth| Review {
                id: u64::from(page * 10 + nth),
                author: 9 + u64::from(page * 10 + nth),
                verdict: Verdict::Approve,
                commit: [u8::try_from(now % 200).expect("small"); 32],
                key: Some(name(limits, b'k')),
                body: bytes(limits.body_bytes, b'r'),
            })
            .collect(),
        more: page < 3,
    }
}

/// What the forge answers `op`, as large as the limits allow: or a failure
/// drawn from `rng`, of every kind over a run.
fn answer(limits: &Limits, rng: &mut Rng, now: u64, op: Asked) -> Result<Answer, Error> {
    match rng.below(10) {
        0 => return Err(Error::Timeout),
        1 => return Err(Error::Unavailable),
        2 if rng.chance(300) => {
            return Err(Error::RateLimited { after: Duration::from_secs(3) });
        }
        3 if rng.chance(200) => return Err(Error::Missing),
        _ => {}
    }
    let answer = match op {
        Asked::Items { page } => {
            let items =
                (1..=u64::from(limits.page)).map(|nth| summary(limits, nth + u64::from(page) * 2, now)).collect();
            Answer::Items { items, more: page < 3, now: Time::ZERO.saturating_add(Duration::from_secs(now)) }
        }
        Asked::Item { number, after } => Answer::Item {
            item: summary(limits, number, now),
            comments: comments(limits, after, after < 4),
            more: after < 6,
        },
        Asked::Comment { id } => Answer::Comment(comments(limits, id - 1, true).into_vec().remove(0)),
        Asked::Pull { number } => Answer::Pull(pull(limits, number, now)),
        Asked::Reviews { number: _, page } => reviews(limits, page, now),
        Asked::Statuses => Answer::Statuses {
            ci: temper_engine_model_forge::Ci::Passed,
            statuses: (0..limits.page)
                .map(|_| Status {
                    context: name(limits, b's'),
                    check: Check::Passed,
                    description: bytes(limits.title_bytes, b'd'),
                    url: bytes(limits.title_bytes, b'u'),
                })
                .collect(),
            more: true,
        },
        Asked::Permission => Answer::Permission(Permission::Write),
        Asked::Branch => Answer::Commit([5; 32]),
        Asked::Pages => Answer::Pages {
            pages: (0..limits.page).map(|_| PageName { name: name(limits, b'p'), revision: 1 }).collect(),
            next: Some(name(limits, b'p')),
        },
        Asked::Page => Answer::Page(Page {
            name: name(limits, b'p'),
            content: bytes(limits.body_bytes, b'w'),
            revision: 1,
            nonce: Some(3),
        }),
        Asked::Created => Answer::Created(rng.below(50)),
        Asked::Commented => Answer::Commented { id: rng.below(50), revision: 7 },
        Asked::Edited => Answer::Edited { revision: 8 },
        Asked::Merged => Answer::Merged([6; 32]),
        Asked::Reviewed => Answer::Reviewed(rng.below(50)),
        Asked::Remarks => Answer::Remarks {
            remarks: (0..limits.page)
                .map(|nth| Remark {
                    id: u64::from(nth),
                    author: 9,
                    path: name(limits, b'f'),
                    line: nth,
                    body: bytes(limits.body_bytes, b'm'),
                })
                .collect(),
            more: true,
        },
        Asked::Revision { .. } => Answer::Revision(2),
        Asked::Done => Answer::Done,
    };
    Ok(answer)
}

/// The writes of every kind, at the limits.
fn writes(limits: &Limits, item: Item, payload: Token) -> Vec<Write> {
    let text = || Content::Text(bytes(limits.body_bytes, b'x'));
    vec![
        Write::CreateIssue {
            repository: 1,
            key: name(limits, b'k'),
            title: bytes(limits.title_bytes, b'T'),
            body: text(),
            labels: labels(limits),
        },
        Write::Comment { item, key: name(limits, b'k'), person: Some(8), body: text() },
        Write::Review { item, key: name(limits, b'k'), verdict: Verdict::Approve, body: text() },
        Write::SetReviewers { item, reviewers: (0..u64::from(limits.members)).collect() },
        Write::SetDependencies { item, dependencies: (0..u64::from(limits.members)).collect() },
        Write::Reopen { item },
        Write::Record { item, payload },
        Write::SetLabels { item, labels: labels(limits) },
        Write::OpenPull {
            repository: item.repository,
            title: bytes(limits.title_bytes, b'T'),
            body: Content::Payload(payload),
            head: name(limits, b'h'),
            base: name(limits, b'b'),
        },
        Write::Merge { item, head: [2; 32] },
        Write::Close { item },
        Write::DeleteBranch { repository: 0, branch: name(limits, b'h') },
        Write::PutPage { repository: 0, name: name(limits, b'p'), content: text(), revision: Some(1) },
        Write::DeletePage { repository: 0, name: name(limits, b'p') },
    ]
}

fn reads(limits: &Limits, item: Item) -> Vec<Read> {
    vec![
        Read::Item { item, after: 0 },
        Read::Pull { item },
        Read::PullFor { repository: 0, head: name(limits, b'h'), base: name(limits, b'b') },
        Read::Statuses { repository: 0, commit: [1; 32], page: 2 },
        Read::Reviews { item, page: 2 },
        Read::Remarks { item, review: 3, page: 1 },
        Read::Permission { repository: 1, user: 9 },
        Read::Branch { repository: 0, branch: name(limits, b'h') },
        Read::Pages { repository: 0, after: Some(name(limits, b'p')) },
        Read::Page { repository: 1, name: name(limits, b'p') },
    ]
}

/// Runs the sub-model for `rounds`, answering every call as the limits allow
/// and failing some, while the parent tracks, links, takes, hints, reads and
/// writes at the limits. Returns the most it held between steps.
fn run(limits: Limits, seed: u64, rounds: u64) -> (Measured, u64) {
    let mut model = Measured::new(limits);
    let mut rng = Rng::new(seed);
    let mut pending: Vec<Call> = Vec::new();
    let mut fullest = 0;
    for round in 0..rounds {
        let now = round * 2;
        model.env.now = Time::ZERO.saturating_add(Duration::from_secs(now));
        pending.extend(model.turn());
        // Answer some of what is out, the rest later.
        let mut kept = Vec::new();
        for call in std::mem::take(&mut pending) {
            if rng.chance(700) {
                let result = answer(&limits, &mut rng, now, call.op);
                let more = model.step(Event::Answered { call: call.call, result });
                kept.extend(more);
            } else {
                kept.push(call);
            }
        }
        pending = kept;
        let item = Item { repository: u32::try_from(rng.below(2)).expect("two"), number: 1 + rng.below(8) };
        let event = match rng.below(9) {
            0 => Event::Track { item },
            1 => Event::Untrack { item },
            2 => Event::Link { item, pull: Some(2 + 2 * rng.below(3)) },
            3 => Event::Took { item, through: rng.below(6) },
            4 => {
                let branch = if rng.chance(500) { Some(name(&limits, b'b')) } else { None };
                Event::Hint { repository: item.repository, item: Some(item.number), commit: Some([1; 32]), branch }
            }
            5 => {
                let all = reads(&limits, item);
                let read = all.into_iter().nth(usize::try_from(rng.below(10)).expect("few")).expect("ten");
                Event::Read { owner: model.owner(), read }
            }
            _ => {
                let payload = model.owner();
                let all = writes(&limits, item, payload);
                let write = all.into_iter().nth(usize::try_from(rng.below(14)).expect("few")).expect("fourteen");
                let resumed =
                    if rng.chance(300) { Some(Cause { comment: rng.below(9), at: Time::ZERO }) } else { None };
                Event::Write { owner: model.owner(), write, resumed }
            }
        };
        pending.extend(model.step(event));
        if pending.is_empty() {
            // Nothing of the test's own is live: what is held is the
            // sub-model's.
            fullest = fullest.max(model.meter.held());
        }
    }
    (model, fullest)
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    for seed in 0..20 {
        let (model, fullest) = run(LIMITS, seed, 400);
        assert!(fullest <= model.bound, "seed {seed}: {fullest} held of a worst case of {}", model.bound);
    }
}

#[test]
fn the_sub_model_full_to_its_limits_stays_within_its_worst_case() {
    // Room for many news and calls, so the working set, its inboxes and the
    // reads and writes in hand fill up.
    let limits = Limits { inbox: 6, calls: 8, ..LIMITS };
    let mut most = 0;
    let mut bound = 0;
    for seed in 0..10 {
        let (model, fullest) = run(limits, seed, 600);
        most = most.max(fullest);
        bound = model.bound;
        assert!(model.model.items() > 0, "seed {seed}: items were held");
    }
    // At its fullest, the sub-model holds a fair share of the bound: it is
    // not a bound by orders of magnitude.
    assert!(most.saturating_mul(4) > bound, "{most} held of a worst case of {bound}");
}

#[test]
fn bodies_of_payloads_are_named_not_held() {
    let mut model = Measured::new(LIMITS);
    // Its first moment, and the wait for what an earlier life asked for.
    model.turn();
    model.env.now = Time::ZERO.saturating_add(LIMITS.lifetime);
    let write = Write::PutPage {
        repository: 0,
        name: name(&LIMITS, b'p'),
        content: Content::Payload(Token::new(5)),
        revision: None,
    };
    let owner = model.owner();
    let calls = model.step(Event::Write { owner, write, resumed: None });
    assert!(calls.is_empty(), "calls go out on resume");
    let calls = model.turn();
    let check = calls.iter().find(|call| call.op == Asked::Page).expect("the page read first");
    let calls = model.step(Event::Answered { call: check.call, result: Err(Error::Missing) });
    assert!(calls.is_empty(), "calls go out on resume");
    let calls = model.turn();
    let put = calls.iter().find(|call| matches!(call.op, Asked::Revision { .. })).expect("the write went out");
    assert_eq!(put.op, Asked::Revision { put: true }, "a payload named by its token");
}
