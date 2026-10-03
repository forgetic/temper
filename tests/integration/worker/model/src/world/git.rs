//! io's git and files, on the fake disk and the one fake forge the engine
//! reads and people use, through the checkout world's translation of the
//! checkout's operations and a route of the world's to the forge ([`Line`]):
//! each after a latency, racing its deadline and the cancel of an aborted
//! prepare, with the faults the world scripts; and what lands, checked as
//! it does.
//!
//! git's calls reach the forge as the worker's forge user, and are answered
//! at once: io's latency for an operation that reaches the forge stands for
//! all of it, the network and the forge included, so the forge adds no
//! latency of its own, nor faults but those the world scripts (a
//! repository unreachable, or refusing what is pushed to it). What a push
//! moves, the forge observes, and its webhooks go out, as for any change.

use temper_checkout_fake::git::{self as fake, Created, Pushed, Remote, Tree as Files};
use temper_checkout_fake::{Checkout, in_git};
use temper_engine_model_tests::deployment::WORKER;
use temper_forge_model::api::{Answer, Error, File, Git as Call, Op as ForgeOp, What, Write};
use temper_forge_model::{self as forge, Config};
use temper_lib::{Env, Queue, ReplyTo, Time, Token};
use temper_worker_model::Event;
use temper_worker_model::checkout::git::{Done, Fault, Kind, Op, Place, Want};
use temper_worker_model_checkout_tests::translate as io;
use temper_world::{Key, Span};

use super::{Delivery, Space, World, gone};
use crate::protocol::IDENTITY;

/// How io's git and files behave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Git {
    /// How long io takes over an operation on the disk, and over one that
    /// reaches the forge.
    pub local: Span,
    pub remote: Span,
    /// How long a cancel takes to reach io, and io to tell that a deadline
    /// passed or that a cancel won.
    pub network: Span,
    /// The chance, per mille, that an operation stalls: it takes a drawn
    /// `stall` instead, which may run past its deadline.
    pub stalls: u32,
    pub stall: Span,
    /// The chance, per mille, that io fails an operation on the worker's side,
    /// and that it carries one out and then reports that it ran out of time.
    pub broken: u32,
    pub ambiguous: u32,
    /// The chance, per mille, that an operation that reaches the forge finds
    /// its repository unreachable, and that a push, or a branch's creation,
    /// is refused.
    pub unreachable: u32,
    pub refusing: u32,
    pub refusing_creates: u32,
    /// The chance, per mille, that a cancel loses its race: the operation ends
    /// of itself, and that is its terminal event.
    pub cancels_lost: u32,
    /// The chance, per mille, that another party moves a push branch of a run
    /// that has started, a drawn `advance_after` later; and that it deletes
    /// the push branch of a run that has answered, so that its item's next
    /// attempt finds it missing.
    pub advance: u32,
    pub advance_after: Span,
    pub deletes: u32,
}

/// The forge user of another party, who moves branches under
/// the worker.
pub(super) const OTHER: u64 = 20;

/// A git operation in flight in io: its end on the way, and what it does then.
#[derive(Debug)]
pub(super) struct Pending {
    delivery: Key,
    workspace: Token,
    work: Work,
}

#[derive(Debug)]
pub(super) enum Work {
    /// It runs when it ends.
    Perform(Op),
    /// It runs when it ends, and io reports that it ran out of time, which
    /// leaves what it did in doubt.
    Ambiguous(Op),
    /// It ends so, having done nothing.
    Ending(Done),
}

impl World {
    /// Whether a git operation runs in `workspace`.
    pub(super) fn busy(&self, workspace: Token) -> bool {
        self.ops.values().any(|pending| pending.workspace == workspace)
    }

