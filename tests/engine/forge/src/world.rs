use crate::translate;
use skein_lib::{Duration, Env, List, Map, Queue, ReplyTo, Time, Token, Wall};
use temper_engine_domain_forge as top;
use temper_engine_domain_forge_client as client;
use temper_fake_forge_domain::{self as forge, api as raw};

pub const REPO: client::api::Repository = client::api::Repository { forge: 1, repository: 2 };
const CLIENT: client::Limits = client::Limits {
    pending: 4,
    calls: 2,
    rate: 100,
    reserve: 0,
    window: Duration::from_secs(10),
    op_bytes: 512,
    answer_bytes: 4096,
    rows: 8,
    inbox: 16,
    read_attempts: 3,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(4),
    entries: 8,
    write_attempts: 3,
    lifetime: Duration::from_secs(10),
    resources: 8,
    repositories: 2,
    poll: Duration::from_secs(5),
    poll_max: Duration::from_secs(30),
    hinted: Duration::from_secs(1),
    slow: Duration::from_secs(40),
    facts: 16,
};
pub const LIMITS: top::Limits = top::Limits {
    repositories: 2,
    tasks: 8,
    holds: 8,
    subscriptions: 8,
    entries: 8,
    changes: 8,
    issues: 8,
    resources_per_task: 4,
    paths_per_subscription: 4,
    name_bytes: 128,
    output: 64,
    facts: 16,
    adoptions: 2,
    collaborators: 8,
    landings: 8,
    brief_sections: 32,
    brief_bytes: 4096,
    client: CLIENT,
    issue_policy: temper_engine_domain_forge_issues::Limits {
        plan_items: 8,
        milestones: 8,
        title_bytes: 128,
        body_bytes: 512,
        comment_bytes: 256,
        interval: Duration::from_secs(5),
    },
    change_policy: temper_engine_domain_forge_change::Limits {
        repairs: 3,
        resolutions: 3,
        updates: 3,
        stall: Duration::from_secs(30),
        gates: 8,
        clean_heads: 8,
    },
    queue_window: Duration::from_secs(30),
};

#[derive(Debug)]
struct Pending {
    client: Token,
    op: client::api::Op,
    generation: u64,
}

/// Scripted root and forge protocol surrounding the connector top.
pub struct World {
    top: top::Domain,
    env: Env<top::Limits>,
    forge: forge::Domain,
    forge_env: Env<forge::Config>,
    to_forge: Queue<forge::Event>,
    forge_out: Queue<forge::Request>,
    to_top: Queue<(u64, top::Event)>,
    top_out: Queue<top::Request>,
    pending: Map<Token, Pending>,
    stored: Map<top::Key, top::Stored>,
    seen: List<top::Request>,
    serial: u64,
    generation: u64,
    seed: u64,
    writes: u32,
    calls: u32,
}

impl World {
    #[must_use]
    pub fn new(seed: u64) -> World {
        Self::with_facts(seed, LIMITS.facts)
    }

    #[must_use]
    pub fn with_facts(seed: u64, facts: u32) -> World {
        let limits = top::Limits { facts, client: client::Limits { facts, ..CLIENT }, ..LIMITS };
        let config = fake_config();
        let mut fake = forge::Domain::new(&config, seed);
        forge::repository(
            &mut fake,
            &config,
            raw::Setup {
                name: Box::from(&b"org/repo"[..]),
                default: Box::from(&b"main"[..]),
                tree: Box::new([]),
                labels: Box::new([]),
                checks: raw::Checks {
                    contexts: Box::new([Box::from(&b"build"[..])]),
                    latency_min: Duration::ZERO,
                    latency_max: Duration::ZERO,
                    silent: 0,
                    passes: 1000,
                    reruns: 0,
                    cue: None,
                },
                protection: None,
                hooked: false,
            },
        );
        forge::grant(&mut fake, b"org/repo", 1, raw::Permission::Admin);
        forge::grant(&mut fake, b"org/repo", 2, raw::Permission::Write);
        let top = top::Domain::new(
            &limits,
            seed,
            client::Config {
                namespace: Box::from(&b"world"[..]),
                writers: Box::new([client::Writer { forge: REPO.forge, author: 1 }]),
            },
        )
        .expect("valid connector configuration");
        let now = Time::from_nanos(100_000_000_000);
        let mut world = World {
            top,
            env: Env { now, wall: Wall::from_nanos(now.as_nanos()), limits },
            forge: fake,
            forge_env: Env { now, wall: Wall::EPOCH, limits: config },
            to_forge: Queue::with_capacity(32),
            forge_out: Queue::with_capacity(forge::MAX_OUT),
            to_top: Queue::with_capacity(32),
            top_out: Queue::with_capacity(top::max_out(&limits)),
            pending: Map::with_capacity(32),
            stored: Map::with_capacity(128),
            seen: List::with_capacity(256),
            serial: 0,
            generation: 1,
            seed,
            writes: 0,
            calls: 0,
        };
        world.event(top::Event::Restored { clock: client::RecoveryClock::Monotonic });
        world
    }

