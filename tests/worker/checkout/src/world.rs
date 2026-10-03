use std::collections::{BTreeMap, BTreeSet};

use skein_lib::{Duration, Rng, Time, Token};
use temper_fake_checkout::Checkout;
use temper_fake_checkout::git::{Created, Remote, Tree};
use temper_worker_domain_checkout::git::{Done, Fault, Kind, Missing, Op, Want};
use temper_worker_domain_checkout::{
    Cached, Domain, Event, Fact, Failure, Landing, Limits, MAX_OUT, Message, Outcome, Prepared, Refusal, Repository,
    Request, Spec, Start, worst_case,
};
use temper_world::{Key, Ledger, Schedule, Span, Stage, Trace};

use crate::client::{Client, Interrupt, Operation, Pick, Plan, Release, Repo};
use crate::forge::{Forge, Move};
use crate::translate;

/// Room in the domain's output queue beyond what one step may emit. Small, so
/// the loop's flow control (take an event only while there is room for what
/// it may produce) is exercised.
const SLACK: u32 = 3;

/// Who the worker is to the forge, in every spec.
pub const IDENTITY: &[u8] = b"temper-bot";

/// What a client's pushes and saves are committed with.
const TITLE: &[u8] = b"Change the notes";

/// The calm limits: room for a few workspaces of a few repositories.
pub const LIMITS: Limits = Limits {
    workspaces: 3,
    repositories: 3,
    name_bytes: 64,
    message_bytes: 256,
    remote_timeout: Duration::from_secs(60),
    local_timeout: Duration::from_secs(10),
    facts: 256,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world.
    pub seed: u64,
    pub checkout: Limits,
    /// Repositories on the forge.
    pub repositories: u32,
    /// Workstreams, each with repositories of its own among the forge's.
    pub workstreams: u32,
    /// How long io takes over an operation on the disk, and over one that
    /// reaches the forge.
    pub local: Span,
    pub remote: Span,
    /// How long a client thinks before each of its steps.
    pub think: Span,
    /// How long a cancel takes to reach io, and io to tell that a deadline
    /// passed or that a cancel won.
    pub network: Span,
    /// The chance, per mille, that io fails an operation on the worker's side.
    pub broken: u32,
    /// The chance, per mille, that io carries out an operation and then
    /// reports that it ran out of time, which leaves what it did in doubt.
    pub ambiguous: u32,
    /// The chance, per mille, that a directory the worker's first workspaces
    /// will have was left on the disk by an earlier worker.
    pub leftovers: u32,
    /// The chance, per mille, that an operation that reaches the forge finds
    /// its repository unreachable.
    pub unreachable: u32,
    /// The chance, per mille, that a push or a branch's creation is refused.
    pub refusing: u32,
    /// The chance, per mille, that a spec names something the forge does not
    /// have, for a repository whose start the world draws.
    pub missing: u32,
    /// The chance, per mille, that another party advances a writable
    /// repository's push branch once a client is prepared.
    pub advance: u32,
    /// The chance, per mille, that a spec names other repositories than its
    /// workstream's own.
    pub reshape: u32,
    /// The chance, per mille, that a client edits a writable repository
    /// before a push or a save.
    pub edit: u32,
    /// The chance, per mille, that a cancel loses its race: the operation
    /// ends of itself, and that is its terminal event.
    pub cancels_lost: u32,
    /// The grid every delivery is rounded up to, so that some come at the
    /// same instant; zero for none.
    pub granule: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: no faults, operations well within
    /// their deadlines, and clients that edit before every push.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            checkout: LIMITS,
            repositories: 4,
            workstreams: 4,
            local: Span::millis(1, 50),
            remote: Span::millis(10, 500),
            think: Span::millis(0, 1_000),
            network: Span::millis(1, 20),
            broken: 0,
            ambiguous: 0,
            leftovers: 0,
            unreachable: 0,
            refusing: 0,
            missing: 0,
            advance: 0,
            reshape: 0,
            edit: 1000,
            cancels_lost: 0,
            granule: Duration::ZERO,
        }
    }
}

/// What the world counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Prepares sent, and how they ended.
    pub prepares: u32,
    pub ready: u32,
    pub busy: u32,
    pub full: u32,
    pub invalid: u32,
    pub transient: u32,
    pub missing_repository: u32,
    pub missing_branch: u32,
    pub missing_commit: u32,
    pub refused: u32,
    pub prepares_aborted: u32,
    /// Pushes and saves answered with landings, and the landings.
    pub pushes: u32,
    pub saves: u32,
    pub landed: u32,
    pub moved: u32,
    pub failed: u32,
    pub push_refused: u32,
    pub unchanged: u32,
    pub landings_aborted: u32,
    /// Pushes and saves sent twice, the second refused as busy.
    pub busy_pushes: u32,
    /// Releases answered, aborts sent, and requests sent to a hold released.
    pub releases: u32,
    pub aborts: u32,
    pub stale: u32,
    /// Operations asked of io; those that ran out of time; those io failed;
    /// those a cancel ended; cancels that lost their race; cancels for an
    /// operation that ended in the iteration they were sent.
    pub ops: u32,
    pub op_timeouts: u32,
    pub op_broken: u32,
    pub cancels: u32,
    pub op_cancels: u32,
    pub cancels_lost: u32,
    pub cancels_crossed: u32,
    /// Operations that found their repository unreachable, or were refused.
    pub unreachable: u32,
    pub refusals: u32,
    /// Operations io carried out and reported as run out of time; pushes
    /// whose landing a verification found; workspaces made over something
    /// already there.
    pub ambiguous: u32,
    pub verified: u32,
    pub made_over: u32,
    /// Base branches created, and found created meanwhile.
    pub created: u32,
    pub exists: u32,
    /// Branches another party advanced.
    pub advances: u32,
    /// The most workspaces and holds the domain had at once.
    pub most_workspaces: u32,
    pub most_holds: u32,
}

