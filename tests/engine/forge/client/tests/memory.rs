//! Count every client entry point at saturation, including owned cache,
//! delivery stamps, echo ledgers, queued operations and durable retry
//! payloads. Parent-owned output payloads are handed away before checking.
use skein_lib::{Env, List, Queue, Time, Token, Wall};
use temper_engine_domain_forge_client::{
    self as client, Condition, Delivery, Domain, Echo, Effect, Entry, Event, Limits, LiveRecord, Position,
    RecoveryClock, RepositoryRecord, Request, Resource, Stored, Watch, What, api,
};
use temper_engine_forge_client_world::{LIMITS, REPO};
use temper_world::heap::{self, Meter};
#[global_allocator]
static HEAP: heap::Counting = heap::Counting;
#[derive(Clone, Copy)]
struct Call {
    token: Token,
    asked: Asked,
}
#[derive(Clone, Copy)]
enum Asked {
    Items,
    Item(u64),
    Pull(u64),
    Reviews,
    Branch,
    Create,
    Comment,
    Review,
    Merge,
    Compare(api::Commit, api::Commit),
    Done,
}
struct Measured {
    domain: Domain,
    env: Env<Limits>,
    out: Queue<Request>,
    calls: Queue<Call>,
    meter: Meter,
    bound: u64,
}
impl Measured {
    fn new(limits: Limits) -> Measured {
        let out = Queue::with_capacity(client::max_out(&limits));
        let calls = Queue::with_capacity(limits.pending);
        let meter = Meter::new();
        let writers = Box::new([client::Writer { forge: REPO.forge, author: 1 }]);
        let namespace = bytes(
            limits
                .op_bytes
                .saturating_sub(u32::try_from(core::mem::size_of::<client::Writer>()).expect("writer size fits"))
                .min(u32::from(u16::MAX)),
            3,
        );
        let domain =
            Domain::configured(&limits, 0, client::Config { namespace, writers }).expect("bounded root config");
        Measured {
            domain,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out,
            calls,
            meter,
            bound: client::worst_case(&limits).expect("memory limits fit"),
        }
    }
    fn event(&mut self, event: Event) {
        self.meter.start();
        client::step(&mut self.domain, &self.env, event, &mut self.out);
        self.drain();
    }
    fn resume(&mut self) {
        self.meter.start();
        client::resume(&mut self.domain, &self.env, &mut self.out);
        self.drain();
    }
    fn drain(&mut self) {
        let measured = self.meter.end();
        for _ in 0..self.out.len() {
            match self.out.pop().expect("output exists") {
                Request::Call { call, op, .. } => {
                    self.calls.push(Call { token: call, asked: asked(&op) });
                }
                Request::Save { .. }
                | Request::Progress { .. }
                | Request::Erase { .. }
                | Request::Outcome { .. }
                | Request::Changed { .. }
                | Request::Drift { .. }
                | Request::Kept { .. }
                | Request::Read { .. }
                | Request::ReadAfreshDone
                | Request::OutboxDone => {}
            }
        }
        self.meter.check(measured, self.bound, self.env.limits);
        self.domain.reclaim();
    }
    fn answer(&mut self, call: Call) {
        let rows = self.env.limits.rows;
        let result = match call.asked {
            Asked::Items => api::Answer::Items { items: Box::new([]), more: false, now: self.env.now },
            Asked::Item(number) => api::Answer::Item {
                item: summary(number, self.env.limits.answer_bytes.saturating_sub(256)),
                comments: Box::new([]),
                more: false,
            },
            Asked::Pull(number) => api::Answer::Pull(api::Pull {
                number,
                state: api::State::Open,
                head: bytes(self.env.limits.op_bytes / 2, 1),
                base: bytes(self.env.limits.op_bytes / 2, 2),
                commit: [1; 32],
                base_commit: Some([2; 32]),
                merged: None,
                mergeable: true,
                ci: api::Ci::Pending,
                reviewers: ids(rows),
            }),
            Asked::Reviews => {
                let mut reviews = List::with_capacity(rows);
                for id in 1..=u64::from(rows) {
                    reviews
                        .push(api::Review {
                            provenance: api::Provenance::Original,
                            id,
                            revision: 1,
                            author: 2,
                            verdict: api::Verdict::Comment,
                            commit: [1; 32],
                            key: None,
                            body: bytes(self.env.limits.answer_bytes / rows / 2, 3),
                            at: self.env.now,
                            official: false,
                        })
                        .expect("review capacity");
                }
                api::Answer::Reviews { reviews: reviews.into_boxed(), more: false }
            }
            Asked::Branch => api::Answer::Commit([1; 32]),
            Asked::Create => api::Answer::Created(9),
            Asked::Comment => api::Answer::Commented(7),
            Asked::Review => api::Answer::Reviewed(7),
            Asked::Merge => api::Answer::Merged([3; 32]),
            Asked::Compare(before, after) => api::Answer::Compare {
                before,
                after,
                contains_before: false,
                files: Box::new([]),
                commits: Box::new([]),
            },
            Asked::Done => api::Answer::Done,
        };
        self.event(Event::Answered { call: call.token, cost: 1, result: Ok(result) });
    }
    fn drive(&mut self) {
        for _ in 0..100 {
            if let Some(call) = self.calls.pop() {
                self.answer(call);
            } else if self.domain.is_ready() {
                self.resume();
            } else {
                break;
            }
        }
    }
}
fn asked(op: &api::Op) -> Asked {
    match op {
        api::Op::Read(read) => match read {
            api::Read::Items { .. } => Asked::Items,
            api::Read::Item { number, .. } => Asked::Item(*number),
            api::Read::Pull { number } => Asked::Pull(*number),
            api::Read::Reviews { .. } => Asked::Reviews,
            api::Read::Branch { .. } => Asked::Branch,
            api::Read::Compare { before, after } => Asked::Compare(*before, *after),
            api::Read::PullFor { .. }
            | api::Read::Branches
            | api::Read::Statuses { .. }
            | api::Read::Remarks { .. }
            | api::Read::PullFiles { .. }
            | api::Read::Checks { .. }
            | api::Read::Job { .. }
            | api::Read::Protection { .. }
            | api::Read::Settings
            | api::Read::Collaborators { .. }
            | api::Read::Permission { .. } => panic!("memory scenario route"),
        },
        api::Op::Write(write) => match write {
            api::Write::CreateIssue { .. } | api::Write::OpenPull { .. } => Asked::Create,
            api::Write::Post { .. } => Asked::Comment,
            api::Write::Review { .. } => Asked::Review,
            api::Write::Merge { .. } => Asked::Merge,
            api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::DeleteBranch { .. } => Asked::Done,
        },
    }
}
fn bytes(count: u32, byte: u8) -> Box<[u8]> {
    let mut bytes = List::with_capacity(count);
    for _ in 0..count {
        bytes.push(byte).expect("byte capacity");
    }
    bytes.into_boxed()
}
fn ids(count: u32) -> Box<[u64]> {
    let mut ids = List::with_capacity(count);
    for id in 1..=u64::from(count) {
        ids.push(id).expect("ID capacity");
    }
    ids.into_boxed()
}
fn summary(number: u64, body: u32) -> api::Summary {
    api::Summary {
        number,
        kind: api::Kind::Pull,
        state: api::State::Open,
        author: 2,
        key: None,
        title: Box::new([]),
        body: bytes(body, 3),
        labels: Box::new([]),
        updated: Time::ZERO,
    }
}
#[test]
fn full_live_caches_inbox_stamps_echoes_outbox_and_every_entry_point_fit() {
    for limits in [
        LIMITS,
        Limits {
            resources: 1,
            repositories: 1,
            entries: 1,
            pending: 1,
            calls: 1,
            rows: 1,
            inbox: 1,
            facts: 0,
            op_bytes: 128,
            answer_bytes: 768,
            ..LIMITS
        },
    ] {
        let mut m = Measured::new(limits);
        m.event(Event::Restore {
            record: Stored::Repository(RepositoryRecord {
                repository: REPO,
                mark: Some(Time::ZERO),
                clock: Time::ZERO,
            }),
        });
        let watches = restore_participants(&mut m, limits);
        m.event(Event::Restored { clock: RecoveryClock::Monotonic });
        m.drive();
        m.event(Event::Keep { owner: Token::new(10), watches });
        for number in 1..=u64::from(limits.entries) {
            m.event(Event::Make {
                entry: Entry {
                    number,
                    task: 7,
                    repository: REPO,
                    effect: Effect {
                        write: api::Write::Post {
                            number: 1,
                            key: bytes(limits.op_bytes / 4, 5),
                            body: bytes(limits.op_bytes / 2, 6),
                        },
                        condition: Condition::None,
                    },
                    start: None,
                    attempt: None,
                    failures: 0,
                },
            });
        }
        for owner in 1..=u64::from(limits.pending) {
            m.event(Event::Read {
                owner: Token::new(owner),
                repository: REPO,
                read: api::Read::Branch { branch: bytes(limits.op_bytes, 7) },
            });
        }
        for _ in 0..limits.pending {
            m.resume();
        }
        m.drive();
        m.event(Event::Withdraw { entry: 1 });
        m.event(Event::Hint { hint: api::Hint { repository: REPO, change: api::Change::Commit([1; 32]), key: None } });
        m.env.now = Time::from_nanos(60_000_000_000);
        for _ in 0..20 {
            if !m.domain.is_due(m.env.now) {
                break;
            }
            m.meter.start();
            client::fire(&mut m.domain, &m.env, &mut m.out);
            m.drain();
        }
        m.drive();
        m.event(Event::Keep { owner: Token::new(10), watches: Box::new([]) });
        m.drive();
        own_branch(&mut m, limits);
    }
}