    pub fn event(&mut self, event: top::Event) {
        top::step(&mut self.top, &self.env, event, &mut self.top_out);
        self.take_top();
        self.top.reclaim();
    }

    pub fn adopt(&mut self) {
        self.adopt_as(top::Role::Owned);
    }

    pub fn adopt_as(&mut self, role: top::Role) {
        self.event(top::Event::Adopt {
            reply_to: Token::new(100),
            adoption: top::Adoption {
                project: 5,
                home: role != top::Role::Context,
                provider: REPO,
                host: Box::from(&b"forge.example"[..]),
                owner: Box::from(&b"org"[..]),
                name: Box::from(&b"repo"[..]),
                prefix: Box::from(&b"temper/"[..]),
                role,
                landing: Box::from(&b"main"[..]),
                ci: true,
                checks: Box::new([]),
            },
        });
        self.run_for(1);
    }

    #[must_use]
    pub fn seen(&self) -> &[top::Request] {
        self.seen.as_slice()
    }
    pub fn take_seen(&mut self) -> Box<[top::Request]> {
        let taken = core::mem::replace(&mut self.seen, List::with_capacity(256));
        taken.into_boxed()
    }
    #[must_use]
    pub fn writes(&self) -> u32 {
        self.writes
    }
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls
    }
    #[must_use]
    pub fn stored(&self) -> &Map<top::Key, top::Stored> {
        &self.stored
    }
    pub fn slow_calls(&mut self, delay: Duration) {
        self.forge_env.limits.latency_min = delay;
        self.forge_env.limits.latency_max = delay;
    }
    pub fn restart(&mut self) {
        self.generation += 1;
        self.top = top::Domain::new(
            &self.env.limits,
            self.seed,
            client::Config {
                namespace: Box::from(&b"world"[..]),
                writers: Box::new([client::Writer { forge: REPO.forge, author: 1 }]),
            },
        )
        .expect("same configured connector");
        let mut rows = List::with_capacity(self.stored.len());
        for (_, record) in &self.stored {
            rows.push(record.clone()).expect("store snapshot capacity");
        }
        for record in rows.into_boxed() {
            self.event(top::Event::Restore { record });
        }
        self.event(top::Event::Restored { clock: client::RecoveryClock::Monotonic });
    }

    /// Script a worker's pushed branch through the independent fake git.
    pub fn produce(&mut self, branch: &[u8]) {
        self.produce_file(branch, b"file", b"change");
    }
    pub fn produce_file(&mut self, branch: &[u8], path: &[u8], content: &[u8]) {
        self.external(raw::Op::Git(raw::Git::Create { branch: Box::from(branch), commit: 1 }), 2);
        self.push_file(branch, path, content);
    }
    pub fn preload_history(&mut self, count: u32) {
        for number in 0..count {
            self.external(
                raw::Op::Write(raw::Write::CreateIssue {
                    title: Box::from(&b"old"[..]),
                    body: Box::from(number.to_be_bytes()),
                    labels: Box::new([]),
                }),
                2,
            );
        }
    }
    pub fn push(&mut self, branch: &[u8], content: &[u8]) {
        self.push_file(branch, b"file", content);
    }
    pub fn push_file(&mut self, branch: &[u8], path: &[u8], content: &[u8]) {
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: self.forge_env.limits };
        forge::advance(&mut self.forge, &env, b"org/repo", branch, path, content, 1).expect("worker push");
    }
    pub fn status(&mut self, branch: &[u8], check: raw::Check) {
        self.external(
            raw::Op::Write(raw::Write::Status {
                commit: self.branch(branch),
                context: Box::from(&b"build"[..]),
                state: check,
            }),
            1,
        );
    }
    pub fn open_pull(&mut self, head: &[u8], base: &[u8]) -> u64 {
        let raw::Answer::Created(number) = self.external(
            raw::Op::Write(raw::Write::OpenPull {
                title: Box::from(&b"Change"[..]),
                body: Box::from(&b"Description"[..]),
                head: Box::from(head),
                base: Box::from(base),
            }),
            2,
        ) else {
            panic!("fixture pull opened")
        };
        number
    }
    pub fn close(&mut self, number: u64) {
        self.external(raw::Op::Write(raw::Write::Close { number }), 2);
    }
    pub fn resolve(&mut self, branch: &[u8], content: &[u8]) {
        let head = self.branch(branch);
        let base = self.branch(b"main");
        let merged = forge::merge_commit(
            &mut self.forge,
            &self.forge_env.limits,
            head,
            base,
            Box::new([raw::File { path: Box::from(&b"file"[..]), content: Box::from(content) }]),
            b"Resolve conflict",
        )
        .expect("fixture merge commit");
        assert_eq!(
            self.external(
                raw::Op::Git(raw::Git::Push { branch: Box::from(branch), commit: merged, expected: Some(head) }),
                1,
            ),
            raw::Answer::Pushed(raw::Pushed::Pushed)
        );
    }
    #[must_use]
    pub fn branch(&self, branch: &[u8]) -> u64 {
        self.forge.branch(b"org/repo", branch).expect("fixture branch")
    }

    fn external(&mut self, op: raw::Op, user: u64) -> raw::Answer {
        let mut config = self.forge_env.limits;
        config.latency_min = Duration::ZERO;
        config.latency_max = Duration::ZERO;
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
        self.serial += 1;
        let token = Token::new(self.serial);
        forge::step(
            &mut self.forge,
            &env,
            forge::Event::Call { reply_to: ReplyTo::new(token), user, repository: Box::from(&b"org/repo"[..]), op },
            &mut self.forge_out,
        );
        for _ in 0..4 {
            if !self.forge.is_due(env.now) {
                break;
            }
            forge::fire(&mut self.forge, &env, &mut self.forge_out);
        }
        let mut answer = None;
        for _ in 0..self.forge_out.len() {
            match self.forge_out.pop().expect("fake output") {
                forge::Request::Reply { result, .. } => answer = Some(result.expect("fixture succeeds")),
                forge::Request::Hook { .. } => {}
            }
        }
        self.forge.reclaim();
        answer.expect("fixture answered")
    }

    pub fn run_for(&mut self, seconds: u64) {
        let end = self.env.now.saturating_add(Duration::from_secs(seconds));
        for _ in 0..20_000 {
            if !self.iterate(end) {
                return;
            }
        }
        panic!("world did not reach horizon");
    }

    fn iterate(&mut self, end: Time) -> bool {
        if let Some((generation, event)) = self.to_top.pop() {
            if generation == self.generation {
                self.event(event);
            }
        } else if let Some(event) = self.to_forge.pop() {
            forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
            self.take_forge();
        } else if self.top.is_ready() {
            top::resume(&mut self.top, &self.env, &mut self.top_out);
            self.take_top();
            self.top.reclaim();
        } else if self.forge.is_due(self.env.now) {
            forge::fire(&mut self.forge, &self.forge_env, &mut self.forge_out);
            self.take_forge();
        } else if self.top.next_deadline().is_some_and(|time| time <= self.env.now) {
            top::fire(&mut self.top, &self.env, &mut self.top_out);
            self.take_top();
            self.top.reclaim();
        } else {
            if self.env.now >= end {
                return false;
            }
            let next = self.top.next_deadline().unwrap_or(end).min(self.forge.next_deadline().unwrap_or(end)).min(end);
            self.env.now = next;
            self.env.wall = Wall::from_nanos(next.as_nanos());
            self.forge_env.now = next;
        }
        self.forge.reclaim();
        true
    }

    fn take_top(&mut self) {
        let mut calls = List::with_capacity(self.top_out.len());
        for _ in 0..self.top_out.len() {
            let request = self.top_out.pop().expect("counted top output");
            match request {
                top::Request::BriefClient { event } => {
                    top::step(&mut self.top, &self.env, top::Event::Client(event), &mut self.top_out);
                }
                top::Request::Save { record } => {
                    let key = key(&record);
                    self.stored.insert(key, record).expect("world store capacity");
                }
                top::Request::Erase { key } => {
                    self.stored.remove(&key).expect("erased row exists");
                }
                top::Request::Call { call, repository, op } => {
                    calls.push((call, repository, op)).expect("bounded calls");
                }
                other @ (top::Request::Adopted { .. }
                | top::Request::Taken { .. }
                | top::Request::Refused { .. }
                | top::Request::Outcome { .. }
                | top::Request::ContinueRelease { .. }
                | top::Request::Released { .. }
                | top::Request::ReleaseFailed { .. }
                | top::Request::ProjectAfter { .. }
                | top::Request::ProjectionFailed { .. }
                | top::Request::ChangeDecision { .. }
                | top::Request::News { .. }
                | top::Request::Read { .. }
                | top::Request::BriefReady { .. }
                | top::Request::Drift { .. }) => {
                    self.seen.push(other).expect("world output capacity");
                }
            }
        }
        for (call, repository, op) in calls.into_boxed() {
            self.submit(call, repository, op);
        }
    }

    fn submit(&mut self, call: Token, repository: client::api::Repository, op: client::api::Op) {
        assert_eq!(repository, REPO);
        self.calls += 1;
        if let client::api::Op::Write(_) = op {
            self.writes += 1;
        }
        self.serial += 1;
        let token = Token::new(self.serial);
        let raw = translate::op(&op, &self.env.limits.client);
        self.pending
            .insert(token, Pending { client: call, op, generation: self.generation })
            .expect("world call slots");
        self.to_forge.push(forge::Event::Call {
            reply_to: ReplyTo::new(token),
            user: 1,
            repository: Box::from(&b"org/repo"[..]),
            op: raw,
        });
    }

    fn take_forge(&mut self) {
        for _ in 0..self.forge_out.len() {
            match self.forge_out.pop().expect("counted fake output") {
                forge::Request::Reply { to, result } => {
                    let pending = self.pending.remove(&to.into_token()).expect("pending call");
                    let result = match result {
                        Ok(answer) => Ok(translate::answer(&pending.op, answer, &self.env.limits.client)),
                        Err(error) => {
                            Err(translate::error(error, forge::time(&self.forge_env.limits, self.forge_env.now)))
                        }
                    };
                    self.to_top.push((
                        pending.generation,
                        top::Event::Client(client::Event::Answered { call: pending.client, cost: 1, result }),
                    ));
                }
                forge::Request::Hook { .. } => {}
            }
        }
    }
}

