//! What the world's scenarios expect of the worker and the agent together,
//! held by a referee (testing-pyramid.md, 5.2) that sees only what the fakes
//! see: the engine's assignments and the answers it hears, what comes and
//! goes through each agent process's pipes and signals, the disk under the
//! agents' tools, and the forge's branches. Safety, on every observation:
//!
//! - the worker commits exactly the tree its agent left: the tree it asked
//!   to push while it runs, the tree it left once it has gone;
//! - the engine records what the run said, if it said anything before its
//!   agent went: the outcome it accepted, or how it failed, a cancel being
//!   the worker's to report; a run ends, and lands a change, only once its
//!   agent started;
//! - what landed is on the forge: on its branch, exactly the tree its agent
//!   left when it asked to push.
//!
//! And liveness: every assignment is answered within a bound the world
//! sets.

use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model::run::{self, outcome::Declared};
use temper_checkout_fake::git::Tree as Files;
use temper_lib::{Duration, Token};
use temper_worker_model::host;
use temper_world::{Expectations, Judge};

use crate::channel;

/// What the referee observes.
#[derive(Debug)]
pub enum Seen {
    /// The engine assigns `attempt`, in a workspace of `repositories`.
    Assigned { attempt: Token, repositories: Vec<Repository> },
    /// The start of `attempt` comes down the pipe of the agent process
    /// `process`.
    Started { process: u64, attempt: Token },
    /// The agent of `process` asks to push: what each repository it may write
    /// holds on the disk now.
    Asked { process: u64, trees: Vec<(Vec<u8>, Files)> },
    /// The run of `process` answers, up its pipe.
    Answered { process: u64, answer: run::Answer },
    /// io signals `process` before the worker has read how its run finishes.
    Stopped { process: u64 },
    /// The agent of `process` has gone, of itself or killed: what each of its
    /// repositories held on the disk then.
    Gone { process: u64, left: BTreeMap<Vec<u8>, Files> },
    /// io commits `tree` in `repository` of the workspace `process` was
    /// spawned in last.
    Committed { process: u64, repository: Vec<u8>, tree: Files },
    /// The forge moves `branch` of `remote` to `commit`, which holds `tree`.
    Moved { remote: Vec<u8>, branch: Vec<u8>, commit: u64, tree: Files },
    /// The engine hears the worker's answer for `attempt`, of `kind`, and the
    /// commits it says landed, by the repository's place in the assignment.
    Reported { attempt: Token, kind: &'static str, report: Report, landed: Vec<(usize, u64)> },
}

/// A repository of an assignment's workspace.
#[derive(Clone, Debug)]
pub struct Repository {
    pub name: Vec<u8>,
    pub remote: Vec<u8>,
    /// The branch a change is pushed to, if it may be written.
    pub push: Option<Vec<u8>>,
}

/// The worker's answer for an attempt, as the engine hears it.
#[derive(Debug)]
pub enum Report {
    Refused,
    /// The run ended with `outcome`.
    Ended {
        outcome: Box<[u8]>,
    },
    Parked,
    Failed(host::Failure),
}

impl Report {
    /// What the referee keeps of `answer`.
    #[must_use]
    pub fn of(answer: &host::Answer) -> Report {
        match answer {
            host::Answer::Refused(_) => Report::Refused,
            host::Answer::Ended { outcome, .. } => Report::Ended { outcome: outcome.clone() },
            host::Answer::Parked { .. } => Report::Parked,
            host::Answer::Failed { failure, .. } => Report::Failed(*failure),
        }
    }
}

/// What the referee expects to happen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Expected {
    /// The worker answers the attempt the engine names so.
    Answer(u64),
}

/// This world injects nothing of its own: the channel to the engine never
/// drops, and nothing restarts.
#[derive(Debug)]
pub enum Stimulus {}

/// The expectations where the worker and the agent meet.
#[derive(Debug)]
pub struct Meeting {
    /// How long the worker may take to answer an assignment.
    within: Duration,
    attempts: BTreeMap<Token, Attempt>,
    processes: BTreeMap<u64, Process>,
    /// The commits the forge's branches moved to, by remote and branch, and
    /// the trees they hold.
    moved: BTreeSet<(Vec<u8>, Vec<u8>, u64)>,
    trees: BTreeMap<u64, Files>,
}

/// An assignment, as the referee saw it.
#[derive(Debug)]
struct Attempt {
    repositories: Vec<Repository>,
    /// The agent process its start came down to.
    process: Option<u64>,
}

/// An agent process, as the referee saw it.
#[derive(Default, Debug)]
struct Process {
    /// What its writable repositories held when it last asked to push, and
    /// what every repository held once it had gone.
    asked: BTreeMap<Vec<u8>, Files>,
    left: Option<BTreeMap<Vec<u8>, Files>>,
    /// Its run's answer, and whether it was signalled before the worker read
    /// how its run finishes.
    answer: Option<run::Answer>,
    stopped: bool,
}

impl Meeting {
    /// Expectations under which the worker answers each assignment `within`
    /// its assignment.
    #[must_use]
    pub fn new(within: Duration) -> Meeting {
        Meeting {
            within,
            attempts: BTreeMap::new(),
            processes: BTreeMap::new(),
            moved: BTreeSet::new(),
            trees: BTreeMap::new(),
        }
    }

    fn process(&mut self, process: u64) -> &mut Process {
        self.processes.entry(process).or_default()
    }

