//! The fake forge (`temper_fake_forge_domain`) as io's git meets it, across the
//! network (testing.md, 4.2 and 4.3): the transport a working tree's
//! git reaches its remotes through ([`Remote`]), routed to the forge's domain
//! as the worker's calls, and the forge's one store, where git names its
//! commits and finds their trees. And what a world does to the forge from
//! outside: its repositories, the faults it scripts on them, and another
//! party moving a branch.
//!
//! A call is routed directly: made, and answered in the same instant. io's
//! latency for an operation that reaches the forge already stands for all of
//! it, the network and the forge included, racing its deadline and any
//! cancel; a latency of the forge's own would count it twice. So the forge
//! answers with no latency, faults or rate limit of its own, and the world's
//! faults are the ones it scripts: a repository unreachable, or refusing what
//! is pushed to it.
//!
//! What moved on the forge is what it observed: every move of a branch,
//! drained by the world as [`Move`]s.

use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use temper_fake_checkout::git::{self, Created, Fault, Pushed, Remote, Tree, Want};
use temper_fake_forge_domain::api::{Answer, Checks, Error, File, Git, Op, Permission, Read, Setup, What};
use temper_fake_forge_domain::{Config, Domain, Event, Limits, MAX_OUT, Observation, Request, Skew};

/// The forge's users: the worker, whose identity every operation of io acts
/// as; another party, who moves branches under it; and CI, which reports
/// nothing here.
const WORKER: u64 = 1;
const OTHER: u64 = 2;
const CI: u64 = 3;

/// Room for what the worlds put on the forge: a few repositories, the
/// branches of their workstreams, and every commit their runs make. Nothing
/// fills: a forge that refused for want of room would fail the world.
pub const LIMITS: Limits = Limits {
    repositories: 16,
    users: 4,
    labels: 1,
    items: 1,
    comments: 1,
    dependencies: 1,
    reviews: 1,
    branches: 64,
    commits: 4096,
    files: 256,
    statuses: 1,
    contexts: 1,
    pages: 1,
    name_bytes: 256,
    title_bytes: 16,
    body_bytes: 16,
    content_bytes: 65_536,
    page_size: 1,
    calls: 1,
    hooks: 1,
    observations: 16,
};

/// A forge that answers at once and fails only as a world scripts it.
pub const CONFIG: Config = Config {
    limits: LIMITS,
    latency_min: Duration::ZERO,
    latency_max: Duration::ZERO,
    late: 0,
    late_min: Duration::ZERO,
    late_max: Duration::ZERO,
    unavailable: 0,
    timeouts: 0,
    landing: 0,
    land_min: Duration::ZERO,
    land_max: Duration::ZERO,
    rate_limit: 0,
    rate_window: Duration::ZERO,
    ci: CI,
    hook_min: Duration::ZERO,
    hook_max: Duration::ZERO,
    hooks_late: 0,
    hooks_lost: 0,
    resolution: Duration::ZERO,
    skew: Skew::None,
    status_updates: false,
    edit_updates: false,
};

/// A branch moved on the forge: created if it was nowhere.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Move {
    pub remote: Vec<u8>,
    pub branch: Vec<u8>,
    pub from: Option<u64>,
    pub to: u64,
}

#[derive(Debug)]
pub struct Forge {
    domain: Domain,
    /// The world's clock, as of its last say.
    now: Time,
    /// The calls made, the last naming the latest.
    calls: u64,
}