fn key(row: &top::Stored) -> top::Key {
    match row {
        top::Stored::Repository(row) => top::Key::Repository(row.provider),
        top::Stored::Hold(row) => top::Key::Hold(row.name.clone()),
        top::Stored::Names { task, .. } => top::Key::Names(*task),
        top::Stored::Subscription(row) => top::Key::Subscription { task: row.task, topic: row.topic.clone() },
        top::Stored::BranchHead(row) => top::Key::BranchHead(row.name.clone()),
        top::Stored::PullState(row) => top::Key::PullState(row.name.clone()),
        top::Stored::Ci(row) => top::Key::Ci { repository: row.repository, head: row.head },
        top::Stored::Landed { commit, .. } => top::Key::Landed(*commit),
        top::Stored::Entry(row) => top::Key::Entry(row.number),
        top::Stored::Client(row) => top::Key::Client(match row {
            client::Stored::Live(row) => client::Key::Live(row.watch.resource.clone()),
            client::Stored::Repository(row) => client::Key::Repository(row.repository),
        }),
        top::Stored::Change(row) => top::Key::Change(row.task),
        top::Stored::Issue(row) => top::Key::Issue(row.goal),
        top::Stored::Release(row) => top::Key::Release(row.task),
    }
}

/// Bounded Forgejo fixture shared by the connector and root worlds.
#[must_use]
pub fn fake_config() -> forge::Config {
    forge::Config {
        limits: forge::Limits {
            repositories: 1,
            users: 3,
            labels: 2,
            items: 64,
            comments: 32,
            dependencies: 2,
            reviews: 8,
            branches: 4,
            commits: 16,
            files: 4,
            statuses: 4,
            contexts: 4,
            pages: 1,
            name_bytes: 64,
            title_bytes: 64,
            body_bytes: 1024,
            content_bytes: 128,
            page_size: 2,
            calls: 4,
            hooks: 8,
            observations: 64,
        },
        latency_min: Duration::from_millis(10),
        latency_max: Duration::from_millis(50),
        late: 0,
        late_min: Duration::from_secs(1),
        late_max: Duration::from_secs(3),
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::from_secs(1),
        land_max: Duration::from_secs(3),
        rate_limit: 0,
        rate_window: Duration::from_secs(10),
        ci: 3,
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: forge::Skew::Ahead(Duration::from_secs(20)),
        status_updates: false,
        edit_updates: false,
    }
}