    pub(super) fn start_op(&mut self, owner: Token, op: Op, deadline: Time) {
        assert!(!self.closed.contains(&owner), "nothing of git runs for a run that has answered");
        let limits = self.settings.worker.checkout;
        let timeout = if op.is_remote() { limits.remote_timeout } else { limits.local_timeout };
        assert_eq!(deadline, self.now.saturating_add(timeout), "an operation's deadline is by where it runs");
        if let Some(identity) = io::identity(&op) {
            assert_eq!(identity, IDENTITY, "an operation acts as its repository's identity");
        }
        let workspace = io::workspace(&op);
        let space = self.spaces.entry(workspace).or_default();
        if let Some(hold) = space.hold
            && hold != owner
        {
            assert!(!self.ops.contains(hold), "a workspace is held by one hold at a time");
        }
        space.hold = Some(owner);
        if let Some(agent) = space.agent {
            let record = self.agents.get(&agent).expect("an agent is known until its run answers");
            if !gone(&self.tree, record) {
                let pushing = !record.pushes.is_empty();
                let part = match &op {
                    Op::Commit { .. } | Op::Fetch { want: Want::Branch { .. }, .. } => pushing,
                    Op::Push { branch, .. } => pushing && !self.save_branches.contains(&**branch),
                    Op::Make { .. } | Op::Clone { .. } | Op::Fetch { .. } | Op::Create { .. } | Op::CheckOut { .. } => {
                        false
                    }
                };
                assert!(
                    part,
                    "no git operation touches a workspace while its agent may run, but the push it asked for: {op:?}"
                );
            }
        }
        let git = self.settings.git;
        let span = if self.rng.chance(git.stalls) {
            git.stall
        } else if op.is_remote() {
            git.remote
        } else {
            git.local
        };
        let mut ends = self.now.saturating_add(span.draw(&mut self.rng));
        let mut work = if self.rng.chance(git.broken) {
            self.stats.op_broken += 1;
            Work::Ending(Done::Failed { fault: Fault::Broken })
        } else if self.rng.chance(git.ambiguous) {
            self.stats.ambiguous += 1;
            Work::Ambiguous(op)
        } else {
            Work::Perform(op)
        };
        // io runs the race with the deadline, and tells it lost a moment
        // after the deadline passes, having done nothing.
        if ends > deadline {
            work = Work::Ending(Done::Failed { fault: Fault::TimedOut });
            ends = deadline.saturating_add(git.network.draw(&mut self.rng));
            self.stats.op_timeouts += 1;
        }
        let delivery = self.send(ends, Delivery::Ran { owner });
        self.ops.open(owner, Pending { delivery, workspace, work });
        self.stats.ops += 1;
    }

    /// io is asked to cancel the operation of `owner`: it ends cancelled after
    /// a while, having done nothing, unless it ends of itself first.
    pub(super) fn cancel_op(&mut self, owner: Token) {
        let Some(pending) = self.ops.get(owner) else {
            // It ended in the iteration the cancel was sent.
            return;
        };
        let prepares = match &pending.work {
            Work::Perform(op) | Work::Ambiguous(op) => match op {
                Op::Make { .. } | Op::Clone { .. } | Op::Fetch { .. } | Op::Create { .. } | Op::CheckOut { .. } => true,
                Op::Commit { .. } | Op::Push { .. } => false,
            },
            Work::Ending(_) => true,
        };
        assert!(prepares, "only a prepare is cancelled: a push or a save runs to its end");
        if self.rng.chance(self.settings.git.cancels_lost) {
            self.stats.cancels_lost += 1;
            return;
        }
        let key = pending.delivery;
        let at = self.now.saturating_add(self.settings.git.network.draw(&mut self.rng));
        self.withdraw(key).expect("an operation in flight has its end on the way");
        let delivery = self.send(at, Delivery::Ran { owner });
        let pending = self.ops.get_mut(owner).expect("looked up above");
        pending.delivery = delivery;
        pending.work = Work::Ending(Done::Failed { fault: Fault::Cancelled });
        self.stats.op_cancels += 1;
    }

    /// The operation of `owner` ends, and io tells the checkout how.
    pub(super) fn ran(&mut self, owner: Token) {
        let pending = self.ops.end(owner);
        let done = match pending.work {
            Work::Perform(op) => self.perform(owner, op),
            Work::Ambiguous(op) => {
                self.perform(owner, op);
                Done::Failed { fault: Fault::TimedOut }
            }
            Work::Ending(done) => done,
        };
        if !self.done {
            self.stage.push(Event::Done { owner, done });
        }
    }

