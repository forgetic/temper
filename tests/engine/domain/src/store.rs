//! Temper's worlds over jig's real store and the service's store routing.
use jig_protocol_store::{self as protocol, Below, Event, Io, Limits, Request, Stores};
use jig_test_store_files::Files;
use skein_lib::{Env, Queue, Time, Token, Wall};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use temper_engine_domain::{self as engine, Deployment, JournalLimits, Key, Range, Record, Write};
use temper_service::store::route::{self, Up};

pub const LIMITS: JournalLimits = JournalLimits {
    commits: 3,
    held: 12,
    writes: 4,
    deliveries: 4,
    transcript_bytes: 128,
    result_bytes: 128,
    run_bytes: 1,
};
pub const HEADER: Deployment = Deployment {
    projects: 1,
    id: [31; 16],
    tasks: 0,
    people: 0,
    sign_ins: 0,
    messages: 0,
    runs: 0,
    calls: 0,
    connector_rows: 0,
    applications: 0,
    commits: 0,
};

/// The installation's identifier these worlds draw: an empty store keeps it,
/// and a reopened one answers the one it kept (jig's configuration.md, 8).
const INSTALLATION: [u8; 16] = [0; 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Hold { commits: u32 },
    Fail { commit: u64 },
    SlowPages { by: u32 },
}
struct Delayed {
    event: Event,
    delay: u32,
}
/// Numbered root transactions and their durable typed mirror.
pub struct Store {
    pub applied: u64,
    pub rows: BTreeMap<Key, Record>,
    pub pending: VecDeque<(u64, Box<[Write]>)>,
    pub held: VecDeque<u64>,
    delays: BTreeMap<u64, u32>,
    component: Option<protocol::Store>,
    files: Files,
    routes: route::Routes,
    limits: engine::Limits,
    component_limits: Limits,
    models: Box<[engine::Model]>,
    inflight: BTreeMap<u64, Box<[Write]>>,
    durable: Vec<(u64, Box<[Write]>)>,
    replies: Vec<engine::StoreInput>,
    delayed: VecDeque<Delayed>,
    hold_next: u32,
    holding: bool,
    fail_at: Option<u64>,
    page_delay: u32,
    failed: bool,
}
impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("applied", &self.applied)
            .field("rows", &self.rows)
            .field("pending", &self.pending)
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
}
impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}
impl jig_conformance::Durable for Store {
    type Write = engine::Write;
    fn submit(&mut self, number: u64, writes: Box<[Self::Write]>, delay: u32) {
        Self::submit(self, number, writes, delay);
    }
    fn tick(&mut self, hold: bool) -> Result<Option<u64>, u64> {
        Self::tick(self, hold)
    }
    fn release_held(&mut self) -> Option<u64> {
        Self::release_held(self)
    }
    fn crash(&mut self, seed: u64) {
        Self::crash(self, seed);
    }
    fn fork(&self) -> Self {
        Self::fork(self)
    }
    fn applied(&self) -> u64 {
        self.applied
    }
    fn idle(&self) -> bool {
        Self::idle(self)
    }
    fn holding(&self) -> bool {
        !self.held.is_empty()
    }
}
impl Store {
    #[must_use]
    pub fn new() -> Store {
        Self::with_limits(&widest())
    }
    #[must_use]
    pub fn with_transcript_bytes(bytes: u32) -> Store {
        let mut limits = widest();
        limits.journal.transcript_bytes = bytes;
        limits.loads.reply_bytes = bytes;
        limits.loads.transcript_bytes = bytes;
        Self::with_limits(&limits)
    }
    #[must_use]
    pub fn with_limits(limits: &engine::Limits) -> Store {
        let component_limits = component_limits(limits);
        let stores = Stores {
            store: skein_kv::Store::memory(&component_limits.store),
            secrets: skein_kv::Store::memory(&component_limits.secrets),
        };
        // The worlds' configured models, in their order (jig's store over a configuration's).
        let models = crate::walking::models();
        let component = protocol::Store::over(
            &component_limits,
            temper_protocol_store::application(&limits.forge).expect("checked family sizes"),
            &models,
            stores,
        );
        let mut store = Store {
            applied: 0,
            rows: BTreeMap::new(),
            pending: VecDeque::new(),
            held: VecDeque::new(),
            delays: BTreeMap::new(),
            component: Some(component),
            files: Files::new(),
            routes: routes(limits),
            limits: *limits,
            component_limits,
            models,
            inflight: BTreeMap::new(),
            durable: Vec::new(),
            replies: Vec::new(),
            delayed: VecDeque::new(),
            hold_next: 0,
            holding: false,
            fail_at: None,
            page_delay: 0,
            failed: false,
        };
        store.open();
        store
    }
    pub fn fault(&mut self, fault: Fault) {
        match fault {
            Fault::Hold { commits } => self.hold_next = commits,
            Fault::Fail { commit } => self.fail_at = Some(commit),
            Fault::SlowPages { by } => self.page_delay = by,
        }
    }
    pub fn submit(&mut self, number: u64, writes: Box<[Write]>, delay: u32) {
        assert!(!self.failed, "a failed world restarts before writing");
        let previous = self.pending.back().map_or(self.applied, |pending| pending.0);
        assert_eq!(number, previous.checked_add(1).expect("bounded commit"));
        self.pending.push_back((number, writes));
        self.delays.insert(number, delay);
    }
    pub fn apply(&mut self) -> u64 {
        let number = self.pending.front().expect("queued transaction").0;
        assert_eq!(self.tick(false), Ok(Some(number)), "one ordered transaction applied");
        assert_eq!(self.header().commits, number, "header and records applied atomically");
        number
    }
    /// # Errors
    /// The failed commit's number.
    pub fn tick(&mut self, hold: bool) -> Result<Option<u64>, u64> {
        self.tick_pages();
        if self.failed {
            return Ok(None);
        }
        let Some((number, _)) = self.pending.front() else {
            return Ok(None);
        };
        if let Some(delay) = self.delays.get_mut(number)
            && *delay != 0
        {
            *delay -= 1;
            return Ok(None);
        }
        let (number, writes) = self.pending.pop_front().expect("checked front");
        self.delays.remove(&number);
        if self.fail_at == Some(number) {
            self.failed = true;
            self.pending.clear();
            return Err(number);
        }
        self.holding = hold || self.hold_next != 0 || !self.held.is_empty();
        self.hold_next = self.hold_next.saturating_sub(1);
        self.inflight.insert(number, writes.clone());
        self.request(engine::StoreRequest::Commit { number, writes });
        if self.failed {
            return Err(number);
        }
        assert_eq!(self.applied, number, "synchronous memory store settles the commit");
        if self.holding { Ok(None) } else { Ok(Some(number)) }
    }
    pub fn release_held(&mut self) -> Option<u64> {
        if self.failed { None } else { self.held.pop_front() }
    }
    pub fn crash(&mut self, seed: u64) {
        let stores = self.component.take().expect("component exists").into_stores();
        self.files.crash(seed);
        self.component = Some(protocol::Store::over(
            &self.component_limits,
            temper_protocol_store::application(&self.limits.forge).expect("checked family sizes"),
            &self.models,
            stores,
        ));
        self.routes = routes(&self.limits);
        self.pending.clear();
        self.delays.clear();
        self.held.clear();
        self.inflight.clear();
        self.replies.clear();
        self.delayed.clear();
        self.holding = false;
        self.failed = false;
        self.hold_next = 0;
        self.fail_at = None;
        self.open();
    }
    #[must_use]
    pub fn fork(&self) -> Store {
        let mut fresh = Store::with_limits(&self.limits);
        for (number, writes) in &self.durable {
            fresh.submit(*number, writes.clone(), 0);
            assert_eq!(fresh.tick(false), Ok(Some(*number)));
        }
        fresh.page_delay = self.page_delay;
        fresh
    }
    /// Fixture changes reach the component in one transaction with its header.
    pub fn seed(&mut self, saves: Box<[Record]>, erases: Box<[Key]>) {
        assert!(self.pending.is_empty(), "seed between root commits");
        let number = self.applied.checked_add(1).expect("bounded fixture commit");
        let header_key = Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Deployment));
        let mut header = if self.rows.contains_key(&header_key) { self.header() } else { HEADER };
        if let Some(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(saved)))) =
            saves.iter().find(|row| row.key() == header_key)
        {
            header = *saved;
        }
        header.commits = number;
        let mut writes: Vec<_> =
            saves.into_vec().into_iter().filter(|row| row.key() != header_key).map(Write::Save).collect();
        writes.extend(erases.into_vec().into_iter().map(Write::Erase));
        writes.push(Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(
            header,
        )))));
        self.submit(number, writes.into_boxed_slice(), 0);
        assert_eq!(self.tick(false), Ok(Some(number)), "seed reaches the real component");
    }
    #[must_use]
    pub fn header(&self) -> Deployment {
        let Some(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(header)))) =
            self.rows.get(&Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Deployment)))
        else {
            panic!("durable deployment");
        };
        *header
    }
    #[must_use]
    pub fn idle(&self) -> bool {
        self.pending.is_empty() && self.delayed.is_empty() && self.routes.idle()
    }
    /// Read one root page through the component and the service's decoding.
    #[must_use]
    pub fn page(&mut self, range: &Range, after: Option<&Key>, most: u32) -> (Box<[Record]>, Option<Key>) {
        self.request(engine::StoreRequest::Load {
            owner: Token::new(1),
            range: range.clone(),
            after: after.cloned(),
            most,
            bytes: self.limits.loads.reply_bytes,
        });
        loop {
            if let Some(reply) = self.replies.pop() {
                match reply {
                    engine::StoreInput::Loaded { rows, next, .. } => return (rows, next),
                    engine::StoreInput::Committed { .. } => continue,
                    reply @ (engine::StoreInput::Uncommitted { .. }
                    | engine::StoreInput::Unloaded { .. }
                    | engine::StoreInput::ReadLoaded { .. }) => {
                        panic!("load {range:?} after {after:?} did not return its page: {reply:?}");
                    }
                }
            }
            assert!(!self.failed, "store load failed");
            assert!(!self.delayed.is_empty(), "load has a terminal");
            self.tick_pages();
        }
    }
    /// Read a historical page through the real store and the service's routing.
    #[must_use]
    pub fn read(&mut self, range: Box<jig_domain_core::reads::Range>) -> jig_domain_core::reads::Loaded {
        self.request(engine::StoreRequest::Read { load: Token::new(1), range });
        loop {
            if let Some(reply) = self.replies.pop() {
                match reply {
                    engine::StoreInput::ReadLoaded { result, .. } => return result,
                    engine::StoreInput::Committed { .. } => continue,
                    engine::StoreInput::Uncommitted { .. }
                    | engine::StoreInput::Unloaded { .. }
                    | engine::StoreInput::Loaded { .. } => {
                        panic!("historical read returned another terminal: {reply:?}")
                    }
                }
            }
            assert!(!self.failed && !self.delayed.is_empty(), "historical read has a terminal");
            self.tick_pages();
        }
    }
    fn request(&mut self, request: engine::StoreRequest) {
        let request = route::down(&mut self.routes, request).expect("root store request admitted");
        self.drive(VecDeque::from([request]), Queue::with_capacity(protocol::MAX_OUT_DOWN));
    }
    fn open(&mut self) {
        let root = self.files.root();
        self.drive(
            VecDeque::from([Request::Open { owner: Token::new(0), root, installation: INSTALLATION }]),
            Queue::with_capacity(protocol::MAX_OUT_DOWN),
        );
        assert!(!self.failed, "durable stores reopen");
    }
    fn env(&self) -> Env<Limits> {
        Env { now: Time::ZERO, wall: Wall::EPOCH, limits: self.component_limits }
    }
    fn drive(&mut self, mut requests: VecDeque<Request>, mut above: Queue<Event>) {
        let mut below = Below {
            store: Queue::with_capacity(skein_kv::MAX_OUT.below),
            secrets: Queue::with_capacity(skein_kv::MAX_OUT.below),
            transcripts: Queue::with_capacity(skein_kv::MAX_OUT.below),
        };
        loop {
            if let Some(event) = above.pop() {
                self.event(event, &mut requests);
                continue;
            }
            if let Some(request) = requests.pop_front() {
                let env = self.env();
                protocol::down(
                    self.component.as_mut().expect("component exists"),
                    &env,
                    request,
                    &mut above,
                    &mut below,
                );
                continue;
            }
            let io = if let Some(request) = below.transcripts.pop() {
                Io::Transcripts(self.files.answer(request))
            } else if let Some(request) = below.store.pop() {
                Io::Store(self.files.answer(request))
            } else if let Some(request) = below.secrets.pop() {
                Io::Secrets(self.files.answer(request))
            } else {
                break;
            };
            let env = self.env();
            protocol::up(self.component.as_mut().expect("component exists"), &env, io, &mut above, &mut below);
        }
    }
    fn event(&mut self, event: Event, requests: &mut VecDeque<Request>) {
        match event {
            Event::Opened { last, .. } => self.applied = last,
            Event::Patched { .. } => {}
            Event::Committed { number, .. } => {
                let writes = self.inflight.remove(&number).expect("root commit");
                for write in &writes {
                    match write {
                        Write::CopySettingsRevision(number) => {
                            let held = Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Settings(
                                jig_domain_core::settings::Key::Held(*number),
                            )));
                            let Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Settings(
                                jig_domain_core::settings::Record::Revision { revision, .. },
                            ))) = self.rows.get(&held).expect("held revision precedes its copy")
                            else {
                                panic!("held revision shape")
                            };
                            let row = Record::Core(jig_domain_core::Record::Core(
                                jig_domain_core::CoreRecord::Settings(jig_domain_core::settings::Record::Revision {
                                    history: true,
                                    revision: revision.clone(),
                                }),
                            ));
                            self.rows.insert(row.key(), row);
                        }
                        Write::Save(row) => {
                            self.rows.insert(row.key(), row.clone());
                        }
                        Write::Erase(key) => {
                            self.rows.remove(key);
                        }
                    }
                }
                self.applied = number;
                self.durable.push((number, writes));
                if self.holding {
                    self.held.push_back(number);
                }
            }
            Event::Failed { .. } | Event::OpenRefused { .. } => self.failed = true,
            event @ (Event::Restored { .. } | Event::Loaded { .. } | Event::Turns { .. } | Event::Got { .. })
                if self.page_delay != 0 =>
            {
                self.delayed.push_back(Delayed { event, delay: self.page_delay });
            }
            event @ Event::Corrupt { .. } => {
                self.failed = true;
                self.route(event, requests);
            }
            event @ (Event::Site { .. }
            | Event::Restored { .. }
            | Event::Loaded { .. }
            | Event::Read { .. }
            | Event::Got { .. }
            | Event::Turns { .. }
            | Event::Unread { .. }
            | Event::Kept { .. }
            | Event::Carried { .. }
            | Event::Stale { .. }
            | Event::Erased { .. }
            | Event::Secret { .. }
            | Event::SecretsLoaded { .. }) => self.route(event, requests),
        }
    }
    fn route(&mut self, event: Event, requests: &mut VecDeque<Request>) {
        let mut outputs = Queue::with_capacity(8);
        route::up(&mut self.routes, event, &mut outputs);
        while let Some(output) = outputs.pop() {
            match output {
                Up::Ask(request) => requests.push_back(request),
                Up::Root(event) => {
                    let engine::Event::Store(input) = *event else {
                        panic!("store terminal");
                    };
                    self.replies.push(input);
                }
            }
        }
    }
    fn tick_pages(&mut self) {
        let mut requests = VecDeque::new();
        for _ in 0..self.delayed.len() {
            let mut delayed = self.delayed.pop_front().expect("counted pages");
            delayed.delay = delayed.delay.saturating_sub(1);
            if delayed.delay == 0 {
                self.route(delayed.event, &mut requests);
            } else {
                self.delayed.push_back(delayed);
            }
        }
        self.drive(requests, Queue::with_capacity(protocol::MAX_OUT_DOWN));
    }
}
fn routes(limits: &engine::Limits) -> route::Routes {
    route::Routes::new(limits.loads.loads, limits, temper_protocol_store::bounds(&limits.forge).expect("family bounds"))
}
/// The largest journal, task and brief bounds used by the engine stories.
#[must_use]
pub fn widest() -> engine::Limits {
    let mut limits = crate::walking::limits();
    limits.tasks.tasks = 12;
    limits.tasks.project_tasks = 12;
    limits.tasks.delegates = 4;
    limits.tasks.tree_tasks = 12;
    limits.tasks.depth = 3;
    limits.tasks.parameters = 8;
    limits.tasks.spec_bytes = 512;
    limits.tasks.executor_kinds = 2;
    limits.tasks.authority_grants = 10;
    limits.tasks.authority_segments = 9;
    limits.tasks.authority_bytes = 512;
    limits.tasks.contract_choices = 2;
    limits.tasks.inbox_messages = 8;
    limits.tasks.inbox_bytes = 1024;
    limits.journal.run_bytes = 8192;
    limits.journal.writes = 8192;
    limits.journal.deliveries = 256;
    limits.journal.held = 1024;
    limits.journal.result_bytes = 4096;
    limits.journal.transcript_bytes = 32_768;
    limits.brief.read_bytes = 512;
    limits.brief.budgets.pull = 384;
    limits.brief.budgets.ci = 384;
    limits.brief.brief_bytes = 768;
    limits.loads.reply_bytes = 32_768;
    limits.loads.transcript_bytes = 32_768;
    limits.authority.grants = 10;
    // Settings patterns use jig's eight-segment store ceiling; task scopes retain their own nine.
    limits.authority.segments = 8;
    limits.authority.segment_bytes = 64;
    limits.authority.executors = 2;
    limits.authority.batch = 2;
    limits.authority.writes = 2;
    limits.people.inbox_tasks = 24;
    limits.people.people = 4;
    limits.people.holdings = 4;
    limits.people.sign_ins = 8;
    limits.people.requests = 8;
    limits.people.pending = 4;
    limits.people.amendment_bytes = 2048;
    limits.people.initial_owners = 2;
    limits.authority.roles = 4;
    limits.authority.requirements = 8;
    limits.authority.batch = 3;
    limits.tasks.batch = 3;
    limits.tasks.dependencies = 2;
    limits.fleet.slots = 5;
    limits.fleet.workstreams = 2;
    limits.fleet.attempts = 8;
    limits.fleet.calls = 4;
    limits.fleet.turns = 64;
    limits.call_records = 4;
    limits.brief.briefs = 5;
    limits.brief.sections = 8;
    limits.brief.brief_bytes = 1024;
    limits.forge.brief_sections = 80;
    limits.people.inbox_recipients = 32;
    limits
}
fn component_limits(limits: &engine::Limits) -> Limits {
    let profile = jig_protocol_config::profile(jig_protocol_config::Profile::Small);
    let core = core_limits(limits);
    let mut profile = profile.store;
    profile.store.budget = 4 * 1024 * 1024;
    profile.store.key = u32::MAX;
    profile.store.value = u32::MAX;
    profile.store.ops = u32::MAX;
    profile.store.commit = u32::MAX;
    profile.store.queued_bytes = u64::MAX;
    profile.store.page.bytes = u32::MAX;
    profile.store.page.rows = 8;
    temper_service::store::limits(limits, &core, &limits.forge, &profile).expect("world sizes fit real store")
}

pub(crate) fn core_limits(limits: &engine::Limits) -> jig_domain_core::Limits {
    let mut core = jig_protocol_config::profile(jig_protocol_config::Profile::Small).core;
    core.tasks = limits.tasks;
    core.settings = limits.settings;
    core.authority = limits.authority;
    core.people = limits.people;
    core.fleet = limits.fleet;
    core.accounts = limits.accounts;
    core.notes = limits.notes;
    core.load_slots = limits.loads.loads;
    core.call_records = limits.call_records;
    core.resume_bytes = limits.journal.transcript_bytes;
    core.run_bytes = limits.journal.run_bytes;
    core.policy_bytes = core.policy_bytes.min(u64::from(limits.journal.transcript_bytes));
    core.call_answer_bytes = limits.journal.transcript_bytes;
    core
}
