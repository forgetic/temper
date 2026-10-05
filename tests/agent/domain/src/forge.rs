//! The one fake forge (`temper_fake_forge_domain`, testing.md, 4.2) that
//! everyone in the world meets: the engine's calls, through its protocol
//! layer as the world plays it, at the forge's latency and with its faults;
//! people's calls; and the git of io's working trees, the worker's clones,
//! fetches and pushes, which reach it as [`Direct`] calls.
//!
//! A direct call is made and answered in the same instant, at no latency and
//! with no fault of the forge's own: io's latency for an operation that
//! reaches the forge already stands for all of it, and its faults are the
//! world's to script (a call that cannot reach the forge, a repository that
//! refuses pushes). What the world does from outside goes the same way:
//! people's issues as they hand them in, with the record an earlier life of
//! the engine wrote ([`crate::desk`]), and another party moving a branch.
//! Whatever else the forge emits meanwhile, answers to others' calls whose
//! time came and webhooks, is kept for the world to route.

use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use temper_fake_checkout::git::{self, Created, Fault, Pushed, Remote, Tree, Want};
use temper_fake_forge_domain::api::{
    Answer, Checks, Cue, Error, File, Git, Op, Permission, Protection, Read, Setup, What, Write,
};
use temper_fake_forge_domain::{self as forge, Config, Domain, Event, MAX_OUT, Request};
use temper_legacy_engine_domain::forge::Position;
use temper_legacy_engine_domain_world::codec;
use temper_legacy_engine_domain_world::deployment::{
    CI, ENGINE, MAIN, PEOPLE, REPOSITORIES, REVIEWER, STRANGER, TRACKING, WORKER,
};
use temper_legacy_engine_forge_world::translate::recorded;

use crate::desk::{self, Hand};
use crate::fixture;

/// The names of direct calls: apart from the world's own.
const DIRECT: u64 = 1 << 63;

/// Another party, who moves branches under the runs.
pub const OTHER: u64 = 20;

/// The most fires one direct call takes to be answered: everything else due
/// at the same instant first.
const FIRES: u32 = 10_000;

/// Sets up every repository of the deployment on `domain`: its default branch
/// holding the fixture's tree, CI cued by the file it reads, or passing each
/// commit at the chance per mille `passes` if it is given, the default branch
/// protected, and its users.
pub fn setup(domain: &mut Domain, config: &Config, passes: Option<u32>) {
    for name in REPOSITORIES {
        let tree =
            fixture::tree().into_iter().map(|(path, content)| File { path: path.into(), content: content.into() });
        let setup = Setup {
            name: name.into(),
            default: MAIN.into(),
            tree: tree.collect(),
            labels: temper_legacy_engine_domain_world::deployment::LABELS.iter().map(|label| (*label).into()).collect(),
            checks: Checks {
                contexts: Box::new([b"ci".as_slice().into()]),
                latency_min: Duration::from_secs(1),
                latency_max: Duration::from_secs(40),
                silent: 0,
                passes: passes.unwrap_or(1_000),
                reruns: 0,
                cue: match passes {
                    Some(_) => None,
                    None => Some(Cue {
                        path: temper_legacy_engine_domain_world::deployment::CUE.into(),
                        green: temper_legacy_engine_domain_world::deployment::GREEN.into(),
                    }),
                },
            },
            protection: Some(Protection {
                branch: MAIN.into(),
                contexts: Box::new([b"ci".as_slice().into()]),
                approvals: 1,
                dismiss_stale: true,
            }),
            hooked: true,
        };
        forge::repository(domain, config, setup);
        forge::grant(domain, name, ENGINE, Permission::Write);
        forge::grant(domain, name, WORKER, Permission::Write);
        forge::grant(domain, name, CI, Permission::Write);
        forge::grant(domain, name, OTHER, Permission::Write);
        forge::grant(domain, name, PEOPLE[0], Permission::Admin);
        for user in PEOPLE[1..].iter().chain([&REVIEWER]) {
            forge::grant(domain, name, *user, Permission::Write);
        }
        forge::grant(domain, name, STRANGER, Permission::Read);
    }
}

/// The forge's configuration for a direct call: `config`'s, at no latency
/// and with no fault.
#[must_use]
pub fn direct(config: &Config) -> Config {
    Config {
        latency_min: Duration::ZERO,
        latency_max: Duration::ZERO,
        late: 0,
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        rate_limit: 0,
        ..*config
    }
}

