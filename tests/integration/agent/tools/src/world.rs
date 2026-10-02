use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model_tools as tools;
use temper_agent_model_tools::{Authority, Call, Done, Expect, Fault, Grants, Op, Outcome, Refusal, Repo, Var};
use temper_checkout_fake::Checkout;
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};

use crate::translate;

/// Durations drawn uniformly from `min..=max`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub min: Duration,
    pub max: Duration,
}

impl Span {
    #[must_use]
    pub const fn millis(min: u64, max: u64) -> Span {
        Span { min: Duration::from_millis(min), max: Duration::from_millis(max) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub tools: tools::Limits,
    /// How long io takes to run an operation.
    pub io: Span,
    /// The chance, per mille, that an operation fails with a fault.
    pub faults: u32,
    /// The chance, per mille, that a cancel loses its race and the operation
    /// ends as it would have.
    pub late_cancels: u32,
    /// The chance, per mille, that an operation abandoned at its deadline had
    /// taken effect all the same.
    pub late_effects: u32,
    /// How long the session takes between one step of its script and the
    /// next.
    pub think: Span,
    /// How long the session gives each call.
    pub call_timeout: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: no faults, io well within every
    /// deadline, room for a few kits.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            tools: tools::Limits {
                kits: 4,
                calls: 8,
                repos: 4,
                path_bytes: 256,
                known_files: 16,
                file_bytes: 4096,
                read_bytes: 1024,
                list_entries: 16,
                match_lines: 4,
                file_timeout: Duration::from_secs(10),
                env_bytes: 256,
                shell_timeout: Duration::from_secs(60),
                shell_timeout_max: Duration::from_secs(600),
                shell_head: 64,
                shell_tail: 128,
                search_hits: 8,
                search_bytes: 256,
                search_timeout: Duration::from_secs(30),
            },
            io: Span::millis(1, 20),
            faults: 0,
            late_cancels: 0,
            late_effects: 0,
            think: Span::millis(1, 100),
            call_timeout: Duration::from_secs(60),
        }
    }
}

/// What a session does with its kit, one step at a time, before it closes it.
pub enum Step {
    /// Sends the calls together, and waits for every answer.
    Calls(Vec<Call>),
    /// Sends the calls, and goes on without waiting.
    Send(Vec<Call>),
    /// Changes the checkout, as something other than the agent would.
    Change(Box<dyn FnOnce(&mut Checkout)>),
    /// Waits.
    Sleep(Duration),
    /// From now on io takes this long to run an operation, for every kit.
    Latency(Span),
}

/// What the world counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Calls the sessions made.
    pub calls: u32,
    /// Operations the tools asked of io, and the commands among them.
    pub ops: u32,
    pub commands: u32,
    /// Operations that failed with a fault.
    pub faults: u32,
    /// Operations that ran out of time, and those that took effect all the
    /// same.
    pub timeouts: u32,
    pub late_effects: u32,
    /// Operations the tools cancelled, those whose cancel lost the race, and
    /// cancels that came after their operation had ended.
    pub cancels: u32,
    pub late_cancels: u32,
    pub stale_cancels: u32,
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// A session opens its kit.
    Open { session: u64 },
    /// A session takes the next step of its script.
    Next { session: u64 },
    /// io has run the operation of `owner`.
    Ran { owner: Token },
    /// The deadline of the operation of `owner` passes.
    Deadline { owner: Token },
}

/// An operation in flight, as io keeps it.
struct Pending {
    work: Work,
    /// When it ends, and when its deadline passes, as deliveries.
    ran: (Time, u64),
    deadline: (Time, u64),
}

/// What io does for an operation in flight.
enum Work {
    /// A file operation, run on the checkout when it is due.
    File(Op),
    /// A command, running since `since`.
    Command { started: translate::Started, since: Time },
    /// An operation that ends in this terminal, as a command that could not
    /// start.
    Ending(Done),
}

/// A session, as the world plays it.
struct Session {
    authority: Option<Authority>,
    script: VecDeque<Step>,
    state: State,
    /// The calls it sent, in order.
    calls: Vec<u64>,
    /// The calls of its last `Calls` step not answered yet.
    awaited: BTreeSet<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Its open is on its way, or the tools have not answered it.
    Opening,
    /// Running its script with its kit.
    Running {
        kit: Token,
    },
    /// Waiting for the answers to its last `Calls` step.
    Waiting {
        kit: Token,
    },
    /// Its close has been sent.
    Closing,
    Closed,
    Refused(Refusal),
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,
    checkout: Checkout,