    /// Runs `op` on the fakes, with the faults the world scripts, and checks
    /// what landed.
    fn perform(&mut self, owner: Token, op: Op) -> Done {
        let workspace = io::workspace(&op);
        let remote = io::remote(&op).map(<[u8]>::to_vec);
        if let Some(remote) = &remote {
            let reachable = !self.rng.chance(self.settings.git.unreachable);
            let refusing = match op.kind() {
                Kind::Create => self.rng.chance(self.settings.git.refusing_creates),
                Kind::Push => self.rng.chance(self.settings.git.refusing),
                Kind::Make | Kind::Clone | Kind::Fetch | Kind::CheckOut | Kind::Commit => false,
            };
            forge::set_reachable(&mut self.forge, remote, reachable);
            forge::set_refusing(&mut self.forge, remote, refusing);
        }
        let made = op.kind() == Kind::Make;
        let checked = match &op {
            Op::CheckOut { at, commit } => Some((at.repository.to_vec(), io::fake(*commit))),
            Op::Make { .. }
            | Op::Clone { .. }
            | Op::Fetch { .. }
            | Op::Create { .. }
            | Op::Commit { .. }
            | Op::Push { .. } => None,
        };
        let pushed = match &op {
            Op::Push { at, remote, commit, branch, .. } => {
                Some((at.repository.to_vec(), remote.to_vec(), io::fake(*commit), branch.to_vec()))
            }
            Op::Make { .. }
            | Op::Clone { .. }
            | Op::Fetch { .. }
            | Op::Create { .. }
            | Op::CheckOut { .. }
            | Op::Commit { .. } => None,
        };
        let mut spill = Vec::new();
        let mut line = Line {
            forge: &mut self.forge,
            env: Env { now: self.now, limits: self.settings.forge },
            spill: &mut spill,
            calls: &mut self.git_calls,
        };
        let done = io::perform(&mut line, &mut self.disk, op);
        if let Some(remote) = &remote {
            forge::set_reachable(&mut self.forge, remote, true);
            forge::set_refusing(&mut self.forge, remote, false);
        }
        for request in spill {
            self.forge_out.push(request);
        }
        let space = self.spaces.entry(workspace).or_default();
        if made {
            *space = Space { hold: Some(owner), agent: space.agent, ..Space::default() };
        }
        if let Some((repository, commit)) = checked {
            space.checked.insert(repository.clone(), commit);
            space.left.insert(repository.clone(), files(&self.disk, workspace, &repository));
            space.asked.remove(&repository);
        }
        if let Some((repository, remote, commit, branch)) = pushed
            && done == Done::Succeeded
        {
            let tree = &tree(&self.forge, commit);
            if self.save_branches.contains(&branch) {
                let left = space.left.get(&repository).expect("a save is of a repository checked out");
                assert_eq!(tree, left, "saved work is exactly the tree its agent left");
                self.saves.insert((remote, branch), commit);
                self.stats.saved += 1;
            } else {
                let asked = space.asked.get(&repository).expect("a push lands what its agent asked to push");
                assert_eq!(tree, asked, "what lands is exactly the tree the agent left when it asked to push");
                self.stats.landed += 1;
            }
        }
        match done {
            Done::Failed { fault: Fault::Unreachable } => self.stats.unreachable += 1,
            Done::Failed { fault: Fault::Refused } => self.stats.refusals += 1,
            Done::Rejected => self.stats.rejected += 1,
            Done::Succeeded
            | Done::Fetched { .. }
            | Done::Committed { .. }
            | Done::Unchanged
            | Done::Exists
            | Done::Failed { .. } => {}
        }
        done
    }

    /// Another party moves `branch` of `remote`, if it is anywhere.
    pub(super) fn advance(&mut self, remote: &[u8], branch: &[u8]) {
        if self.forge.branch(remote, branch).is_none() {
            return;
        }
        self.stats.advanced += 1;
        let content = format!("another party, {}", self.stats.advanced);
        let env = Env { now: self.now, limits: self.settings.forge };
        let advanced = forge::advance(&mut self.forge, &env, remote, branch, b"OTHER", content.as_bytes(), OTHER);
        advanced.expect("the forge has room for another party's commit");
    }
}

impl World {
    /// Another party deletes `branch` of `remote`, if it is there: the open
    /// pull requests from it close.
    pub(super) fn delete(&mut self, remote: &[u8], branch: &[u8]) {
        if self.forge.branch(remote, branch).is_none() {
            return;
        }
        self.stats.deleted += 1;
        let name = self.wire.name();
        self.theirs.open(name, super::Theirs::Person { tale: None });
        let op = ForgeOp::Write(Write::DeleteBranch { branch: branch.into() });
        let reply_to = ReplyTo::new(Token::new(name));
        self.forge_call(forge::Event::Call { reply_to, user: OTHER, repository: remote.into(), op });
    }
}

/// The files of `commit` in the forge's store.
pub(super) fn tree(forge: &forge::Model, commit: u64) -> Files {
    let object = forge.object(commit).expect("a commit of the store");
    object.tree.iter().map(|(path, content)| (path.to_vec(), content.to_vec())).collect()
}

/// git's route to the forge: each call made as the worker's forge user and
/// answered at once, with no latency or fault of the forge's own; and what
/// else the forge had due meanwhile, spilled for the world to route.
pub(super) struct Line<'a> {
    pub(super) forge: &'a mut forge::Model,
    pub(super) env: Env<Config>,
    pub(super) spill: &'a mut Vec<forge::Request>,
    /// git's calls so far, the last naming the latest.
    pub(super) calls: &'a mut u64,
}