/// The forge, met directly at `now`, as `user`.
#[derive(Debug)]
pub struct Direct<'a> {
    pub domain: &'a mut Domain,
    /// The forge's configuration for direct calls, and the world's clock.
    pub env: Env<Config>,
    pub user: u64,
    /// What the forge emitted for others meanwhile.
    pub stray: &'a mut Vec<Request>,
    /// The direct calls made so far, the last naming the latest.
    pub calls: &'a mut u64,
    /// Whether io reaches the forge: if not, every call fails as unreachable.
    pub reachable: bool,
}

impl Direct<'_> {
    /// Makes `op` on `repository`, answered at once.
    ///
    /// # Errors
    ///
    /// As the forge answers it.
    pub fn call(&mut self, repository: &[u8], op: Op) -> Result<Answer, Error> {
        *self.calls += 1;
        let name = Token::new(DIRECT | *self.calls);
        let mut out = Queue::with_capacity(MAX_OUT);
        let event = Event::Call { reply_to: ReplyTo::new(name), user: self.user, repository: repository.into(), op };
        forge::step(self.domain, &self.env, event, &mut out);
        for _ in 0..FIRES {
            while let Some(request) = out.pop() {
                match request {
                    Request::Reply { to, result } => {
                        let to = to.into_token();
                        if to == name {
                            self.stray.extend(drain(&mut out));
                            return result;
                        }
                        self.stray.push(Request::Reply { to: ReplyTo::new(to), result });
                    }
                    request @ Request::Hook { .. } => self.stray.push(request),
                }
            }
            assert!(self.domain.is_due(self.env.now), "a direct call is answered at once");
            forge::fire(self.domain, &self.env, &mut out);
        }
        panic!("a direct call is answered within {FIRES} fires");
    }

    fn git(&mut self, remote: &[u8], op: Git) -> Result<Answer, Fault> {
        if !self.reachable {
            return Err(Fault::Unreachable);
        }
        self.call(remote, Op::Git(op)).map_err(fault)
    }

    /// A person hands `hand` in on the forge: they open its issue, the
    /// engine's record goes on it as an earlier life of the engine wrote it,
    /// then the tracking label. Returns the issue's number.
    pub fn hand_in(&mut self, hand: &Hand, person: u64) -> u64 {
        let repository = temper_legacy_engine_domain_world::deployment::name(hand.repository);
        self.user = person;
        let title = desk::title(hand).into_boxed_slice();
        let body = desk::guidance(hand.job).into_boxed_slice();
        let op = Op::Write(Write::CreateIssue { title, body, labels: Box::new([]) });
        let Ok(Answer::Created(number)) = self.call(repository, op) else {
            panic!("the forge has room for every issue handed in");
        };
        self.user = ENGINE;
        let record = codec::record_block(&desk::record(hand, self.env.now));
        let body = recorded(Position::START, 0, &record).into_boxed_slice();
        let made = self.call(repository, Op::Write(Write::Comment { number, body }));
        assert!(made.is_ok(), "the forge takes the record of every issue handed in: {made:?}");
        let labels = Box::new([TRACKING.into()]);
        let made = self.call(repository, Op::Write(Write::AddLabels { number, labels }));
        assert!(made.is_ok(), "the forge takes the tracking label of every issue handed in: {made:?}");
        number
    }

    /// Another party moves `branch` of `remote`, making it first at the
    /// default branch if it is nowhere, writing `path` with `content`.
    /// Returns whether it moved.
    pub fn advance(&mut self, remote: &[u8], branch: &[u8], path: &[u8], content: &[u8]) -> bool {
        self.user = OTHER;
        if self.domain.branch(remote, branch).is_none() {
            let base = self.domain.branch(remote, MAIN).expect("every repository has its default branch");
            let op = Git::Create { branch: branch.into(), commit: base };
            if self.call(remote, Op::Git(op)).is_err() {
                return false;
            }
        }
        let env = Env { now: self.env.now, wall: self.env.wall, limits: self.env.limits };
        forge::advance(self.domain, &env, remote, branch, path, content, OTHER).is_ok()
    }
}

/// What is left in `out`.
fn drain(out: &mut Queue<Request>) -> Vec<Request> {
    let mut left = Vec::new();
    while let Some(request) = out.pop() {
        left.push(request);
    }
    left
}