    tools: tools::Model,
    env: Env<tools::Limits>,
    tools_in: VecDeque<tools::Event>,
    tools_out: Queue<tools::Request>,

    /// Deliveries in flight, by time and then by the order they were sent.
    wire: BTreeMap<(Time, u64), Delivery>,
    /// Names for sessions, calls and deliveries.
    serial: u64,
    sessions: BTreeMap<u64, Session>,
    /// Every call made, by name: whose it is, and its answer once it came.
    calls: BTreeMap<u64, (u64, Option<Outcome>)>,
    /// io's operations in flight.
    ops: BTreeMap<Token, Pending>,
    /// The versions io has told the tools of, for each place: the only ones a
    /// store may expect there.
    versions: BTreeSet<(u64, Vec<u8>, u64)>,
    /// Whether some kit may write each root: those none may write change only
    /// by the world's own changes, which `untouched` follows.
    writable: BTreeMap<u64, bool>,
    untouched: Option<BTreeMap<Vec<u8>, Vec<u8>>>,

    stats: Stats,
    trace: Vec<String>,
}

impl World {
    /// A world over `checkout`, whose roots name the repositories of the
    /// authorities its sessions open kits with.
    #[must_use]
    pub fn new(settings: Settings, checkout: Checkout) -> World {
        assert!(tools::worst_case(&settings.tools).is_some(), "the shell refuses limits it cannot provision");
        let room = tools::max_out(&settings.tools).saturating_mul(2);
        World {
            now: Time::ZERO,
            rng: Rng::new(settings.seed),
            settings,
            checkout,
            tools: tools::Model::new(&settings.tools),
            env: Env { now: Time::ZERO, limits: settings.tools },
            tools_in: VecDeque::new(),
            tools_out: Queue::with_capacity(room),
            wire: BTreeMap::new(),
            serial: 0,
            sessions: BTreeMap::new(),
            calls: BTreeMap::new(),
            ops: BTreeMap::new(),
            versions: BTreeSet::new(),
            writable: BTreeMap::new(),
            untouched: None,
            stats: Stats::default(),
            trace: Vec::new(),
        }
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    #[must_use]
    pub fn checkout(&self) -> &Checkout {
        &self.checkout
    }

    /// What crossed between the tools and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// A session that opens a kit with `authority` at `at`, runs `script`
    /// with it, and closes it. Returns the session's name.
    pub fn session(&mut self, at: Time, authority: Authority, script: Vec<Step>) -> u64 {
        for repo in &authority.repos {
            *self.writable.entry(repo.root.raw()).or_insert(false) |= repo.writable;
        }
        let session = self.next_serial();
        let state = State::Opening;
        let script = script.into();
        let entry = Session { authority: Some(authority), script, state, calls: Vec::new(), awaited: BTreeSet::new() };
        self.sessions.insert(session, entry);
        self.schedule(at, Delivery::Open { session });
        session
    }

    /// The answers to the calls `session` made, in the order it made them.
    #[must_use]
    pub fn answers(&self, session: u64) -> Vec<&Outcome> {
        let calls = &self.sessions.get(&session).expect("a session the world made").calls;
        calls.iter().map(|call| self.calls[call].1.as_ref().expect("every call is answered")).collect()
    }

    /// Why the tools refused to open a kit for `session`, if they did.
    #[must_use]
    pub fn refusal(&self, session: u64) -> Option<Refusal> {
        match self.sessions.get(&session).expect("a session the world made").state {
            State::Refused(refusal) => Some(refusal),
            State::Opening | State::Running { .. } | State::Waiting { .. } | State::Closing | State::Closed => None,
        }
    }

    /// The names of every session.
    pub fn sessions(&self) -> impl Iterator<Item = u64> + '_ {
        self.sessions.keys().copied()
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics if it takes more than `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            let Some(next) = self.next_time() else {
                self.assert_settled();
                return;
            };
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("the world did not settle in {iterations} iterations");
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        if self.untouched.is_none() {
            self.untouched = Some(self.read_only());
        }
        self.env.now = self.now;
        self.deliver();

        // The tools take their events while they have room for what one more
        // may produce.
        let max_out = tools::max_out(&self.settings.tools);
        while self.tools_out.room() >= max_out {
            let Some(event) = self.tools_in.pop_front() else { break };
            self.log(&format!("tools <- {event:?}"));
            tools::step(&mut self.tools, &self.env, event, &mut self.tools_out);
        }

        // What they asked for, carried out at the end of the iteration.
        while let Some(request) = self.tools_out.pop() {
            self.log(&format!("tools -> {request:?}"));
            self.request(request);
        }

        // The reclaim point.
        self.tools.reclaim();
        let limits = &self.settings.tools;
        assert!(self.tools.kits() <= limits.kits, "kits stay within their slots");
        assert!(self.tools.jobs() <= limits.kits * limits.calls * 2, "jobs stay within their slots");
    }

    /// The tools' requests, answered the way the session and io would.
    fn request(&mut self, request: tools::Request) {
        match request {
            tools::Request::Opened { session, kit } => {
                let entry = self.session_mut(session.raw());
                assert_eq!(entry.state, State::Opening, "a kit opens once");
                entry.state = State::Running { kit };
                self.next(session.raw());
            }
            tools::Request::Refused { session, refusal } => {
                let entry = self.session_mut(session.raw());
                assert_eq!(entry.state, State::Opening, "an open is answered once");
                entry.state = State::Refused(refusal);
            }
            tools::Request::Answer { to, outcome } => {
                let call = to.into_token().raw();
                let (session, answer) = self.calls.get_mut(&call).expect("an answer to a call that was made");
                assert!(answer.is_none(), "a call is answered once");
                *answer = Some(outcome);
                let session = *session;
                let entry = self.session_mut(session);
                entry.awaited.remove(&call);
                if let State::Waiting { kit } = entry.state
                    && entry.awaited.is_empty()
                {
                    entry.state = State::Running { kit };
                    self.next(session);
                }
            }
            tools::Request::Closed { session } => {
                let entry = self.sessions.get(&session.raw()).expect("a session the world made");
                assert_eq!(entry.state, State::Closing, "a kit closes once, when asked");
                for call in &entry.calls {
                    assert!(self.calls[call].1.is_some(), "a kit answers every call before it closes");
                }
                self.session_mut(session.raw()).state = State::Closed;
            }
            tools::Request::Io { owner, op, deadline } => {
                if let Op::Store { at, expect: Expect::Is { version }, .. } = &op {
                    let known = (at.root.raw(), at.path.to_vec(), version.raw()[0]);
                    assert!(self.versions.contains(&known), "a store expects a version io gave for its place");
                }
                let (work, ran) = self.start(op);
                let ran = self.schedule(ran, Delivery::Ran { owner });
                let deadline = self.schedule(deadline, Delivery::Deadline { owner });
                let pending = Pending { work, ran, deadline };
                assert!(self.ops.insert(owner, pending).is_none(), "an operation's owner has one in flight");
                self.stats.ops += 1;
            }
            tools::Request::CancelIo { owner } => {
                // An operation that has ended has its terminal on the way: the
                // cancel lost the race and changes nothing.
                if !self.ops.contains_key(&owner) {
                    self.stats.stale_cancels += 1;
                    return;
                }
                self.stats.cancels += 1;
                if self.rng.chance(self.settings.late_cancels) {
                    self.stats.late_cancels += 1;
                    return;
                }
                let pending = self.ops.remove(&owner).expect("checked above");
                self.wire.remove(&pending.ran);
                self.wire.remove(&pending.deadline);
                self.abandon(pending.work);
                self.tools_in.push_back(tools::Event::Done { owner, done: Done::Cancelled });
            }
        }
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(entry) = self.wire.first_entry() {
            if entry.key().0 > self.now {
                break;
            }
            match entry.remove() {
                Delivery::Open { session } => {
                    let authority = self.session_mut(session).authority.take().expect("a session opens once");
                    self.tools_in.push_back(tools::Event::Open { session: Token::new(session), authority });
                }
                Delivery::Next { session } => self.step(session),
                Delivery::Ran { owner } => {
                    let pending = self.ops.remove(&owner).expect("a run is withdrawn when its operation ends first");
                    self.wire.remove(&pending.deadline);
                    let done = match pending.work {
                        Work::File(_) if self.rng.chance(self.settings.faults) => {
                            self.stats.faults += 1;
                            Done::Failed { fault: self.fault() }
                        }
                        Work::File(op) => self.perform(op),
                        Work::Command { started, since: _ } => {
                            self.checkout.finish(&started.process);
                            self.assert_untouched();
                            let program = &started.process.program;
                            translate::exited(Some(program.exit), &program.output, started.head, started.tail)
                        }
                        Work::Ending(done) => done,
                    };
                    self.tools_in.push_back(tools::Event::Done { owner, done });
                }
                Delivery::Deadline { owner } => {
                    let pending =
                        self.ops.remove(&owner).expect("a deadline is withdrawn when its operation ends first");
                    self.wire.remove(&pending.ran);
                    let done = match &pending.work {
                        // io kills a command at its deadline, and tells what
                        // it wrote by then.
                        Work::Command { started, since } => {
                            let program = &started.process.program;
                            let ran = u128::from(self.now.saturating_since(*since).as_nanos());
                            let written = usize::try_from(
                                ran * program.output.len() as u128 / program.duration.as_nanos().max(1),
                            )
                            .expect("no more than the output")
                            .min(program.output.len());
                            translate::exited(None, &program.output[..written], started.head, started.tail)
                        }
                        Work::File(_) | Work::Ending(_) => Done::TimedOut,
                    };
                    self.abandon(pending.work);
                    self.tools_in.push_back(tools::Event::Done { owner, done });
                    self.stats.timeouts += 1;
                }
            }
        }
    }

    /// Starts `op`, and says when it ends.
    fn start(&mut self, op: Op) -> (Work, Time) {
        let latency = self.now.saturating_add(self.draw(self.settings.io));
        match op {
            Op::Spawn { cwd, command, env, roots, head, tail } => {
                self.stats.commands += 1;
                let started = if self.rng.chance(self.settings.faults) {
                    self.stats.faults += 1;
                    Err(Done::Failed { fault: self.fault() })
                } else {
                    translate::spawn(&self.checkout, &cwd, &command, &env, &roots, (head, tail))
                };
                match started {
                    Ok(started) => {
                        let duration = u64::try_from(started.process.program.duration.as_nanos()).expect("short");
                        let ends = self.now.saturating_add(Duration::from_nanos(duration));
                        (Work::Command { started, since: self.now }, ends)
                    }
                    Err(done) => (Work::Ending(done), latency),
                }
            }
            op @ (Op::Load { .. } | Op::Scan { .. } | Op::Store { .. } | Op::Search { .. }) => {
                if let Op::Store { at, expect: Expect::Is { version }, .. } = &op {
                    let known = (at.root.raw(), at.path.to_vec(), version.raw()[0]);
                    assert!(self.versions.contains(&known), "a store expects a version io gave for its place");
                }
                (Work::File(op), latency)
            }
        }
    }

    /// Abandons `work`, at its deadline or for a cancel. What it did may
    /// have taken effect all the same: a store renamed into place, a command's
    /// changes made before it was killed.
    fn abandon(&mut self, work: Work) {
        if !self.rng.chance(self.settings.late_effects) {
            return;
        }
        match work {
            Work::File(op) => drop(translate::perform(&mut self.checkout, op)),
            Work::Command { started, since: _ } => self.checkout.finish(&started.process),
            Work::Ending(_) => return,
        }
        self.assert_untouched();
        self.stats.late_effects += 1;
    }

    /// Runs `op` on the checkout, and notes the version it tells of.
    fn perform(&mut self, op: Op) -> Done {
        let at = match &op {
            Op::Load { at, .. } | Op::Scan { at, .. } | Op::Store { at, .. } | Op::Search { at, .. } => at.clone(),
            Op::Spawn { .. } => unreachable!("a spawn is started, not run"),
        };
        let done = translate::perform(&mut self.checkout, op);
        self.assert_untouched();
        match &done {
            Done::Loaded { version, .. } | Done::Stored { version } => {
                self.versions.insert((at.root.raw(), at.path.to_vec(), version.raw()[0]));
            }
            Done::Scanned { .. }
            | Done::Conflict { .. }
            | Done::Missing
            | Done::NotFile
            | Done::Linked
            | Done::Exited { .. }
            | Done::Found { .. }
            | Done::NotDirectory
            | Done::TooLarge { .. }
            | Done::Escapes
            | Done::Failed { .. }
            | Done::TimedOut
            | Done::Cancelled => {}
        }
        done
    }

    /// The session takes the next step of its script, or closes its kit once
    /// it has run out.
    fn step(&mut self, session: u64) {
        let entry = self.session_mut(session);
        let State::Running { kit } = entry.state else {
            unreachable!("a session steps while it runs");
        };
        let Some(step) = entry.script.pop_front() else {
            entry.state = State::Closing;
            self.tools_in.push_back(tools::Event::Close { kit });
            return;
        };
        match step {
            Step::Calls(calls) => {
                let sent = self.send(session, kit, calls);
                let entry = self.session_mut(session);
                entry.awaited.extend(sent);
                if entry.awaited.is_empty() {
                    self.next(session);
                } else {
                    entry.state = State::Waiting { kit };
                }
            }
            Step::Send(calls) => {
                drop(self.send(session, kit, calls));
                self.next(session);
            }
            Step::Change(change) => {
                change(&mut self.checkout);
                self.untouched = Some(self.read_only());
                self.log("checkout changed");
                self.next(session);
            }
            Step::Sleep(span) => {
                let at = self.now.saturating_add(span);
                self.schedule(at, Delivery::Next { session });
            }
            Step::Latency(span) => {
                self.settings.io = span;
                self.next(session);
            }
        }
    }

    /// Sends `calls` to `kit`, for `session`, and returns their names.
    fn send(&mut self, session: u64, kit: Token, calls: Vec<Call>) -> Vec<u64> {
        let mut names = Vec::new();
        for call in calls {
            let name = self.next_serial();
            self.calls.insert(name, (session, None));
            self.session_mut(session).calls.push(name);
            let deadline = self.now.saturating_add(self.settings.call_timeout);
            let reply_to = ReplyTo::new(Token::new(name));
            self.tools_in.push_back(tools::Event::Call { kit, reply_to, call, deadline });
            self.stats.calls += 1;
            names.push(name);
        }
        names
    }

    /// Schedules the session's next step, after it has thought about it.
    fn next(&mut self, session: u64) {
        let at = self.now.saturating_add(self.draw(self.settings.think));
        self.schedule(at, Delivery::Next { session });
    }

    fn has_work_now(&self) -> bool {
        !self.tools_in.is_empty() || self.wire.first_key_value().is_some_and(|((at, _), _)| *at <= self.now)
    }

    fn next_time(&self) -> Option<Time> {
        self.wire.first_key_value().map(|((at, _), _)| *at)
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.tools.kits(), 0, "every kit has closed and been reclaimed");
        assert_eq!(self.tools.jobs(), 0, "every job has ended and been reclaimed");
        assert!(self.ops.is_empty(), "no operation is in flight");
        assert!(self.wire.is_empty() && self.tools_in.is_empty(), "nothing is on its way");
        for (name, session) in &self.sessions {
            let ended = matches!(session.state, State::Closed | State::Refused(_));
            assert!(ended, "session {name} ended, not {:?}", session.state);
        }
        for (call, (_, answer)) in &self.calls {
            assert!(answer.is_some(), "call {call} was answered");
        }
    }

    /// The files of the repositories no kit may write.
    fn read_only(&self) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let mut files = BTreeMap::new();
        for (root, writable) in &self.writable {
            if *writable {
                continue;
            }
            let at = self.checkout.root_path(*root);
            for (path, content) in self.checkout.files() {
                let beneath = at.is_empty() || path == at || path.strip_prefix(at).is_some_and(|rest| rest[0] == b'/');
                if beneath {
                    files.insert(path.to_vec(), content.to_vec());
                }
            }
        }
        files
    }

    /// Read-only repositories change only by the world's own changes.
    fn assert_untouched(&self) {
        let untouched = self.untouched.as_ref().expect("taken at the first iteration");
        assert_eq!(&self.read_only(), untouched, "a read-only repository changed");
    }

    fn session_mut(&mut self, session: u64) -> &mut Session {
        self.sessions.get_mut(&session).expect("a session the world made")
    }

    fn fault(&mut self) -> Fault {
        match self.rng.below(3) {
            0 => Fault::Denied,
            1 => Fault::NoSpace,
            _ => Fault::Other,
        }
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) -> (Time, u64) {
        let key = (at, self.next_serial());
        self.wire.insert(key, delivery);
        key
    }

    fn draw(&mut self, span: Span) -> Duration {
        Duration::from_nanos(self.rng.between(span.min.as_nanos(), span.max.as_nanos()))
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    fn log(&mut self, line: &str) {
        self.trace.push(format!("{:>16} {line}", self.now.as_nanos()));
    }
}

/// A repository of `checkout` at `mount`, made a root.
pub fn repo(checkout: &mut Checkout, mount: &[u8], writable: bool) -> Repo {
    let root = checkout.root(mount.strip_prefix(b"/").expect("a mount is absolute"));
    Repo { mount: translate::names(mount), root: translate::token(root), writable }
}

/// An authority whose relative paths start at `cwd`, and whose commands run
/// with a plain environment.
#[must_use]
pub fn authority(cwd: &[u8], repos: Vec<Repo>, grants: Grants) -> Authority {
    let var = |name: &[u8], value: &[u8]| Var { name: name.into(), value: value.into() };
    let env = Box::new([var(b"PATH", b"/usr/bin:/bin"), var(b"HOME", b"/home/agent")]);
    Authority { cwd: translate::names(cwd), repos: repos.into(), grants, env }
}
