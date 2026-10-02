//! io's git and files, on the fake forge and disk, through the checkout
//! world's translation of the checkout's operations: each after a latency,
//! racing its deadline and the cancel of an aborted prepare, with the faults
//! the world scripts; and what moves on the forge, and what lands, checked as
//! it does.

use temper_checkout_fake::git::{Forge, Move, Tree as Files};
use temper_checkout_fake::{Checkout, in_git};
use temper_fake_engine_model::{BASE, IDENTITY};
use temper_lib::{Rng, Time, Token};
use temper_worker_model::Event;
use temper_worker_model::checkout::git::{Done, Fault, Kind, Op, Place, Want};
use temper_worker_model_checkout_tests::translate as io;
use temper_world::{Key, Span};

use super::{Delivery, PUSH_PREFIX, Settings, Space, World, gone};

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
    /// its repository unreachable, and that a push or a branch's creation is
    /// refused.
    pub unreachable: u32,
    pub refusing: u32,
    /// The chance, per mille, that a cancel loses its race: the operation ends
    /// of itself, and that is its terminal event.
    pub cancels_lost: u32,
    /// The chance, per mille, that another party moves a push branch of a run
    /// that has started, a drawn `advance_after` later.
    pub advance: u32,
    pub advance_after: Span,
    /// The chance, per mille, that a repository's default branch is not the
    /// base branch, which a run then creates; and that a workstream's branch
    /// exists in a repository from the start, which a run may start from.
    pub trunks: u32,
    pub branched: u32,
}

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
        let delivery = self.wire.send(ends, Delivery::Ran { owner });
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
        self.wire.withdraw(key).expect("an operation in flight has its end on the way");
        let delivery = self.wire.send(at, Delivery::Ran { owner });
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
    /// what moved on the forge and what landed.
    fn perform(&mut self, owner: Token, op: Op) -> Done {
        let workspace = io::workspace(&op);
        let remote = io::remote(&op).map(<[u8]>::to_vec);
        let writes = op.kind() == Kind::Create || op.kind() == Kind::Push;
        if let Some(remote) = &remote {
            let reachable = !self.rng.chance(self.settings.git.unreachable);
            let refusing = writes && self.rng.chance(self.settings.git.refusing);
            self.forge.set_reachable(remote, reachable);
            self.forge.set_refusing(remote, refusing);
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
        let moves = self.forge.moves().len();
        let done = io::perform(&mut self.forge, &mut self.disk, op);
        if let Some(remote) = &remote {
            self.forge.set_reachable(remote, true);
            self.forge.set_refusing(remote, false);
        }
        for Move { remote, branch, from, to } in &self.forge.moves()[moves..] {
            if let Some(from) = from {
                assert!(
                    self.forge.is_ancestor(*from, *to),
                    "{}: {} moved only by a fast-forward",
                    String::from_utf8_lossy(remote),
                    String::from_utf8_lossy(branch)
                );
            }
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
            let tree = &self.forge.object(commit).tree;
            if self.save_branches.contains(&branch) {
                let left = space.left.get(&repository).expect("a save is of a repository checked out");
                assert_eq!(tree, left, "saved work is exactly the tree its agent left");
                self.saves.insert((remote, branch), commit);
                self.stats.saved += 1;
            } else {
                let asked = space.asked.get(&repository).expect("a push lands what its agent asked to push");
                assert_eq!(tree, asked, "what lands is exactly the tree its agent left when it asked to push");
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
        self.forge.advance(remote, branch, b"OTHER", content.as_bytes());
    }
}

/// What a repository of `workspace` holds, less its git directory.
pub(super) fn files(disk: &Checkout, workspace: Token, repository: &[u8]) -> Files {
    let at = Place { workspace, repository: repository.into() };
    let mut files = disk.tree(&io::path(&at));
    files.retain(|path, _| !in_git(path));
    files
}

/// The forge, seeded from the engine's names: each repository the engine
/// draws from, its default branch the base branch or not, and each
/// workstream's branch in it or not. Returns it, and the commit the engine's
/// one hash stands for: the first repository's first.
pub(super) fn seed_forge(rng: &mut Rng, settings: &Settings) -> (Forge, u64) {
    let mut forge = Forge::new();
    let mut first = None;
    for origin in &settings.engine.repositories {
        let default: &[u8] = if rng.chance(settings.git.trunks) { b"trunk" } else { BASE };
        let files =
            Files::from([(b"README".to_vec(), origin.name.to_vec()), (b"src/lib.rs".to_vec(), b"fn f() {}".to_vec())]);
        let commit = forge.repository(&origin.remote, default, files);
        first.get_or_insert(commit);
        for key in &settings.engine.workstreams {
            if rng.chance(settings.git.branched) {
                let branch = [PUSH_PREFIX, key].concat();
                forge.create(&origin.remote, &branch, commit).expect("a branch is created on a forge that works");
            }
        }
    }
    (forge, first.expect("the engine draws from a repository"))
}