fn own_branch(m: &mut Measured, limits: Limits) {
    let branch = Resource { repository: REPO, what: What::Branch(bytes(limits.op_bytes, 8)) };
    m.event(Event::Keep {
        owner: Token::new(11),
        watches: Box::new([Watch { resource: branch.clone(), participating: false }]),
    });
    m.event(Event::Writer { resource: branch.clone(), taken: true });
    m.event(Event::Pushed { resource: branch.clone(), commit: [3; 32] });
    m.event(Event::Writer { resource: branch, taken: false });
    m.drive();
    m.event(Event::Keep { owner: Token::new(12), watches: Box::new([]) });
    m.drive();
}

fn stamps(cap: u32) -> Box<[Delivery]> {
    let mut stamps = List::with_capacity(cap);
    for id in 1..=u64::from(cap) {
        stamps.push(Delivery { id, revision: 1 }).expect("stamp table cap");
    }
    stamps.into_boxed()
}

fn restore_participants(m: &mut Measured, limits: Limits) -> Box<[Watch]> {
    let mut watches = List::with_capacity(limits.resources);
    for number in 1..=u64::from(limits.resources) {
        let watch = Watch { resource: Resource { repository: REPO, what: What::Pull(number) }, participating: true };
        watches.push(watch.clone()).expect("live capacity");
        let mut echoes = List::with_capacity(limits.rows);
        for id in 1..=u64::from(limits.rows) {
            echoes.push(Echo::Review(id)).expect("echo capacity");
        }
        m.event(Event::Restore {
            record: Stored::Live(LiveRecord {
                watch,
                position: Position { at: Time::ZERO, comment: 0, review: 0, review_page: 1 },
                listed: Time::ZERO,
                comment_after: 0,
                comment_page: 1,
                comment_scanning: true,
                comment_repair: true,
                review_page: 1,
                comments: stamps(limits.inbox),
                reviews: stamps(limits.inbox),
                cached: Box::new(client::Cached { item: None, pull: None, tip: None }),
                pushed: None,
                echoes: echoes.into_boxed(),
            }),
        });
    }
    watches.into_boxed()
}