/// The facts the domain told, by kind, as the loop drained them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Told {
    pub refused: u32,
    pub reused: u32,
    pub rebuilt: u32,
    pub new: u32,
    pub evicted: u32,
    pub started: u32,
    pub ended: u32,
    pub aborting: u32,
    pub prepared: u32,
    pub pushed: u32,
    pub released: u32,
}

/// Something on its way, delivered at its time.
#[derive(Debug)]
enum Delivery {
    /// A client sends its prepare.
    Prepare { client: u64 },
    /// A client takes its next step: edits and a push, a save, or its
    /// release.
    Act { client: u64 },
    /// A client interrupts.
    Interrupt { client: u64, interrupt: Interrupt },
    /// An operation of io ends.
    Ran { owner: Token },
    /// Another party advances a branch.
    Advance { repository: Vec<u8>, branch: Vec<u8> },
}

/// A repository on the forge: its directory in a workspace, its address on
/// the forge, and its first commit.
#[derive(Clone, Debug)]
struct Hosted {
    name: Vec<u8>,
    remote: Vec<u8>,
    first: u64,
}

/// An operation in flight in io: its end on the way, and what it does then.
#[derive(Debug)]
struct Pending {
    delivery: Key,
    work: Work,
    /// The repository whose push it verifies, by its remote, if it does.
    verifies: Option<Vec<u8>>,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "fixed diagnostic tails keep boundary records bounded without allocation"
)]
enum Work {
    /// It runs when it ends.
    Perform(Op),
    /// It runs when it ends, and io reports that it ran out of time.
    Ambiguous(Op),
    /// It ends so, having done nothing.
    Ending(Done),
}

/// A push a hold asked for: of which commit, to which branch, and how it was
/// verified.
#[derive(Debug)]
struct Attempt {
    commit: u64,
    branch: Vec<u8>,
    verified: Verified,
}

/// What the fetch that verified a push found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verified {
    /// No fetch verified it.
    Not,
    /// The branch was at this commit.
    At(u64),
    /// The fetch failed.
    Failed,
}