impl Forge {
    /// An empty forge, drawing from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Forge {
        Forge { domain: Domain::new(&CONFIG, seed), now: Time::ZERO, calls: 0 }
    }

    /// Tells the forge the world's time, for the calls that follow.
    pub fn at(&mut self, now: Time) {
        assert!(now >= self.now, "the world's clock moves forward");
        self.now = now;
    }

    // What a world does to the forge from outside.

    /// Adds the repository at `remote`, whose `default` branch is at a first
    /// commit of `tree`, where the worker and another party may push, and
    /// returns that commit.
    pub fn repository(&mut self, remote: &[u8], default: &[u8], tree: Tree) -> u64 {
        let setup = Setup {
            name: remote.into(),
            default: default.into(),
            tree: files(tree),
            labels: Box::new([]),
            checks: Checks {
                contexts: Box::new([]),
                latency_min: Duration::ZERO,
                latency_max: Duration::ZERO,
                silent: 0,
                passes: 0,
                reruns: 0,
                cue: None,
            },
            protection: None,
            hooked: false,
        };
        let first = temper_fake_forge_domain::repository(&mut self.domain, &CONFIG, setup);
        temper_fake_forge_domain::grant(&mut self.domain, remote, WORKER, Permission::Write);
        temper_fake_forge_domain::grant(&mut self.domain, remote, OTHER, Permission::Write);
        first
    }

    /// Makes the repository at `remote` reachable, or not.
    pub fn set_reachable(&mut self, remote: &[u8], reachable: bool) {
        temper_fake_forge_domain::set_reachable(&mut self.domain, remote, reachable);
    }

    /// Makes the repository at `remote` refuse what is pushed to it, branches
    /// created included, or not.
    pub fn set_refusing(&mut self, remote: &[u8], refusing: bool) {
        temper_fake_forge_domain::set_refusing(&mut self.domain, remote, refusing);
    }

    /// Another party commits on `branch` of the repository at `remote`,
    /// writing `path` with `content`, and moves the branch to it, as a push of
    /// its own would. Returns the commit.
    pub fn advance(&mut self, remote: &[u8], branch: &[u8], path: &[u8], content: &[u8]) -> u64 {
        let env = Env { now: self.now, wall: Wall::EPOCH, limits: CONFIG };
        let advanced = temper_fake_forge_domain::advance(&mut self.domain, &env, remote, branch, path, content, OTHER);
        advanced.expect("the forge has room for another party's commit")
    }

    /// Another party creates `branch` of the repository at `remote` at
    /// `commit`, only if it is nowhere, as git's push of a new branch would.
    ///
    /// # Errors
    ///
    /// As the worker's would: the repository is missing, cannot be reached,
    /// or refuses it.
    pub fn create_branch(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault> {
        let op = Git::Create { branch: branch.into(), commit };
        let Answer::Branch(created) = self.call(remote, OTHER, op)? else {
            panic!("a branch's creation is answered with how it went");
        };
        Ok(created_of(created))
    }

    // What a world reads of the forge.

    /// Where `branch` of the repository at `remote` is, if the forge has both.
    #[must_use]
    pub fn branch(&self, remote: &[u8], branch: &[u8]) -> Option<u64> {
        let read = Read::Branch { branch: branch.into() };
        match self.domain.inspect(&CONFIG, remote, &read) {
            Ok(Answer::Commit(commit)) => Some(commit),
            Ok(answer) => panic!("a branch is read as where it is: {answer:?}"),
            Err(_) => None,
        }
    }

    /// The branches of the repository at `remote`, in their order.
    #[must_use]
    pub fn branches(&self, remote: &[u8]) -> Vec<Vec<u8>> {
        self.domain.branches(remote).iter().map(|(branch, _)| branch.to_vec()).collect()
    }

    /// Whether `ancestor` is `commit` or one of its ancestors.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: u64, commit: u64) -> bool {
        self.domain.is_ancestor(ancestor, commit)
    }

    /// The branches moved since the world last asked, in order, from the
    /// forge's observations: the worker's pushes and creations, and another
    /// party's. Refused calls and rejected pushes moved nothing.
    pub fn moves(&mut self) -> Vec<Move> {
        let mut moves = Vec::new();
        while let Some(observation) = self.domain.pop_observation() {
            match observation {
                Observation::Moved { repository, branch, from, to, by: _ } => {
                    moves.push(Move { remote: repository.into(), branch: branch.into(), from, to });
                }
                Observation::Refused { .. } | Observation::Rejected { .. } => {}
                Observation::Deleted { .. }
                | Observation::Opened { .. }
                | Observation::Closed { .. }
                | Observation::Reopened { .. }
                | Observation::Labelled { .. }
                | Observation::Revised { .. }
                | Observation::Depends { .. }
                | Observation::Requested { .. }
                | Observation::Defined { .. }
                | Observation::Commented { .. }
                | Observation::Edited { .. }
                | Observation::Removed { .. }
                | Observation::Reviewed { .. }
                | Observation::Reported { .. }
                | Observation::Merged { .. }
                | Observation::Wiki { .. } => panic!("only git reaches the forge here: {observation:?}"),
            }
        }
        assert_eq!(self.domain.observations_lost(), 0, "the world drains the forge's observations as they come");
        moves
    }

    /// Makes `op` on the repository at `remote` as `user`, routed directly:
    /// the forge answers at once.
    fn call(&mut self, remote: &[u8], user: u64, op: Git) -> Result<Answer, Fault> {
        self.calls += 1;
        let env = Env { now: self.now, wall: Wall::EPOCH, limits: CONFIG };
        let mut out = Queue::with_capacity(MAX_OUT);
        let reply_to = ReplyTo::new(Token::new(self.calls));
        let event = Event::Call { reply_to, user, repository: remote.into(), op: Op::Git(op) };
        temper_fake_forge_domain::step(&mut self.domain, &env, event, &mut out);
        temper_fake_forge_domain::fire(&mut self.domain, &env, &mut out);
        self.domain.reclaim();
        let Some(Request::Reply { to, result }) = out.pop() else {
            panic!("the forge answers a call at once");
        };
        assert_eq!(to.into_token(), Token::new(self.calls), "the forge answers the call made");
        assert!(out.is_empty() && self.domain.calls() == 0, "nothing else is in flight");
        result.map_err(fault)
    }
}