impl Line<'_> {
    fn call(&mut self, remote: &[u8], user: u64, op: ForgeOp) -> Result<Answer, Error> {
        *self.calls += 1;
        // The world's names for its own calls are below; git's above.
        let name = Token::new(u64::MAX - *self.calls);
        let direct = Config {
            latency_min: temper_lib::Duration::ZERO,
            latency_max: temper_lib::Duration::ZERO,
            late: 0,
            unavailable: 0,
            timeouts: 0,
            landing: 0,
            rate_limit: 0,
            ..self.env.limits
        };
        let mut out = Queue::with_capacity(forge::MAX_OUT);
        let event = forge::Event::Call { reply_to: ReplyTo::new(name), user, repository: remote.into(), op };
        forge::step(self.forge, &Env { now: self.env.now, limits: direct }, event, &mut out);
        let mut answered = None;
        while answered.is_none() {
            while let Some(request) = out.pop() {
                match request {
                    forge::Request::Reply { to, result } => {
                        let to = to.into_token();
                        if to == name {
                            answered = Some(result);
                        } else {
                            self.spill.push(forge::Request::Reply { to: ReplyTo::new(to), result });
                        }
                    }
                    forge::Request::Hook { .. } => self.spill.push(request),
                }
            }
            if answered.is_none() {
                assert!(self.forge.is_due(self.env.now), "the forge answers git at once");
                forge::fire(self.forge, &self.env, &mut out);
            }
        }
        answered.expect("answered above")
    }

    fn git(&mut self, remote: &[u8], op: Call) -> Result<Answer, fake::Fault> {
        self.call(remote, WORKER, ForgeOp::Git(op)).map_err(fault)
    }
}

impl Remote for Line<'_> {
    fn heads(&mut self, remote: &[u8]) -> Result<Vec<u64>, fake::Fault> {
        let Answer::Cloned { default: _, branches } = self.git(remote, Call::Clone)? else {
            panic!("a clone is answered with the branches");
        };
        Ok(branches.iter().map(|head| head.commit).collect())
    }

    fn fetch(&mut self, remote: &[u8], want: fake::Want<'_>) -> Result<u64, fake::Fault> {
        let want = match want {
            fake::Want::Branch(branch) => temper_forge_model::api::Want::Branch(branch.into()),
            fake::Want::Commit(commit) => temper_forge_model::api::Want::Commit(commit),
            fake::Want::Default => temper_forge_model::api::Want::Default,
        };
        let Answer::Commit(commit) = self.git(remote, Call::Fetch { want })? else {
            panic!("a fetch is answered with the commit fetched");
        };
        Ok(commit)
    }

    fn create(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Created, fake::Fault> {
        let Answer::Branch(created) = self.git(remote, Call::Create { branch: branch.into(), commit })? else {
            panic!("a branch's creation is answered with how it went");
        };
        Ok(match created {
            temper_forge_model::api::Created::Created => Created::Created,
            temper_forge_model::api::Created::Exists => Created::Exists,
        })
    }

    fn push(&mut self, remote: &[u8], branch: &[u8], commit: u64) -> Result<Pushed, fake::Fault> {
        let Answer::Pushed(pushed) = self.git(remote, Call::Push { branch: branch.into(), commit })? else {
            panic!("a push is answered with how it went");
        };
        Ok(match pushed {
            temper_forge_model::api::Pushed::Pushed => Pushed::Pushed,
            temper_forge_model::api::Pushed::Rejected => Pushed::Rejected,
        })
    }

    fn parent(&self, commit: u64) -> Option<u64> {
        self.forge.object(commit).expect("a commit of the store").parent
    }

    fn tree(&self, commit: u64) -> Files {
        tree(self.forge, commit)
    }

    fn store(&mut self, parent: u64, tree: Files) -> Option<u64> {
        let files = tree.into_iter().map(|(path, content)| File { path: path.into(), content: content.into() });
        let committed = forge::commit(self.forge, &self.env.limits, parent, files.collect());
        committed.expect("the forge's store has room for every commit")
    }
}

/// git's fault for what the forge refused: only what the world scripts, or
/// what a start names that the forge does not have.
fn fault(error: Error) -> fake::Fault {
    match error {
        Error::Missing(What::Repository) => fake::Fault::Missing(fake::What::Repository),
        Error::Missing(What::Branch) => fake::Fault::Missing(fake::What::Branch),
        Error::Missing(What::Commit) => fake::Fault::Missing(fake::What::Commit),
        Error::Unreachable => fake::Fault::Unreachable,
        Error::Refused => fake::Fault::Refused,
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

/// What a repository of `workspace` holds, less its git directory.
pub(super) fn files(disk: &Checkout, workspace: Token, repository: &[u8]) -> Files {
    let at = Place { workspace, repository: repository.into() };
    let mut files = disk.tree(&io::path(&at));
    files.retain(|path, _| !in_git(path));
    files
}