#[derive(Debug)]
pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,
    domain: Domain,
    stage: Stage<Limits, Event, Request>,
    wire: Schedule<Delivery>,
    disk: Checkout,
    forge: Forge,
    /// The forge's repositories.
    repositories: Vec<Hosted>,
    /// Each workstream's repositories, by place among the forge's.
    workstreams: Vec<Vec<usize>>,
    clients: BTreeMap<u64, Client>,
    /// The client of each hold.
    holds: BTreeMap<Token, u64>,
    /// The hold each workspace is held by, as its operations show.
    held_by: BTreeMap<Token, Token>,
    /// Every workspace an operation named.
    workspaces: BTreeSet<Token>,
    /// The commit each hold checked each repository out at.
    checked_out: BTreeMap<(Token, Vec<u8>), u64>,
    /// The pushes of each hold's push or save under way, by remote.
    attempts: BTreeMap<(Token, Vec<u8>), Attempt>,
    ops: Ledger<Token, Pending>,
    cancel_lost: BTreeSet<Token>,
    stats: Stats,
    told: Told,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(worst_case(&settings.checkout).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let mut forge = Forge::new(settings.seed);
        let mut repositories = Vec::new();
        for place in 0..settings.repositories {
            let name = format!("r{place}").into_bytes();
            let remote = format!("forge/r{place}").into_bytes();
            let tree =
                Tree::from([(b"README".to_vec(), name.clone()), (b"src/lib.rs".to_vec(), b"fn f() {}".to_vec())]);
            let first = forge.repository(&remote, b"main", tree);
            repositories.push(Hosted { name, remote, first });
        }
        let mut workstreams = Vec::new();
        for _ in 0..settings.workstreams {
            workstreams.push(draw_repositories(&mut rng, &settings));
        }
        // What an earlier worker left where the first workspaces go: io names
        // a new workspace by its slot, in the high half of its token.
        let mut disk = Checkout::new();
        for slot in 0..u64::from(settings.checkout.workspaces) {
            if rng.chance(settings.leftovers) {
                let dir = translate::dir(Token::new(slot << 32));
                disk.write(&[dir.as_slice(), b"/r0/.git/HEAD"].concat(), b"stale");
                disk.write(&[dir.as_slice(), b"/r0/half-written"].concat(), b"stale");
            }
        }
        World {
            now: Time::ZERO,
            rng,
            settings,
            domain: Domain::new(&settings.checkout),
            stage: Stage::new(settings.checkout, MAX_OUT, MAX_OUT + SLACK),
            wire: Schedule::new(),
            disk,
            forge,
            repositories,
            workstreams,
            clients: BTreeMap::new(),
            holds: BTreeMap::new(),
            held_by: BTreeMap::new(),
            workspaces: BTreeSet::new(),
            checked_out: BTreeMap::new(),
            attempts: BTreeMap::new(),
            ops: Ledger::new("operation"),
            cancel_lost: BTreeSet::new(),
            stats: Stats::default(),
            told: Told::default(),
            trace: Trace::default(),
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

    /// The facts the domain told, and how many it dropped for want of room.
    #[must_use]
    pub fn told(&self) -> (Told, u64) {
        (self.told, self.domain.facts_lost())
    }

    /// What crossed between the domain and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    #[must_use]
    pub fn forge(&self) -> &Forge {
        &self.forge
    }

    /// The forge's address for the repository at `place`.
    #[must_use]
    pub fn repository(&self, place: usize) -> &[u8] {
        &self.repositories[place].remote
    }

    /// The repositories of the workstream at `place`, by place among the
    /// forge's.
    #[must_use]
    pub fn workstream(&self, place: u32) -> &[usize] {
        &self.workstreams[usize::try_from(place).expect("a small world")]
    }

    /// Has a client carry out `plan`, sending its prepare at `at`. Returns
    /// the world's name for it.
    pub fn submit(&mut self, at: Time, plan: Plan) -> u64 {
        assert!(plan.workstream < self.settings.workstreams, "a workstream of the world");
        let client = self.wire.name();
        self.clients.insert(client, Client::new(plan));
        self.schedule(at, Delivery::Prepare { client });
        client
    }

    #[must_use]
    pub fn client(&self, client: u64) -> &Client {
        self.clients.get(&client).expect("a client submitted")
    }

    /// Every client, by the world's name for it.
    pub fn clients(&self) -> impl Iterator<Item = (u64, &Client)> {
        self.clients.iter().map(|(client, state)| (*client, state))
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics if it takes more than `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            let Some(next) = self.wire.next_time() else {
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
        self.stage.tick(self.now);
        self.deliver();
        while let Some(event) = self.stage.next_event() {
            self.log(&format!("checkout <- {}", describe_event(&event)));
            temper_worker_domain_checkout::step(&mut self.domain, &self.stage.env, event, &mut self.stage.out);
        }
        // The facts, drained as the shell would write them out.
        while let Some(fact) = self.domain.pop_fact() {
            self.tell(&fact);
        }
        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.stage.out.pop() {
            self.request(request);
        }
        // What the iteration held at its most, before the reclaim point.
        let limits = self.settings.checkout;
        assert!(self.domain.workspaces() <= limits.workspaces, "the cache stays within its bound");
        assert!(self.domain.holds() <= limits.workspaces * 2, "holds stay within their slots");
        assert!(self.workspaces.len() <= usize::try_from(limits.workspaces).expect("small"), "no more directories");
        self.stats.most_workspaces = self.stats.most_workspaces.max(self.domain.workspaces());
        self.stats.most_holds = self.stats.most_holds.max(self.domain.holds());
        self.domain.reclaim();
    }

    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Prepare { client } => self.prepare(client),
                Delivery::Act { client } => self.act(client),
                Delivery::Interrupt { client, interrupt } => self.interrupt(client, interrupt),
                Delivery::Ran { owner } => self.ran(owner),
                Delivery::Advance { repository, branch } => self.advance(&repository, &branch),
            }
        }
    }

    /// Another party advances `branch` of `repository`, or creates it from the
    /// default branch if the forge does not have it.
    fn advance(&mut self, repository: &[u8], branch: &[u8]) {
        self.forge.at(self.now);
        if self.forge.branch(repository, branch).is_some() {
            let content = format!("theirs {}", self.wire.name()).into_bytes();
            self.forge.advance(repository, branch, b"THEIRS", &content);
            self.stats.advances += 1;
            self.log(&format!("forge: {} advanced", String::from_utf8_lossy(branch)));
        } else {
            let main = self.forge.branch(repository, b"main").expect("every repository has its default branch");
            let created = self.forge.create_branch(repository, branch, main);
            assert_eq!(created, Ok(Created::Created), "another party creates a branch where there is none");
            self.log(&format!("forge: {} created", String::from_utf8_lossy(branch)));
        }
        self.check_moves();
    }

    // The client.

    /// The client sends its prepare, for a spec drawn from its plan and what
    /// the forge has.
    fn prepare(&mut self, name: u64) {
        let plan = self.client(name).plan;
        let workstream = usize::try_from(plan.workstream).expect("small");
        let mut places = self.workstreams[workstream].clone();
        if self.rng.chance(self.settings.reshape) {
            places = draw_repositories(&mut self.rng, &self.settings);
        }
        let base = format!("base/{workstream}").into_bytes();
        let saved = format!("saved/{workstream}").into_bytes();
        let mut repositories = Vec::new();
        let mut repos = Vec::new();
        for (index, place) in places.into_iter().enumerate() {
            let writable = index == 0 || self.rng.chance(500);
            let pick = match plan.start {
                Some(pick) => pick,
                None => self.draw_pick(),
            };
            // A read-only repository starts from what exists: no base branch
            // is created for it.
            let pick = match pick {
                Pick::Base | Pick::Saved if !writable => Pick::Branch,
                Pick::Base
                | Pick::Branch
                | Pick::Commit
                | Pick::Saved
                | Pick::MissingRepository
                | Pick::MissingBranch
                | Pick::MissingCommit => pick,
            };
            let Hosted { mut name, mut remote, first } = self.repositories[place].clone();
            let mut commit = None;
            let start = match pick {
                Pick::Branch => Start::Branch { branch: b"main".as_slice().into() },
                Pick::Commit => {
                    commit = Some(first);
                    Start::Commit { commit: translate::commit(first) }
                }
                Pick::Saved if self.forge.branch(&remote, &saved).is_some() => {
                    Start::Saved { branch: saved.clone().into() }
                }
                Pick::Base | Pick::Saved => Start::Base { branch: base.clone().into() },
                Pick::MissingRepository => {
                    name = format!("nowhere-{index}").into_bytes();
                    remote = format!("forge/nowhere-{index}").into_bytes();
                    Start::Branch { branch: b"main".as_slice().into() }
                }
                Pick::MissingBranch => Start::Branch { branch: b"gone".as_slice().into() },
                Pick::MissingCommit => {
                    commit = Some(1_000_000);
                    Start::Commit { commit: translate::commit(1_000_000) }
                }
            };
            let push = if writable { Some(base.clone().into()) } else { None };
            let identity = IDENTITY.into();
            let spec = Repository { name: name.clone().into(), remote: remote.clone().into(), start, identity, push };
            repositories.push(spec);
            repos.push(Repo { name, remote, writable, commit });
        }
        // Another party may create or advance a base branch meanwhile.
        let first = repos.first().expect("a workstream has a repository");
        let hosted = self.repositories.iter().any(|hosted| hosted.remote == first.remote);
        if hosted && self.rng.chance(self.settings.advance) {
            let delivery = Delivery::Advance { repository: first.remote.clone(), branch: base.clone() };
            let at = self.now.saturating_add(self.settings.remote.draw(&mut self.rng));
            self.schedule(at, delivery);
        }
        let mut key = format!("work-{workstream}").into_bytes();
        if plan.invalid {
            // Beyond the limits: no workstream, or a directory that is not one
            // safe path component, or named twice.
            let names: [&[u8]; 5] = [b"..", b".", b".GIT", b"a/b", b"a\0b"];
            let first = repositories.first().expect("a workstream has a repository").name.clone();
            match self.rng.below(7) {
                0 => key = Vec::new(),
                1 => repositories.push(Repository {
                    name: first.clone(),
                    remote: first,
                    start: Start::Branch { branch: b"main".as_slice().into() },
                    identity: IDENTITY.into(),
                    push: None,
                }),
                way => repositories[0].name = names[usize::try_from(way - 2).expect("small")].into(),
            }
        }
        let spec = Spec { key: key.into(), repositories: repositories.into() };
        let client = self.clients.get_mut(&name).expect("a client submitted");
        client.repos = repos;
        client.operation = Some(Operation::Prepare);
        self.stage.push(Event::Prepare { client: Token::new(name), spec });
        self.stats.prepares += 1;
        if let Some((after, interrupt)) = plan.interrupt {
            self.schedule(self.now.saturating_add(after), Delivery::Interrupt { client: name, interrupt });
        }
    }

    fn draw_pick(&mut self) -> Pick {
        if self.rng.chance(self.settings.missing) {
            return match self.rng.below(3) {
                0 => Pick::MissingRepository,
                1 => Pick::MissingBranch,
                _ => Pick::MissingCommit,
            };
        }
        match self.rng.below(10) {
            0..=3 => Pick::Base,
            4 => Pick::Branch,
            5 => Pick::Commit,
            _ => Pick::Saved,
        }
    }

    /// The client's next step, once it has thought: edits and a push, a
    /// save, or its release.
    fn act(&mut self, name: u64) {
        let client = self.clients.get_mut(&name).expect("a client submitted");
        if client.release != Release::NotAsked {
            return;
        }
        assert!(client.operation.is_none(), "a client acts between its operations");
        let ready = match client.prepared {
            Some(Prepared::Ready { .. }) => true,
            Some(Prepared::Failed { .. } | Prepared::Aborted) => false,
            Some(Prepared::Refused { .. }) | None => unreachable!("a client acts once it holds a workspace"),
        };
        if ready && client.pushes_left > 0 {
            client.pushes_left -= 1;
            self.edit(name);
            self.send_push(name, false);
        } else if ready && client.save_left {
            client.save_left = false;
            self.edit(name);
            self.send_push(name, true);
        } else {
            self.release(name);
        }
    }

    /// The client edits its writable repositories, as a run's tools would:
    /// adds a file, rewrites one, or removes one.
    fn edit(&mut self, name: u64) {
        let client = self.clients.get(&name).expect("a client submitted");
        let workspace = client.workspace.expect("a client edits a workspace it has ready");
        let repos: Vec<Repo> = client.repos.iter().filter(|repo| repo.writable).cloned().collect();
        for repo in repos {
            if !self.rng.chance(self.settings.edit) {
                continue;
            }
            let dir = [translate::dir(workspace).as_slice(), b"/", &repo.name].concat();
            let serial = self.wire.name();
            let content = format!("edit {serial} by client {name}").into_bytes();
            let files: Vec<Vec<u8>> = self.disk.tree(&dir).into_keys().filter(|path| !in_git(path)).collect();
            let pick = self.rng.below(3);
            if pick == 0 || files.is_empty() {
                self.disk.write(&[dir.as_slice(), format!("/notes/{serial}").as_bytes()].concat(), &content);
                continue;
            }
            let count = u64::try_from(files.len()).expect("small");
            let path = &files[usize::try_from(self.rng.below(count)).expect("small")];
            let at = [dir.as_slice(), b"/", path].concat();
            if pick == 1 || files.len() == 1 {
                self.disk.write(&at, &content);
            } else {
                self.disk.remove(&at);
            }
        }
    }

    /// The client asks to push, or to save, what its working trees hold now.
    fn send_push(&mut self, name: u64, save: bool) {
        let client = self.clients.get(&name).expect("a client submitted");
        let workspace = client.workspace.expect("a client pushes a workspace it has ready");
        let hold = client.hold.expect("a client pushes what it holds");
        let workstream = client.plan.workstream;
        let mut left = Vec::new();
        for repo in &client.repos {
            left.push(self.tree(workspace, &repo.name));
        }
        let twice = client.plan.twice;
        let client = self.clients.get_mut(&name).expect("a client submitted");
        client.left = left;
        let (event, operation) = if save {
            let branch = format!("saved/{workstream}").into_bytes().into();
            (Event::Save { hold, branch, message: message() }, Operation::Save)
        } else {
            (Event::Push { hold, message: message() }, Operation::Push)
        };
        client.operation = Some(operation);
        self.stage.push(event);
        if twice {
            client.refusals += 1;
            let again = if save {
                Event::Save { hold, branch: format!("saved/{workstream}").into_bytes().into(), message: message() }
            } else {
                Event::Push { hold, message: message() }
            };
            self.stage.push(again);
            self.stats.busy_pushes += 1;
        }
    }

    fn release(&mut self, name: u64) {
        let client = self.clients.get_mut(&name).expect("a client submitted");
        let hold = client.hold.expect("a client releases what it holds");
        client.release = Release::Asked;
        self.stage.push(Event::Release { hold });
    }

    /// The client interrupts at a moment of its own.
    fn interrupt(&mut self, name: u64, interrupt: Interrupt) {
        let client = self.clients.get_mut(&name).expect("a client submitted");
        let Some(hold) = client.hold else {
            // Refused at the entrance: it holds nothing.
            return;
        };
        match client.release {
            Release::Done => {
                // Its handle is stale: whatever it sends is dropped.
                let stale = match interrupt {
                    Interrupt::Abort => Event::Abort { hold },
                    Interrupt::Release => Event::Release { hold },
                };
                self.stage.push(stale);
                self.stats.stale += 1;
                return;
            }
            Release::Asked => {
                self.stage.push(Event::Abort { hold });
                self.stats.aborts += 1;
                return;
            }
            Release::NotAsked => {}
        }
        match interrupt {
            Interrupt::Abort => {
                client.pushes_left = 0;
                self.stage.push(Event::Abort { hold });
                self.stats.aborts += 1;
            }
            Interrupt::Release => self.release(name),
        }
    }

    // What the domain asks for.

    fn request(&mut self, request: Request) {
        self.log(&format!("checkout -> {}", describe_request(&request)));
        match request {
            Request::Held { client, hold } => {
                let name = client.raw();
                let client = self.clients.get_mut(&name).expect("the domain answers a client that asked");
                assert_eq!(client.operation, Some(Operation::Prepare), "a hold is for a prepare in flight");
                assert!(client.hold.is_none(), "a prepare is held once");
                client.hold = Some(hold);
                assert!(self.holds.insert(hold, name).is_none(), "every hold has a name of its own");
            }
            Request::Prepared { client, prepared } => self.prepared(client.raw(), prepared),
            Request::Pushed { client, outcome } => self.pushed(client.raw(), outcome, false),
            Request::Saved { client, outcome } => self.pushed(client.raw(), outcome, true),
            Request::Released { client } => self.released(client.raw()),
            Request::Io { owner, op, deadline } => self.start_op(owner, op, deadline),
            Request::Cancel { owner } => self.cancel_op(owner),
        }
    }

    fn prepared(&mut self, name: u64, prepared: Prepared) {
        let client = self.clients.get_mut(&name).expect("the domain answers a client that asked");
        assert_eq!(client.operation.take(), Some(Operation::Prepare), "a prepare ends once");
        assert!(client.prepared.is_none(), "a client prepares once");
        client.prepared = Some(prepared);
        let stats = &mut self.stats;
        match prepared {
            Prepared::Ready { workspace } => {
                stats.ready += 1;
                client.workspace = Some(workspace);
                self.check_ready(name, workspace);
            }
            Prepared::Refused { refusal } => {
                assert!(client.hold.is_none(), "a prepare refused holds nothing");
                client.release = Release::Done;
                let count = match refusal {
                    Refusal::Busy => &mut stats.busy,
                    Refusal::Full => &mut stats.full,
                    Refusal::Invalid => &mut stats.invalid,
                };
                *count += 1;
                return;
            }
            Prepared::Failed { failure } => {
                let count = match failure {
                    Failure::Transient => &mut stats.transient,
                    Failure::Missing { missing: Missing::Repository, .. } => &mut stats.missing_repository,
                    Failure::Missing { missing: Missing::Branch, .. } => &mut stats.missing_branch,
                    Failure::Missing { missing: Missing::Commit, .. } => &mut stats.missing_commit,
                    Failure::Refused { .. } => &mut stats.refused,
                };
                *count += 1;
            }
            Prepared::Aborted => stats.prepares_aborted += 1,
        }
        self.think(name);
    }

    /// Checks a workspace just prepared: each repository holds the tree of
    /// the commit it was checked out at, which is the spec's if it named one.
    fn check_ready(&mut self, name: u64, workspace: Token) {
        let client = self.clients.get(&name).expect("a client submitted");
        let hold = client.hold.expect("a prepared client holds a workspace");
        let mut start = Vec::new();
        for repo in &client.repos {
            let commit = *self.checked_out.get(&(hold, repo.name.clone())).expect("each repository was checked out");
            if let Some(named) = repo.commit {
                assert_eq!(commit, named, "a repository starts at the commit its spec names");
            }
            let tree = self.tree(workspace, &repo.name);
            assert_eq!(tree, self.forge.tree(commit), "a repository is checked out at its starting point");
            start.push(tree);
        }
        let client = self.clients.get_mut(&name).expect("a client submitted");
        client.pushed.clone_from(&start);
        client.start = start;
        // Another party may advance a push branch meanwhile.
        if self.rng.chance(self.settings.advance) {
            let workstream = client.plan.workstream;
            let repo = client.repos.iter().find(|repo| repo.writable).expect("a spec's first repository is writable");
            let delivery = Delivery::Advance {
                repository: repo.remote.clone(),
                branch: format!("base/{workstream}").into_bytes(),
            };
            let at = self.now.saturating_add(self.settings.think.draw(&mut self.rng));
            self.schedule(at, delivery);
        }
    }

    fn pushed(&mut self, name: u64, outcome: Outcome, saved: bool) {
        let client = self.clients.get_mut(&name).expect("the domain answers a client that asked");
        let landings = match outcome {
            Outcome::Pushed { landings } => landings,
            Outcome::Refused { refusal } => {
                assert!(refusal == Refusal::Busy && client.refusals > 0, "only a push sent twice is refused");
                client.refusals -= 1;
                return;
            }
        };
        let expected = if saved { Operation::Save } else { Operation::Push };
        assert_eq!(client.operation.take(), Some(expected), "a push or a save ends once");
        let hold = client.hold.expect("a client pushes what it holds");
        assert_eq!(landings.len(), client.repos.len(), "a landing for each repository");
        let stats = &mut self.stats;
        if saved {
            stats.saves += 1;
        } else {
            stats.pushes += 1;
        }
        for (index, landing) in landings.iter().enumerate() {
            let repo = &client.repos[index];
            let left = &client.left[index];
            match landing {
                Landing::Landed { commit } => {
                    assert!(repo.writable, "only a writable repository is pushed");
                    let tree = self.forge.tree(translate::fake(*commit));
                    assert_eq!(&tree, left, "what landed is exactly the tree the client left");
                    if !saved {
                        client.pushed[index] = left.clone();
                    }
                    let attempt = self.attempts.get(&(hold, repo.remote.clone())).expect("what landed was pushed");
                    assert_eq!(attempt.commit, translate::fake(*commit), "what landed is what was pushed");
                    match attempt.verified {
                        Verified::Not => {}
                        Verified::At(tip) => {
                            assert_eq!(tip, attempt.commit, "a verified push landed");
                            stats.verified += 1;
                        }
                        Verified::Failed => panic!("a push whose verification failed did not land"),
                    }
                    stats.landed += 1;
                }
                Landing::Moved => {
                    assert!(repo.writable, "only a writable repository is pushed");
                    let attempt = self.attempts.get(&(hold, repo.remote.clone())).expect("what moved was pushed");
                    let tip = self.forge.branch(&repo.remote, &attempt.branch).expect("a moved branch exists");
                    assert!(!self.forge.is_ancestor(tip, attempt.commit), "a push is moved only if its branch moved");
                    stats.moved += 1;
                }
                Landing::Unchanged => {
                    if repo.writable {
                        let base = if saved { &client.start[index] } else { &client.pushed[index] };
                        assert_eq!(left, base, "nothing to push only if nothing changed");
                    }
                    stats.unchanged += 1;
                }
                Landing::Failed
                | Landing::Explained {
                    fault:
                        Fault::Missing { .. } | Fault::Unreachable | Fault::Broken | Fault::TimedOut | Fault::Cancelled,
                    ..
                } => {
                    // A push that landed is reported landed, unless the fetch
                    // that would have told so failed too.
                    if let Some(attempt) = self.attempts.get(&(hold, repo.remote.clone())) {
                        let tip = self.forge.branch(&repo.remote, &attempt.branch);
                        let unverified = attempt.verified == Verified::Failed;
                        assert!(tip != Some(attempt.commit) || unverified, "a push that failed did not land");
                    }
                    stats.failed += 1;
                }
                Landing::Refused | Landing::Explained { fault: Fault::Refused, .. } => stats.push_refused += 1,
                Landing::Aborted => stats.landings_aborted += 1,
            }
        }
        client.landings.push((saved, landings));
        self.attempts.retain(|(holder, _), _| *holder != hold);
        self.think(name);
    }

    fn released(&mut self, name: u64) {
        let client = self.clients.get_mut(&name).expect("the domain answers a client that asked");
        assert_eq!(client.release, Release::Asked, "a release answers one asked for, once");
        assert!(client.operation.is_none(), "an operation ends before its hold is released");
        client.release = Release::Done;
        let hold = client.hold.expect("a released client held a workspace");
        self.held_by.retain(|_, holder| *holder != hold);
        self.stats.releases += 1;
    }

    /// The client thinks before its next step, unless it is releasing.
    fn think(&mut self, name: u64) {
        let client = self.clients.get(&name).expect("a client submitted");
        if client.release == Release::NotAsked {
            let at = self.now.saturating_add(self.settings.think.draw(&mut self.rng));
            self.schedule(at, Delivery::Act { client: name });
        }
    }

    // io.

    fn start_op(&mut self, owner: Token, op: Op, deadline: Time) {
        let name = *self.holds.get(&owner).expect("an operation is a hold's");
        let client = self.clients.get(&name).expect("a hold's client");
        assert!(client.release != Release::Done, "no operation touches a workspace after its release");
        assert!(client.operation.is_some(), "an operation runs only while its client waits for one to end");
        let limits = self.settings.checkout;
        let timeout = if op.is_remote() { limits.remote_timeout } else { limits.local_timeout };
        assert_eq!(deadline, self.now.saturating_add(timeout), "an operation's deadline is by where it runs");
        if let Some(identity) = translate::identity(&op) {
            assert_eq!(identity, IDENTITY, "an operation acts as its repository's identity");
        }
        let workspace = translate::workspace(&op);
        if let Some(holder) = self.held_by.insert(workspace, owner) {
            assert_eq!(holder, owner, "a workspace is held by one hold at a time, and evicted only when idle");
        }
        self.workspaces.insert(workspace);
        let verifies = self.attempt(owner, client.operation, &op);
        let span = if op.is_remote() { self.settings.remote } else { self.settings.local };
        let mut ends = self.now.saturating_add(span.draw(&mut self.rng));
        let mut work = if self.rng.chance(self.settings.broken) {
            self.stats.op_broken += 1;
            Work::Ending(Done::Failed { fault: Fault::Broken })
        } else if self.rng.chance(self.settings.ambiguous) {
            self.stats.ambiguous += 1;
            Work::Ambiguous(op)
        } else {
            Work::Perform(op)
        };
        // io runs the race with the deadline, and tells it lost a moment
        // after the deadline passes, having done nothing.
        if ends > deadline {
            work = Work::Ending(Done::Failed { fault: Fault::TimedOut });
            ends = deadline.saturating_add(self.settings.network.draw(&mut self.rng));
            self.stats.op_timeouts += 1;
        }
        let delivery = self.schedule(ends, Delivery::Ran { owner });
        self.ops.open(owner, Pending { delivery, work, verifies });
        self.stats.ops += 1;
    }

    /// Notes a push `owner` asks for, while its client pushes or saves; and
    /// whether `op` is the fetch that verifies one: returns its remote if so.
    fn attempt(&mut self, owner: Token, operation: Option<Operation>, op: &Op) -> Option<Vec<u8>> {
        match operation {
            Some(Operation::Push | Operation::Save) => {}
            Some(Operation::Prepare) | None => return None,
        }
        match op {
            Op::Push { remote, commit, branch, .. } => {
                let attempt =
                    Attempt { commit: translate::fake(*commit), branch: branch.to_vec(), verified: Verified::Not };
                self.attempts.insert((owner, remote.to_vec()), attempt);
                None
            }
            Op::Fetch { remote, want: Want::Branch { branch }, .. } => {
                let attempt = self.attempts.get(&(owner, remote.to_vec())).expect("a push is verified once asked for");
                assert_eq!(&*attempt.branch, &**branch, "a push is verified on its branch");
                assert_eq!(attempt.verified, Verified::Not, "a push is verified once");
                Some(remote.to_vec())
            }
            Op::Fetch { .. } | Op::Make { .. } | Op::Clone { .. } | Op::Create { .. } | Op::CheckOut { .. } => {
                panic!("a push or a save commits, pushes and verifies, nothing else: {op:?}")
            }
            Op::Commit { .. } => None,
        }
    }

    /// io is asked to cancel the operation of `owner`: it ends cancelled
    /// after a network draw, having done nothing, unless it ends of itself
    /// first.
    fn cancel_op(&mut self, owner: Token) {
        let name = self.holds.get(&owner).expect("a cancel names a hold's operation");
        let client = self.clients.get(name).expect("a hold's client");
        assert_eq!(client.operation, Some(Operation::Prepare), "only a prepare's operation is cancelled, not a push's");
        self.stats.cancels += 1;
        let Some(pending) = self.ops.get(owner) else {
            // It ended in the iteration the cancel was sent.
            self.stats.cancels_crossed += 1;
            return;
        };
        let lost = self.rng.chance(self.settings.cancels_lost);
        if lost {
            self.cancel_lost.insert(owner);
            self.stats.cancels_lost += 1;
            return;
        }
        let key = pending.delivery;
        let at = self.now.saturating_add(self.settings.network.draw(&mut self.rng));
        let delivery = self.schedule(at, Delivery::Ran { owner });
        self.wire.withdraw(key).expect("an operation in flight has its end on the way");
        let pending = self.ops.get_mut(owner).expect("looked up above");
        (pending.delivery, pending.work) = (delivery, Work::Ending(Done::Failed { fault: Fault::Cancelled }));
        self.stats.op_cancels += 1;
    }

    /// The operation of `owner` ends, and io tells the checkout how.
    fn ran(&mut self, owner: Token) {
        let pending = self.ops.end(owner);
        self.cancel_lost.remove(&owner);
        let done = match pending.work {
            Work::Perform(op) => self.perform(owner, op),
            Work::Ambiguous(op) => {
                self.perform(owner, op);
                Done::Failed { fault: Fault::TimedOut }
            }
            Work::Ending(done) => done,
        };
        if let Some(remote) = pending.verifies {
            let attempt = self.attempts.get_mut(&(owner, remote)).expect("a push is verified once asked for");
            attempt.verified = match done {
                Done::Fetched { commit } => Verified::At(translate::fake(commit)),
                Done::Succeeded
                | Done::Committed { .. }
                | Done::Unchanged
                | Done::Exists
                | Done::Rejected
                | Done::Failed { .. }
                | Done::FailedWithOutput { .. } => Verified::Failed,
            };
        }
        self.stage.push(Event::Done { owner, done });
    }

    /// Runs `op` on the fakes, with the faults the world scripts, and checks
    /// what moved on the forge.
    fn perform(&mut self, owner: Token, op: Op) -> Done {
        // As it takes effect: its hold still holds its workspace, and its
        // client still waits for the operation it is part of.
        let workspace = translate::workspace(&op);
        assert_eq!(self.held_by.get(&workspace), Some(&owner), "an operation runs in a workspace its hold holds");
        let client = self.clients.get(self.holds.get(&owner).expect("a hold's")).expect("a hold's client");
        assert!(client.release != Release::Done, "nothing touches a workspace once its hold is released");
        assert!(client.operation.is_some(), "nothing touches a workspace once its client has heard the end");
        if op.kind() == Kind::Make && self.disk.exists(&translate::dir(workspace)) {
            self.stats.made_over += 1;
        }
        let remote = translate::remote(&op).map(<[u8]>::to_vec);
        let on_forge = remote.filter(|remote| self.repositories.iter().any(|hosted| hosted.remote == *remote));
        let writes = op.kind() == Kind::Create || op.kind() == Kind::Push;
        if let Some(name) = &on_forge {
            let reachable = !self.rng.chance(self.settings.unreachable);
            let refusing = writes && self.rng.chance(self.settings.refusing);
            self.forge.set_reachable(name, reachable);
            self.forge.set_refusing(name, refusing);
        }
        match &op {
            Op::CheckOut { at, commit } => {
                self.checked_out.insert((owner, at.repository.to_vec()), translate::fake(*commit));
            }
            Op::Make { .. }
            | Op::Clone { .. }
            | Op::Fetch { .. }
            | Op::Create { .. }
            | Op::Commit { .. }
            | Op::Push { .. } => {}
        }
        let creates = op.kind() == Kind::Create;
        self.forge.at(self.now);
        let done = translate::perform(&mut self.forge, &mut self.disk, op);
        if let Some(name) = &on_forge {
            self.forge.set_reachable(name, true);
            self.forge.set_refusing(name, false);
        }
        let new = self.check_moves();
        match done {
            Done::Failed { fault: Fault::Unreachable } | Done::FailedWithOutput { fault: Fault::Unreachable, .. } => {
                self.stats.unreachable += 1;
            }
            Done::Failed { fault: Fault::Refused } | Done::FailedWithOutput { fault: Fault::Refused, .. } => {
                self.stats.refusals += 1;
            }
            Done::Succeeded if creates => {
                assert!(new.len() == 1 && new[0].from.is_none(), "a base branch is created, never moved");
                self.stats.created += 1;
            }
            Done::Exists => {
                assert!(new.is_empty(), "a base branch that exists is left where it is");
                self.stats.exists += 1;
            }
            Done::Succeeded
            | Done::Fetched { .. }
            | Done::Committed { .. }
            | Done::Unchanged
            | Done::Rejected
            | Done::Failed { .. }
            | Done::FailedWithOutput { .. } => {}
        }
        done
    }

    /// The branches the forge moved since the world last looked, each a
    /// fast-forward.
    fn check_moves(&mut self) -> Vec<Move> {
        let moves = self.forge.moves();
        for Move { remote, branch, from, to } in &moves {
            if let Some(from) = from {
                assert!(
                    self.forge.is_ancestor(*from, *to),
                    "{}: {} moved only by a fast-forward",
                    String::from_utf8_lossy(remote),
                    String::from_utf8_lossy(branch)
                );
            }
        }
        moves
    }

    // Facts.

    fn tell(&mut self, fact: &Fact) {
        let told = &mut self.told;
        let count = match fact {
            Fact::Refused { .. } => &mut told.refused,
            Fact::Held { cached: Cached::Reused, .. } => &mut told.reused,
            Fact::Held { cached: Cached::Rebuilt, .. } => &mut told.rebuilt,
            Fact::Held { cached: Cached::New, .. } => &mut told.new,
            Fact::Held { cached: Cached::Evicted, .. } => &mut told.evicted,
            Fact::Started { .. } => &mut told.started,
            Fact::Ended { .. } => &mut told.ended,
            Fact::Aborting { .. } => &mut told.aborting,
            Fact::Prepared { .. } => &mut told.prepared,
            Fact::Pushed { .. } => &mut told.pushed,
            Fact::Released { .. } => &mut told.released,
        };
        *count += 1;
    }

    /// What the facts must add up to when none were dropped: what the world
    /// saw cross the boundary.
    fn assert_told(&self) {
        let (told, stats) = (&self.told, &self.stats);
        let refused = stats.busy + stats.full + stats.invalid;
        let admitted = stats.prepares - refused;
        assert_eq!(told.refused, refused + stats.busy_pushes, "a fact for every refusal");
        let held = told.reused + told.rebuilt + told.new + told.evicted;
        assert_eq!((held, told.prepared), (admitted, admitted), "a fact for every hold and its prepare's end");
        assert_eq!((told.started, told.ended), (stats.ops, stats.ops), "a fact for every operation and its end");
        assert_eq!(told.aborting, stats.cancels, "a fact for every cancel");
        assert_eq!(told.pushed, stats.pushes + stats.saves, "a fact for every push and save");
        assert_eq!(told.released, stats.releases, "a fact for every release");
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.domain.holds(), 0, "every hold has been released and reclaimed");
        assert_eq!(self.domain.idle(), self.domain.workspaces(), "every workspace is idle");
        assert!(self.ops.is_empty() && self.cancel_lost.is_empty(), "every operation has ended, once");
        assert!(self.held_by.is_empty(), "no workspace is held");
        assert!(self.wire.is_empty() && !self.stage.has_events(), "nothing is on its way");
        for (name, client) in &self.clients {
            assert_eq!(client.release, Release::Done, "client {name} released what it held, or was refused");
            assert!(client.operation.is_none() && client.refusals == 0, "client {name} heard every end");
        }
        for hosted in &self.repositories {
            for branch in self.forge.branches(&hosted.remote) {
                assert!(
                    branch.starts_with(b"base/") || branch.starts_with(b"saved/") || branch == b"main",
                    "only the workstreams' branches are made: {}",
                    String::from_utf8_lossy(&branch)
                );
            }
        }
        if self.domain.facts_lost() == 0 {
            self.assert_told();
        }
    }

    // The world's own.

    fn has_work_now(&self) -> bool {
        self.stage.has_events() || self.wire.is_due(self.now)
    }

    /// The files of the repository `name` in `workspace`, less its git
    /// directory.
    fn tree(&self, workspace: Token, name: &[u8]) -> Tree {
        let dir = [translate::dir(workspace).as_slice(), b"/", name].concat();
        let mut tree = self.disk.tree(&dir);
        tree.retain(|path, _| !in_git(path));
        tree
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) -> Key {
        // On a coarse grid, if the world has one, so that deliveries coincide.
        let granule = self.settings.granule.as_nanos();
        let at =
            if granule > 0 { Time::from_nanos(at.as_nanos().div_ceil(granule).saturating_mul(granule)) } else { at };
        self.wire.send(at, delivery)
    }

    fn log(&mut self, line: &str) {
        self.trace.log(self.now, line);
    }
}

/// A workstream's repositories: a few of the forge's, by place, in an order
/// of their own.
fn draw_repositories(rng: &mut Rng, settings: &Settings) -> Vec<usize> {
    let most = settings.checkout.repositories.min(settings.repositories);
    let count = rng.between(1, u64::from(most));
    let mut places: Vec<usize> = Vec::new();
    while u64::try_from(places.len()).expect("small") < count {
        let place = usize::try_from(rng.below(u64::from(settings.repositories))).expect("small");
        if !places.contains(&place) {
            places.push(place);
        }
    }
    places
}

fn message() -> Message {
    Message { title: TITLE.into(), body: b"What the run changed.".as_slice().into() }
}

fn in_git(path: &[u8]) -> bool {
    temper_fake_checkout::in_git(path)
}

fn describe_event(event: &Event) -> String {
    match event {
        Event::Prepare { client, spec } => {
            format!("prepare {} {:?}", client.raw(), String::from_utf8_lossy(&spec.key))
        }
        Event::Push { hold, .. } => format!("push {}", hold.raw()),
        Event::Save { hold, .. } => format!("save {}", hold.raw()),
        Event::Abort { hold } => format!("abort {}", hold.raw()),
        Event::Release { hold } => format!("release {}", hold.raw()),
        Event::Done { owner, done } => format!("done {} {done:?}", owner.raw()),
    }
}

fn describe_request(request: &Request) -> String {
    match request {
        Request::Held { client, hold } => format!("held {} {}", client.raw(), hold.raw()),
        Request::Prepared { client, prepared } => format!("prepared {} {prepared:?}", client.raw()),
        Request::Pushed { client, outcome } => format!("pushed {} {outcome:?}", client.raw()),
        Request::Saved { client, outcome } => format!("saved {} {outcome:?}", client.raw()),
        Request::Released { client } => format!("released {}", client.raw()),
        Request::Io { owner, op, deadline } => {
            format!(
                "io {} {:?} {:?} by {}",
                owner.raw(),
                op.kind(),
                translate::workspace(op).raw(),
                deadline.as_nanos()
            )
        }
        Request::Cancel { owner } => format!("cancel {}", owner.raw()),
    }
}
