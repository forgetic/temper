use crate::{referee::Referee, translate};
use skein_lib::{Duration, Env, List, Map, Queue, ReplyTo, Time, Token, Wall};
use temper_engine_domain_forge_client::{
    self as client, Entry, Event, Key, Limits, Outcome, RecoveryClock, Request, Resource, Stored, Watch, api,
};
use temper_fake_forge_domain::{self as forge, Config, api as raw};
pub const REPO: api::Repository = api::Repository { forge: 0, repository: 0 };
pub const LIMITS: Limits = Limits {
    pending: 4,
    calls: 2,
    rate: 100,
    reserve: 20,
    window: Duration::from_secs(10),
    op_bytes: 256,
    answer_bytes: 4096,
    rows: 2,
    inbox: 32,
    read_attempts: 3,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(4),
    entries: 4,
    write_attempts: 5,
    lifetime: Duration::from_secs(10),
    clock_margin: Duration::from_secs(1),
    resources: 3,
    repositories: 1,
    poll: Duration::from_secs(5),
    poll_max: Duration::from_secs(30),
    hinted: Duration::from_secs(1),
    slow: Duration::from_secs(40),
    facts: 8,
};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub seed: u64,
    pub faults: bool,
    pub facts: u32,
    pub history: u32,
}
impl Settings {
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings { seed, faults: false, facts: 8, history: 0 }
    }
    #[must_use]
    pub const fn random(seed: u64) -> Settings {
        Settings { seed, faults: true, facts: 8, history: 20 }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stats {
    pub digest: u64,
    pub calls: u32,
    pub reads: u32,
    pub writes: u32,
    pub terminals: u32,
    pub restarts: u32,
    pub outcomes: u32,
    pub read_terminals: u32,
    pub creations: u32,
    pub unavailable: u32,
    pub timeouts: u32,
    pub late_landings: u32,
    pub limited: u32,
}
#[derive(Debug)]
struct Pending {
    client: Token,
    op: api::Op,
    generation: u64,
}
struct Delivery {
    generation: u64,
    event: Event,
}
pub struct World {
    client: client::Domain,
    env: Env<Limits>,
    forge: forge::Domain,
    forge_env: Env<Config>,
    client_out: Queue<Request>,
    forge_out: Queue<forge::Request>,
    to_forge: Queue<forge::Event>,
    to_client: Queue<Delivery>,
    pending: Map<Token, Pending>,
    durable: Map<Key, Stored>,
    entries: Map<u64, Entry>,
    planned: Map<u64, Entry>,
    readers: Map<Token, ()>,
    read_results: Map<Token, Result<api::Answer, api::Error>>,
    outcomes: List<(u64, Outcome)>,
    news: List<(Resource, api::Answer)>,
    referee: Referee,
    sequence: u64,
    generation: u64,
    seed: u64,
    blocked: Time,
    stats: Stats,
}
impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        let limits = Limits { facts: settings.facts, ..LIMITS };
        let config = config(settings.faults);
        let mut neighbour = forge::Domain::new(&config, settings.seed);
        forge::repository(
            &mut neighbour,
            &config,
            raw::Setup {
                name: Box::from(&b"org/repo"[..]),
                default: Box::from(&b"main"[..]),
                tree: Box::new([]),
                labels: Box::new([]),
                checks: raw::Checks {
                    contexts: Box::new([]),
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
        forge::grant(&mut neighbour, b"org/repo", 1, raw::Permission::Admin);
        forge::grant(&mut neighbour, b"org/repo", 2, raw::Permission::Write);
        let mut world = World {
            client: configured(&limits, settings.seed),
            env: Env { now: Time::from_nanos(100_000_000_000), wall: Wall::from_nanos(100_000_000_000), limits },
            forge: neighbour,
            forge_env: Env { now: Time::from_nanos(100_000_000_000), wall: Wall::EPOCH, limits: config },
            client_out: Queue::with_capacity(client::max_out(&limits)),
            forge_out: Queue::with_capacity(forge::MAX_OUT),
            to_forge: Queue::with_capacity(16),
            to_client: Queue::with_capacity(16),
            pending: Map::with_capacity(16),
            durable: Map::with_capacity(16),
            entries: Map::with_capacity(limits.entries),
            planned: Map::with_capacity(64),
            readers: Map::with_capacity(16),
            read_results: Map::with_capacity(16),
            outcomes: List::with_capacity(128),
            news: List::with_capacity(128),
            referee: Referee::new(),
            sequence: 0,
            generation: 1,
            seed: settings.seed,
            blocked: Time::ZERO,
            stats: Stats {
                digest: 0,
                calls: 0,
                reads: 0,
                writes: 0,
                terminals: 0,
                restarts: 0,
                outcomes: 0,
                read_terminals: 0,
                creations: 0,
                unavailable: 0,
                timeouts: 0,
                late_landings: 0,
                limited: 0,
            },
        };
        // Historical items are created independently, before keeping starts.
        for number in 0..settings.history {
            let read = raw::Read::Items {
                state: None,
                kind: None,
                labels: Box::new([]),
                author: None,
                since: Time::ZERO,
                page: 1,
                limit: 1,
            };
            assert!(world.forge.inspect(&config, b"org/repo", &read).is_ok());
            let op = raw::Op::Write(raw::Write::CreateIssue {
                title: Box::from(&b"old"[..]),
                body: Box::new([]),
                labels: Box::new([]),
            });
            world.external(op, 10_000 + u64::from(number), 2);
        }
        world.event(Event::Restored { clock: RecoveryClock::Monotonic });
        world
    }
    fn event(&mut self, event: Event) {
        client::step(&mut self.client, &self.env, event, &mut self.client_out);
        self.take_client();
        self.client.reclaim();
    }
    pub fn keep(&mut self, watches: Box<[Watch]>) {
        self.event(Event::Keep { owner: Token::new(1), watches });
    }
    pub fn make(&mut self, entry: Entry) {
        assert!(self.planned.insert(entry.number, entry.clone()).expect("world plan limit").is_none());
        assert!(self.entries.insert(entry.number, entry.clone()).expect("top outbox limit").is_none());
        self.event(Event::Make { entry });
    }
    pub fn read(&mut self, number: u64, read: api::Read) {
        assert!(
            self.readers.insert(Token::new(number), ()).expect("world reader capacity").is_none(),
            "one live fresh read per owner"
        );
        self.event(Event::Read { owner: Token::new(number), repository: REPO, read });
    }
    pub fn restart(&mut self) {
        // Volatile budget knowledge belongs to the old live generation.
        // The independent forge keeps its window and can answer 429 again.
        self.blocked = Time::ZERO;
        self.generation = self.generation.checked_add(1).expect("world generation bound");
        self.stats.restarts = self.stats.restarts.saturating_add(1);
        self.client = configured(&self.env.limits, self.seed);
        // Decision reads belong to the former process, and are reissued by
        // the parent after cold recovery, rather than restored as effects.
        self.readers = Map::with_capacity(16);
        self.read_results = Map::with_capacity(16);
        let mut records = List::with_capacity(self.durable.len());
        for (_, record) in &self.durable {
            records.push(record.clone()).expect("world store limit");
        }
        for record in records.into_boxed() {
            self.event(Event::Restore { record });
        }
        self.event(Event::Restored { clock: RecoveryClock::Monotonic });
        let mut entries = List::with_capacity(self.entries.len());
        for (_, entry) in &self.entries {
            entries.push(entry.clone()).expect("top outbox limit");
        }
        for entry in entries.into_boxed() {
            self.event(Event::Make { entry });
        }
    }
    pub fn calm(&mut self) {
        self.forge_env.limits = config(false);
    }
    /// Answer writes as timed out, then apply their delayed copies at `after`.
    pub fn land_writes_late(&mut self, after: Duration) {
        self.forge_env.limits.landing = 1000;
        self.forge_env.limits.land_min = after;
        self.forge_env.limits.land_max = after;
    }
    #[must_use]
    pub fn branch(&self, branch: &[u8]) -> Option<u64> {
        self.forge.branch(b"org/repo", branch)
    }
    #[must_use]
    pub fn entry(&self, number: u64) -> Option<&Entry> {
        self.entries.get(&number)
    }
    #[must_use]
    pub fn stats(&self) -> Stats {
        let tally = self.forge.tally();
        Stats {
            creations: self.referee.creations,
            unavailable: tally.unavailable,
            timeouts: tally.timeouts,
            late_landings: tally.landed,
            limited: tally.limited,
            ..self.stats
        }
    }
    #[must_use]
    pub fn read_result(&self, number: u64) -> Option<&Result<api::Answer, api::Error>> {
        self.read_results.get(&Token::new(number))
    }
    #[must_use]
    pub fn outcomes(&self) -> &[(u64, Outcome)] {
        self.outcomes.as_slice()
    }
    #[must_use]
    pub fn news(&self) -> &[(Resource, api::Answer)] {
        self.news.as_slice()
    }
    pub fn run_for(&mut self, seconds: u64) {
        let end = self.env.now.saturating_add(Duration::from_secs(seconds));
        for _ in 0..20_000 {
            if !self.iterate(end) {
                return;
            }
        }
        panic!("world did not reach its horizon, seed {}", self.seed);
    }
    pub fn finish(&mut self) {
        self.calm();
        self.keep(Box::new([]));
        self.run_for(100);
        assert!(
            self.pending.is_empty() && self.to_forge.is_empty() && self.to_client.is_empty() && self.readers.is_empty()
        );
        assert!(self.durable.is_empty() && self.entries.is_empty(), "outbox and working set settled");
        assert!(!self.client.is_ready());
        assert_eq!(self.forge.observations_lost(), 0);
    }
    fn iterate(&mut self, end: Time) -> bool {
        if let Some(delivery) = self.to_client.pop() {
            if delivery.generation == self.generation {
                self.event(delivery.event);
            }
        } else if let Some(event) = self.to_forge.pop() {
            forge::step(&mut self.forge, &self.forge_env, event, &mut self.forge_out);
            self.take_forge();
        } else if self.client.is_ready() {
            client::resume(&mut self.client, &self.env, &mut self.client_out);
            self.take_client();
            self.client.reclaim();
        } else if self.forge.is_due(self.env.now) {
            forge::fire(&mut self.forge, &self.forge_env, &mut self.forge_out);
            self.take_forge();
        } else if self.client.is_due(self.env.now) {
            client::fire(&mut self.client, &self.env, &mut self.client_out);
            self.take_client();
            self.client.reclaim();
        } else {
            let mut next = end;
            if let Some(at) = self.client.next_deadline() {
                next = next.min(at);
            }
            if let Some(at) = self.forge.next_deadline() {
                next = next.min(at);
            }
            if self.env.now >= end {
                return false;
            }
            self.env.now = next;
            self.env.wall = Wall::from_nanos(next.as_nanos());
            self.forge_env.now = next;
        }
        for _ in 0..128 {
            let Some(observation) = self.forge.pop_observation() else {
                break;
            };
            self.referee.observe(&observation);
        }
        self.forge.reclaim();
        true
    }
    fn take_client(&mut self) {
        let mut released = List::with_capacity(self.client_out.len());
        for _ in 0..self.client_out.len() {
            let request = self.client_out.pop().expect("output length measured");
            let text = format!("{request:?}");
            self.stats.digest = self.stats.digest.rotate_left(7) ^ translate::digest(text.as_bytes());
            match request {
                Request::Save { record } => {
                    let key = match &record {
                        Stored::Live(record) => Key::Live(record.watch.resource.clone()),
                        Stored::Repository(record) => Key::Repository(record.repository),
                    };
                    self.durable.insert(key, record).expect("world durable limit");
                }
                Request::Progress { entry } => {
                    self.entries.insert(entry.number, entry).expect("top outbox limit");
                }
                Request::Erase { key } => {
                    self.durable.remove(&key).expect("a durable record erased");
                }
                Request::Call { .. } => {
                    released.push(request).expect("bounded decision output");
                }
                Request::Outcome { entry, outcome, .. } => {
                    match outcome {
                        Outcome::Uncertain => {}
                        Outcome::Made { .. }
                        | Outcome::Failed(_)
                        | Outcome::Raced { .. }
                        | Outcome::Held
                        | Outcome::Withdrawn => {
                            self.entries.remove(&entry).expect("top settles stored entry");
                        }
                    }
                    self.stats.outcomes = self.stats.outcomes.saturating_add(1);
                    self.outcomes.push((entry, outcome)).expect("world outcome limit");
                }
                Request::Changed { resource, result } => {
                    if let Ok(answer) = result {
                        self.referee.news(&answer);
                        self.news.push((resource, answer)).expect("world news limit");
                    }
                }
                Request::Kept { result, .. } => assert_eq!(result, Ok(())),
                Request::Read { owner, result } => {
                    self.readers.remove(&owner).expect("one terminal for every live decision read");
                    self.stats.read_terminals = self.stats.read_terminals.saturating_add(1);
                    assert!(
                        self.read_results.insert(owner, result).expect("world fresh result capacity").is_none(),
                        "one fresh result per owner"
                    );
                }
                Request::Drift { .. } | Request::ReadAfreshDone | Request::OutboxDone => {}
            }
        }
        // The entire decision is committed before any Call is released.
        for request in released.into_boxed() {
            match request {
                Request::Call { call, repository, op } => self.submit(call, repository, op),
                Request::Save { .. }
                | Request::Progress { .. }
                | Request::Erase { .. }
                | Request::Outcome { .. }
                | Request::Changed { .. }
                | Request::Kept { .. }
                | Request::Drift { .. }
                | Request::Read { .. }
                | Request::ReadAfreshDone
                | Request::OutboxDone => unreachable!("only calls withheld"),
            }
        }
    }
    fn submit(&mut self, call: Token, repository: api::Repository, op: api::Op) {
        assert_eq!(repository, REPO);
        assert!(self.env.now >= self.blocked, "no class submits during a rate reset");
        match &op {
            api::Op::Write(write) => {
                Referee::write(&self.planned, &self.entries, write);
                self.stats.writes = self.stats.writes.saturating_add(1);
            }
            api::Op::Read(_) => self.stats.reads = self.stats.reads.saturating_add(1),
        }
        self.stats.calls = self.stats.calls.saturating_add(1);
        self.sequence = self.sequence.checked_add(1).expect("world call limit");
        let token = Token::new(self.sequence);
        let raw = translate::op(&op, &self.env.limits);
        self.pending
            .insert(token, Pending { client: call, op, generation: self.generation })
            .expect("world in-flight limit");
        self.to_forge.push(forge::Event::Call {
            reply_to: ReplyTo::new(token),
            user: 1,
            repository: Box::from(&b"org/repo"[..]),
            op: raw,
        });
    }
    fn take_forge(&mut self) {
        for _ in 0..self.forge_out.len() {
            match self.forge_out.pop().expect("fake output exists") {
                forge::Request::Reply { to, result } => {
                    let pending = self.pending.remove(&to.into_token()).expect("one terminal per submitted fake call");
                    let result = match result {
                        Ok(answer) => Ok(translate::answer(&pending.op, answer, &self.env.limits)),
                        Err(error) => {
                            Err(translate::error(error, forge::time(&self.forge_env.limits, self.forge_env.now)))
                        }
                    };
                    if pending.generation == self.generation
                        && let Err(api::Error::RateLimited { after }) = result
                    {
                        self.blocked = self.blocked.max(self.env.now.saturating_add(after));
                    }
                    self.stats.terminals = self.stats.terminals.saturating_add(1);
                    self.to_client.push(Delivery {
                        generation: pending.generation,
                        event: Event::Answered { call: pending.client, cost: 1, result },
                    });
                }
                forge::Request::Hook { .. } => {}
            }
        }
    }
    /// Independent pre-adoption fixture. It writes through the fake API,
    /// bypassing all client decisions and keeping its authenticated author.
    pub fn historical_issue(&mut self) -> u64 {
        let raw::Answer::Created(number) = self.external(
            raw::Op::Write(raw::Write::CreateIssue {
                title: Box::from(&b"adopt"[..]),
                body: Box::new([]),
                labels: Box::new([]),
            }),
            20_000,
            2,
        ) else {
            panic!("fixture issue creation");
        };
        number
    }
    pub fn historical_comment(&mut self, number: u64, author: u64, key: Option<Box<[u8]>>) -> u64 {
        let body = match key {
            Some(key) => translate::keyed(&key, b"old"),
            None => Box::from(&b"human"[..]),
        };
        let raw::Answer::Commented(id) =
            self.external(raw::Op::Write(raw::Write::Comment { number, body }), 20_001, author)
        else {
            panic!("fixture comment creation");
        };
        id
    }
    pub fn historical_pull(&mut self) -> u64 {
        self.external(raw::Op::Git(raw::Git::Create { branch: Box::from(&b"topic"[..]), commit: 1 }), 21_000, 2);
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: self.forge_env.limits };
        forge::advance(&mut self.forge, &env, b"org/repo", b"topic", b"file", b"topic", 2).expect("fixture branch");
        let raw::Answer::Created(number) = self.external(
            raw::Op::Write(raw::Write::OpenPull {
                title: Box::from(&b"pull"[..]),
                body: Box::new([]),
                head: Box::from(&b"topic"[..]),
                base: Box::from(&b"main"[..]),
            }),
            21_001,
            2,
        ) else {
            panic!("fixture pull creation");
        };
        number
    }
    /// Another writer takes a branch name before a delayed client creation lands.
    pub fn outside_branch(&mut self, branch: &[u8], commit: u64) {
        let now = self.env.now;
        assert_eq!(
            self.external_at(
                raw::Op::Write(raw::Write::CreateBranch { branch: Box::from(branch), commit }),
                21_008,
                2,
                now,
            ),
            raw::Answer::Branch(raw::Created::Created)
        );
    }
    pub fn historical_review(&mut self, number: u64, pending: bool) -> u64 {
        let verdict = if pending { None } else { Some(raw::Verdict::Comment) };
        let raw::Answer::Reviewed(id) = self.external(
            raw::Op::Write(raw::Write::Review { number, verdict, body: Box::from(&b"human review"[..]) }),
            21_002,
            2,
        ) else {
            panic!("fixture review creation");
        };
        id
    }
    pub fn recent_review(&mut self, number: u64, pending: bool) -> u64 {
        let verdict = if pending { None } else { Some(raw::Verdict::Comment) };
        let raw::Answer::Reviewed(id) = self.external_at(
            raw::Op::Write(raw::Write::Review { number, verdict, body: Box::from(&b"recent review"[..]) }),
            21_006,
            2,
            self.env.now,
        ) else {
            panic!("fixture recent review");
        };
        id
    }
    pub fn missed_review_submission(&mut self, number: u64, review: u64) {
        self.external_at(
            raw::Op::Write(raw::Write::Submit { number, review, verdict: raw::Verdict::Comment }),
            21_007,
            2,
            self.env.now,
        );
    }
    pub fn submit_review(&mut self, number: u64, review: u64) {
        self.external_at(
            raw::Op::Write(raw::Write::Submit { number, review, verdict: raw::Verdict::Comment }),
            21_003,
            2,
            self.env.now,
        );
        self.event(Event::Hint { hint: api::Hint { repository: REPO, change: api::Change::Item(number), key: None } });
    }
    pub fn missed_edit_comment(&mut self, id: u64, body: Box<[u8]>) {
        self.external_at(raw::Op::Write(raw::Write::EditComment { id, body }), 21_005, 2, self.env.now);
    }
    pub fn edit_comment(&mut self, number: u64, id: u64, body: Box<[u8]>) {
        self.external_at(raw::Op::Write(raw::Write::EditComment { id, body }), 21_004, 2, self.env.now);
        self.event(Event::Hint { hint: api::Hint { repository: REPO, change: api::Change::Item(number), key: None } });
    }
    pub fn run_until_inbox(&mut self, rows: u32) {
        let end = self.env.now.saturating_add(Duration::from_secs(5));
        for _ in 0..20_000 {
            let mut received = 0_u32;
            for (_, answer) in self.news.as_slice() {
                received = received.saturating_add(inbox_count(answer));
            }
            if received >= rows {
                return;
            }
            assert!(self.iterate(end), "fixture inbox rows reached before horizon");
        }
        panic!("world inbox step budget");
    }
    fn external(&mut self, op: raw::Op, call: u64, user: u64) -> raw::Answer {
        self.external_at(op, call, user, Time::ZERO)
    }
    fn external_at(&mut self, op: raw::Op, call: u64, user: u64, now: Time) -> raw::Answer {
        let mut config = self.forge_env.limits;
        config.latency_min = Duration::ZERO;
        config.latency_max = Duration::ZERO;
        config.unavailable = 0;
        config.timeouts = 0;
        config.landing = 0;
        config.late = 0;
        config.rate_limit = 0;
        let env = Env { now, wall: Wall::EPOCH, limits: config };
        forge::step(
            &mut self.forge,
            &env,
            forge::Event::Call {
                reply_to: ReplyTo::new(Token::new(call)),
                user,
                repository: Box::from(&b"org/repo"[..]),
                op,
            },
            &mut self.forge_out,
        );
        for _ in 0..4 {
            if !self.forge.is_due(env.now) {
                break;
            }
            forge::fire(&mut self.forge, &env, &mut self.forge_out);
        }
        let mut answered = None;
        for _ in 0..self.forge_out.len() {
            match self.forge_out.pop().expect("external reply") {
                forge::Request::Reply { result, .. } => {
                    assert!(answered.is_none(), "fixture API has one terminal");
                    answered = Some(result.expect("external fixture succeeds"));
                }
                forge::Request::Hook { .. } => {}
            }
        }
        self.forge.reclaim();
        for _ in 0..128 {
            if self.forge.pop_observation().is_none() {
                break;
            }
        }
        answered.expect("fixture terminal")
    }
}
fn config(faults: bool) -> Config {
    Config {
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
        late: if faults { 100 } else { 0 },
        late_min: Duration::from_secs(1),
        late_max: Duration::from_secs(3),
        unavailable: if faults { 100 } else { 0 },
        timeouts: if faults { 150 } else { 0 },
        landing: if faults { 150 } else { 0 },
        land_min: Duration::from_secs(1),
        land_max: Duration::from_secs(3),
        rate_limit: if faults { 8 } else { 0 },
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

fn configured(limits: &client::Limits, seed: u64) -> client::Domain {
    client::Domain::configured(
        limits,
        seed,
        client::Config {
            namespace: Box::from(&b"world"[..]),
            writers: Box::new([client::Writer { forge: REPO.forge, author: 1 }]),
        },
    )
    .expect("world authenticated writer")
}

fn inbox_count(answer: &api::Answer) -> u32 {
    match answer {
        api::Answer::Item { comments, .. } => u32::try_from(comments.len()).expect("bounded inbox rows"),
        api::Answer::Reviews { reviews, .. } => u32::try_from(reviews.len()).expect("bounded inbox rows"),
        api::Answer::Items { .. }
        | api::Answer::Pull(_)
        | api::Answer::Statuses { .. }
        | api::Answer::Remarks { .. }
        | api::Answer::Commit(_)
        | api::Answer::Branches(_)
        | api::Answer::PullFiles { .. }
        | api::Answer::Compare { .. }
        | api::Answer::Checks(_)
        | api::Answer::Job { .. }
        | api::Answer::Protection(_)
        | api::Answer::Settings(_)
        | api::Answer::Collaborators { .. }
        | api::Answer::Permission(_)
        | api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Branch(_)
        | api::Answer::Done => 0,
    }
}