impl Remote for Direct<'_> {
    fn heads(&mut self, remote: &[u8]) -> Result<Vec<u64>, Fault> {
        let Answer::Cloned { default: _, branches } = self.git(remote, Git::Clone)? else {
            panic!("a clone is answered with the branches");
        };
        Ok(branches.iter().map(|head| head.commit).collect())
    }

    fn fetch(&mut self, remote: &[u8], want: Want<'_>) -> Result<u64, Fault> {
        let want = match want {
            Want::Branch(branch) => temper_fake_forge_domain::api::Want::Branch(branch.into()),
            Want::Commit(commit) => temper_fake_forge_domain::api::Want::Commit(commit),
            Want::Default => temper_fake_forge_domain::api::Want::Default,
        };
        let Answer::Commit(commit) = self.git(remote, Git::Fetch { want })? else {
            panic!("a fetch is answered with the commit fetched");
        };
        Ok(commit)
    }

    fn create(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault> {
        let Answer::Branch(created) = self.git(remote, Git::Create { branch: branch.into(), commit })? else {
            panic!("a branch's creation is answered with how it went");
        };
        Ok(match created {
            temper_fake_forge_domain::api::Created::Created => Created::Created,
            temper_fake_forge_domain::api::Created::Exists => Created::Exists,
        })
    }

    fn push(&mut self, remote: &[u8], branch: &[u8], commit: u64, expected: Option<u64>) -> Result<Pushed, Fault> {
        let Answer::Pushed(pushed) = self.git(remote, Git::Push { branch: branch.into(), commit, expected })? else {
            panic!("a push is answered with how it went");
        };
        Ok(match pushed {
            temper_fake_forge_domain::api::Pushed::Pushed => Pushed::Pushed,
            temper_fake_forge_domain::api::Pushed::Rejected => Pushed::Rejected,
        })
    }

    fn parent(&self, commit: u64) -> Option<u64> {
        self.domain.object(commit).expect("a commit of the store").parent
    }

    fn merge_parent(&self, commit: u64) -> Option<u64> {
        self.domain.object(commit).expect("a commit of the store").merge_parent
    }

    fn tree(&self, commit: u64) -> Tree {
        tree(self.domain, commit)
    }

    fn store(&mut self, parent: u64, merging: Option<u64>, tree: Tree) -> Option<u64> {
        let files = tree.into_iter().map(|(path, content)| File { path: path.into(), content: content.into() });
        match merging {
            Some(second) => Some(
                forge::merge_commit(self.domain, &self.env.limits, parent, second, files.collect())
                    .expect("the forge's store has room for every merge"),
            ),
            None => forge::commit(self.domain, &self.env.limits, parent, files.collect())
                .expect("the forge's store has room for every commit"),
        }
    }
}

/// The files of `commit`.
#[must_use]
pub fn tree(domain: &Domain, commit: u64) -> Tree {
    let object = domain.object(commit).expect("a commit of the store");
    object.tree.iter().map(|(path, content)| (path.to_vec(), content.to_vec())).collect()
}

/// Where `branch` of `repository` is, if the forge has both.
#[must_use]
pub fn branch(domain: &Domain, config: &Config, repository: &[u8], branch: &[u8]) -> Option<u64> {
    match domain.inspect(config, repository, &Read::Branch { branch: branch.into() }) {
        Ok(Answer::Commit(commit)) => Some(commit),
        Ok(answer) => panic!("a branch is read as where it is: {answer:?}"),
        Err(_) => None,
    }
}

/// The world's clock, as the forge's direct calls take it.
#[must_use]
pub fn env(config: &Config, now: Time) -> Env<Config> {
    Env { now, wall: Wall::EPOCH, limits: direct(config) }
}

/// git's fault for what the forge refused: only what the world scripts, or
/// what a spec names that the forge does not have.
fn fault(error: Error) -> Fault {
    match error {
        Error::Missing(What::Repository) => Fault::Missing(git::What::Repository),
        Error::Missing(What::Branch) => Fault::Missing(git::What::Branch),
        Error::Missing(What::Commit) => Fault::Missing(git::What::Commit),
        Error::Unreachable => Fault::Unreachable,
        Error::Refused => Fault::Refused,
        Error::Missing(
            What::Item | What::Pull | What::Comment | What::Label | What::File | What::Page | What::Review | What::Job,
        )
        | Error::Unavailable
        | Error::Timeout
        | Error::RateLimited { .. }
        | Error::Forbidden
        | Error::TooLarge
        | Error::Full
        | Error::Exists
        | Error::Circular
        | Error::NothingToMerge
        | Error::Empty
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected => panic!("the forge fails git only as the world scripts it: {error:?}"),
    }
}