    /// The worker commits exactly what the agent of `process` left in
    /// `repository`: the tree it asked to push while it runs, the tree it
    /// left once it has gone.
    fn committed(&mut self, process: u64, repository: &[u8], tree: &Files, judge: &mut Judge<Expected, Stimulus>) {
        let process = self.process(process);
        let left = process.left.as_ref().unwrap_or(&process.asked);
        judge.check(left.get(repository) == Some(tree), "the worker commits exactly the tree its agent left");
    }

    /// The engine hears the worker's `report` for `attempt`: it records what
    /// the run said, if it said anything before its agent went, and what
    /// landed is on the forge, exactly the trees its agent left when it asked
    /// to push.
    fn reported(
        &mut self,
        attempt: Token,
        kind: &str,
        report: &Report,
        landed: &[(usize, u64)],
        judge: &mut Judge<Expected, Stimulus>,
    ) {
        judge.meet(&Expected::Answer(attempt.raw()));
        let record = self.attempts.get(&attempt).expect("the world observes an assignment before its answer");
        let ends = match report {
            Report::Ended { .. } => true,
            Report::Refused | Report::Parked | Report::Failed(_) => false,
        };
        let Some(id) = record.process else {
            judge.check(!ends && landed.is_empty(), "a run ends, and lands a change, only once its agent started");
            return;
        };
        let process = self.processes.entry(id).or_default();
        // Stopped before the worker read how the run finishes, the run is
        // answered as the worker stopped it: its agent's fault, or a cancel.
        let stopped = match report {
            Report::Failed(failure) => match failure {
                host::Failure::Cancelled(_) | host::Failure::Agent(_) => true,
                host::Failure::Unprepared(_) | host::Failure::Run(_) => false,
            },
            Report::Refused | Report::Ended { .. } | Report::Parked => false,
        };
        let own = !(process.stopped && stopped);
        let expected = match &process.answer {
            Some(_) if !own => None,
            Some(run::Answer::Accepted { outcome, .. }) => {
                match report {
                    Report::Ended { outcome: reported } => {
                        let accepted = channel::outcome(outcome);
                        judge.check(*reported == accepted, "the engine records the outcome the run accepted");
                    }
                    Report::Refused | Report::Parked | Report::Failed(_) => {
                        judge.fail(format_args!("a run that accepted its outcome ends with it, not {kind}"));
                    }
                }
                match outcome {
                    Declared::Change(_) => judge.check(!landed.is_empty(), "a change accepted has landed"),
                    Declared::Verdict(_) => judge.check(landed.is_empty(), "a verdict lands nothing"),
                }
                None
            }
            Some(run::Answer::Failed { failure: run::Failure::Cancelled, .. }) => {
                judge.check(stopped, format_args!("a run cancelled is answered as the worker stopped it, not {kind}"));
                None
            }
            Some(run::Answer::Failed { failure, .. }) => Some(host::Failure::Run(run_failure(*failure))),
            Some(run::Answer::Refused(_)) => Some(host::Failure::Run(host::RunFailure::Policy)),
            None => {
                judge.check(!ends, "a run ends only as its agent says");
                None
            }
        };
        if let Some(expected) = expected {
            let failed = match report {
                Report::Failed(failure) => Some(*failure),
                Report::Refused | Report::Ended { .. } | Report::Parked => None,
            };
            judge.check(failed == Some(expected), format_args!("the engine records how the run failed: {kind}"));
        }
        for (index, commit) in landed {
            let repository = &record.repositories[*index];
            let Some(branch) = &repository.push else {
                judge.fail("a change lands in a repository that may be written");
                continue;
            };
            let on_branch = self.moved.contains(&(repository.remote.clone(), branch.clone(), *commit));
            judge.check(on_branch, "what landed is on its branch");
            let tree = self.trees.get(commit);
            judge.check(
                tree.is_some() && tree == process.asked.get(&repository.name),
                "what landed is the tree its agent left",
            );
        }
    }
}

impl Expectations for Meeting {
    type Seen = Seen;
    type Name = Expected;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Expected, Stimulus>) {
        match seen {
            Seen::Assigned { attempt, repositories } => {
                self.attempts.insert(attempt, Attempt { repositories, process: None });
                judge.expect(Expected::Answer(attempt.raw()), self.within);
            }
            Seen::Started { process, attempt } => {
                let record =
                    self.attempts.get_mut(&attempt).expect("the world observes an assignment before its start");
                record.process = Some(process);
            }
            Seen::Asked { process, trees } => self.process(process).asked.extend(trees),
            Seen::Answered { process, answer } => self.process(process).answer = Some(answer),
            Seen::Stopped { process } => self.process(process).stopped = true,
            Seen::Gone { process, left } => self.process(process).left = Some(left),
            Seen::Committed { process, repository, tree } => self.committed(process, &repository, &tree, judge),
            Seen::Moved { remote, branch, commit, tree } => {
                self.moved.insert((remote, branch, commit));
                self.trees.insert(commit, tree);
            }
            Seen::Reported { attempt, kind, report, landed } => self.reported(attempt, kind, &report, &landed, judge),
        }
    }
}

/// The worker's word for how a run failed, as it reports it.
fn run_failure(failure: run::Failure) -> host::RunFailure {
    match failure {
        run::Failure::Model(_) => host::RunFailure::Model,
        run::Failure::Budget(_) => host::RunFailure::Budget,
        run::Failure::Policy(_) => host::RunFailure::Policy,
        run::Failure::Cancelled => host::RunFailure::Cancelled,
        run::Failure::Stale => host::RunFailure::Stale,
    }
}