impl Remote for Forge {
    fn heads(&mut self, remote: &[u8]) -> Result<Vec<u64>, Fault> {
        let Answer::Cloned { default: _, branches } = self.call(remote, WORKER, Git::Clone)? else {
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
        let Answer::Commit(commit) = self.call(remote, WORKER, Git::Fetch { want })? else {
            panic!("a fetch is answered with the commit fetched");
        };
        Ok(commit)
    }

    fn create(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, Fault> {
        let op = Git::Create { branch: branch.into(), commit };
        let Answer::Branch(created) = self.call(remote, WORKER, op)? else {
            panic!("a branch's creation is answered with how it went");
        };
        Ok(created_of(created))
    }

    fn push(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Pushed, Fault> {
        let op = Git::Push { branch: branch.into(), commit };
        let Answer::Pushed(pushed) = self.call(remote, WORKER, op)? else {
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

    fn tree(&self, commit: u64) -> Tree {
        let object = self.domain.object(commit).expect("a commit of the store");
        object.tree.iter().map(|(path, content)| (path.to_vec(), content.to_vec())).collect()
    }

    fn store(&mut self, parent: u64, tree: Tree) -> Option<u64> {
        let committed = temper_fake_forge_domain::commit(&mut self.domain, &CONFIG, parent, files(tree));
        committed.expect("the forge's store has room for every commit")
    }
}

fn files(tree: Tree) -> Box<[File]> {
    tree.into_iter().map(|(path, content)| File { path: path.into(), content: content.into() }).collect()
}

fn created_of(created: temper_fake_forge_domain::api::Created) -> Created {
    match created {
        temper_fake_forge_domain::api::Created::Created => Created::Created,
        temper_fake_forge_domain::api::Created::Exists => Created::Exists,
    }
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
            What::Item | What::Pull | What::Comment | What::Label | What::File | What::Page | What::Review,
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
