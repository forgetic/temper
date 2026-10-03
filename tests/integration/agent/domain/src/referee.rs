//! What the world's scenarios expect of the worker and the agent together,
//! held by a referee (testing-pyramid.md, 5.2) that sees only what the fakes
//! see: the engine's assignments and the answers it hears, what comes and
//! goes through each agent process's pipes and signals, the disk under the
//! agents' tools, and the forge's branches. Safety, on every observation:
//!
//! - the worker commits exactly the tree its agent left: the tree it asked
//!   to push while it runs, the tree it left once it has gone;
//! - the engine hears what the run said, if it said anything before its
//!   agent went: the outcome it accepted, or how it failed, a cancel being
//!   the worker's to report; a run ends, and lands a change, only once its
//!   agent started;
//! - the engine posts an outcome only for an attempt the worker answered as
//!   ended, and the outcome it posts is the one the answer carried, as the
//!   engine's codec reads it;
//! - the engine holds an item for a person only for what the scenario
//!   makes happen: its runs' failures, a person's stop, its plan's reasons;
//!   for its writes, or its record, only where the forge or the store were
//!   scripted to fail (or, where the world allows it, for its writes once a
//!   merge was refused for a conflict, which the engine holds for today
//!   where its run should repair it); and never for a person's acceptance,
//!   which nothing handed in here needs;
//! - what landed is on the forge: an ancestor of its branch's tip, whoever
//!   moved the branch since, exactly the tree its agent left when it asked
//!   to push;
//! - what the engine merges is what a run landed, or what another party
//!   moved its branch to, and the merge keeps every file the head changed
//!   as the head has it.
//!
//! And liveness: every assignment is answered within a bound the world
//! sets, every outcome a run ended with is posted on its item within
//! [`POSTED`], and every issue handed in ends, closed or held for a person
//! as the scenario allows, within [`STORY`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display};

use temper_agent_domain::run::{self, outcome::Declared};
use temper_checkout_fake::git::Tree as Files;
use temper_engine_domain::work::{Hold, Phase};
use temper_engine_domain::{Decoded, Item, Outcome};
use temper_engine_domain_tests::codec;
use temper_engine_domain_tests::deployment::{self, ENGINE};
use temper_forge_domain::Observation;
use temper_lib::{Duration, Token};
use temper_worker_domain::host;
use temper_world::{Expectations, Judge};

use crate::{channel, protocol};

/// Within which the engine posts the outcome a run ended with, once the
/// worker answered with it: the engine's calls are retried past the forge's
/// outages and rate limits.
pub const POSTED: Duration = Duration::from_secs(3_600);

/// Within which an issue handed in ends, closed or held: the retries of its
/// step's runs, each within the worker's wall time, and their backoffs.
pub const STORY: Duration = Duration::from_secs(12 * 3_600);

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
    /// The forge moves `branch` of `remote` to `tip`, which holds `tree`, a
    /// push of the worker's, a merge of the engine's, or `other` party's:
    /// `brought` are the commits the move brings onto the branch, from its
    /// new tip back to its old.
    Moved { remote: Vec<u8>, branch: Vec<u8>, tip: u64, brought: Vec<u64>, tree: Files, other: bool },
    /// The forge merges `head` into `base` of `remote`, with the tree
    /// `merged`: `changed` are the files the head changed since it forked
    /// from the base, as the head has them.
    Merged { remote: Vec<u8>, base: Vec<u8>, head: u64, changed: Files, merged: Files },
    /// The engine hears the worker's answer for `attempt`, of `kind`, and the
    /// commits it says landed, by the repository's place in the assignment.
    Reported { attempt: Token, kind: &'static str, report: Report, landed: Vec<(usize, u64)> },
    /// The forge did this, as it observed it.
    Forge(Observation),
    /// A person handed `item` in.
    Handed { item: Item },
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
    /// The engine posts the outcome the worker's answer for the attempt so
    /// named ended with.
    Posted(u64),
    /// The item handed in ends: closed, or held for a person.
    Story(Item),
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
    /// Whether the forge or the store were scripted to fail, so that a
    /// write, or a record, may fail for good.
    faults: bool,
    attempts: BTreeMap<Token, Attempt>,
    processes: BTreeMap<u64, Process>,
    /// Each branch of the forge seen to move, by remote and branch: its tip
    /// and the commits of its history; and the trees of the commits moved
    /// to.
    branches: BTreeMap<(Vec<u8>, Vec<u8>), Branch>,
    trees: BTreeMap<u64, Files>,
    /// The commits runs landed, and those another party made.
    landed: BTreeSet<u64>,
    others: BTreeSet<u64>,
    /// The outcome each answer that said its run ended carried, by attempt,
    /// as the engine's codec reads it.
    ended: BTreeMap<Token, Option<Outcome>>,
}

/// A branch of the forge, as the referee saw it move.
#[derive(Debug)]
struct Branch {
    tip: u64,
    /// The tip and its ancestors, as far as moves brought them.
    history: BTreeSet<u64>,
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
    /// its assignment, and the forge or the store fail if `faults`.
    #[must_use]
    pub fn new(within: Duration, faults: bool) -> Meeting {
        Meeting {
            within,
            faults,
            attempts: BTreeMap::new(),
            processes: BTreeMap::new(),
            branches: BTreeMap::new(),
            trees: BTreeMap::new(),
            landed: BTreeSet::new(),
            others: BTreeSet::new(),
            ended: BTreeMap::new(),
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
        let left = process.left.as_ref().unwrap_or(&process.asked).get(repository);
        judge.check(
            left == Some(tree),
            format_args!(
                "the worker commits exactly the tree its agent left: {} committed, {} left",
                Show(Some(tree)),
                Show(left)
            ),
        );
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
        match report {
            Report::Ended { outcome } => {
                let outcome = codec::outcome_of(outcome);
                judge.check(outcome.is_some(), "an outcome the worker carries decodes as the engine's codec reads it");
                self.ended.insert(attempt, outcome);
                judge.expect(Expected::Posted(attempt.raw()), POSTED);
            }
            Report::Refused | Report::Parked | Report::Failed(_) => {}
        }
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
                        judge.check(
                            *reported == accepted,
                            format_args!(
                                "the worker carries the outcome the run accepted: {:?} carried, {:?} accepted",
                                String::from_utf8_lossy(reported),
                                String::from_utf8_lossy(&accepted)
                            ),
                        );
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
            judge.check(
                failed == Some(expected),
                format_args!("the engine records how the run failed: {kind} recorded, {expected:?} expected"),
            );
        }
        for (index, commit) in landed {
            self.landed.insert(*commit);
            let repository = &record.repositories[*index];
            let Some(branch) = &repository.push else {
                judge.fail("a change lands in a repository that may be written");
                continue;
            };
            // On its branch: an ancestor of the branch's tip, now.
            let seen = self.branches.get(&(repository.remote.clone(), branch.clone()));
            let tip = seen.map(|branch| branch.tip);
            let on_branch = seen.is_some_and(|branch| branch.history.contains(commit));
            judge.check(
                on_branch,
                format_args!(
                    "what landed is on its branch: {commit} is not an ancestor of {}'s tip, {tip:?}",
                    String::from_utf8_lossy(branch)
                ),
            );
            let tree = self.trees.get(commit);
            let asked = process.asked.get(&repository.name);
            judge.check(
                tree.is_some() && tree == asked,
                format_args!("what landed is the tree its agent left: {} landed, {} left", Show(tree), Show(asked)),
            );
        }
    }
}

impl Meeting {
    /// What the forge did: the engine's comments, each an outcome it posted
    /// or its record.
    fn forge(&mut self, observation: &Observation, judge: &mut Judge<Expected, Stimulus>) {
        match observation {
            Observation::Commented { repository, number, id, body, by } if *by == ENGINE => {
                let Some(index) = deployment::index(repository) else { return };
                let item = Item { repository: index, number: *number };
                match codec::comment(*id, body) {
                    Some(Decoded::Outcome { posted, .. }) => self.posted(item, posted.attempt, &posted.outcome, judge),
                    Some(Decoded::Record { record, .. }) => self.recorded(item, record.lifecycle.phase, judge),
                    Some(Decoded::Page { .. }) | None => {}
                }
            }
            Observation::Edited { repository, number, id, body, by } if *by == ENGINE => {
                let Some(index) = deployment::index(repository) else { return };
                let item = Item { repository: index, number: *number };
                match codec::comment(*id, body) {
                    Some(Decoded::Record { record, .. }) => self.recorded(item, record.lifecycle.phase, judge),
                    Some(Decoded::Outcome { .. } | Decoded::Page { .. }) | None => {}
                }
            }
            Observation::Closed { repository, number, .. } => {
                if let Some(index) = deployment::index(repository) {
                    judge.meet(&Expected::Story(Item { repository: index, number: *number }));
                }
            }
            Observation::Commented { .. }
            | Observation::Edited { .. }
            | Observation::Refused { .. }
            | Observation::Moved { .. }
            | Observation::Merged { .. }
            | Observation::Wiki { .. }
            | Observation::Deleted { .. }
            | Observation::Opened { .. }
            | Observation::Reopened { .. }
            | Observation::Labelled { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Removed { .. }
            | Observation::Reviewed { .. }
            | Observation::Reported { .. }
            | Observation::Rejected { .. } => {}
        }
    }

    /// The engine records `item` in `phase`: held only for what the scenario
    /// allows, which ends its story.
    fn recorded(&mut self, item: Item, phase: Phase, judge: &mut Judge<Expected, Stimulus>) {
        let why = match phase {
            Phase::Held { why, .. } => why,
            Phase::Waiting
            | Phase::Parked
            | Phase::Retrying(_)
            | Phase::Claimed
            | Phase::Applying { .. }
            | Phase::Done => return,
        };
        let allowed = match why {
            Hold::Failures(_) | Hold::Stopped | Hold::Plan { .. } => true,
            Hold::Writes | Hold::Record => self.faults,
            Hold::Acceptance => false,
        };
        judge.check(
            allowed,
            format_args!(
                "the engine holds {item:?} only for what the scenario makes happen, not {why:?} (the forge and \
                 the store {})",
                if self.faults { "fail" } else { "never fail" }
            ),
        );
        judge.meet(&Expected::Story(item));
    }

    /// The engine posts `outcome` on `item` for its attempt `count`: only
    /// for an attempt the worker answered as ended, and the outcome that
    /// answer carried.
    fn posted(&mut self, item: Item, count: u64, outcome: &Outcome, judge: &mut Judge<Expected, Stimulus>) {
        let attempt = protocol::attempt(item, count);
        let Some(carried) = self.ended.get(&attempt) else {
            judge.fail(format_args!(
                "the engine posts an outcome only for an attempt answered as ended: {item:?}#{count}, {outcome:?}"
            ));
            return;
        };
        judge.check(
            carried.as_ref() == Some(outcome),
            format_args!(
                "the engine posts the outcome the run ended with: {item:?}#{count} posted {outcome:?}, ended with \
                 {carried:?}"
            ),
        );
        judge.meet(&Expected::Posted(attempt.raw()));
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
            Seen::Moved { remote, branch, tip, brought, tree, other } => {
                if other {
                    self.others.extend(brought.iter().copied());
                }
                let branch =
                    self.branches.entry((remote, branch)).or_insert_with(|| Branch { tip, history: BTreeSet::new() });
                branch.tip = tip;
                branch.history.extend(brought);
                self.trees.insert(tip, tree);
            }
            Seen::Merged { remote, base, head, changed, merged } => {
                judge.check(
                    self.landed.contains(&head) || self.others.contains(&head),
                    format_args!(
                        "the engine merges into {}'s {} only what a run landed or another party made, not {head}",
                        String::from_utf8_lossy(&remote),
                        String::from_utf8_lossy(&base)
                    ),
                );
                for (path, content) in &changed {
                    judge.check(
                        merged.get(path) == Some(content),
                        format_args!(
                            "a merge keeps {} as the head {head} has it: {} merged",
                            String::from_utf8_lossy(path),
                            Show(Some(&merged))
                        ),
                    );
                }
            }
            Seen::Reported { attempt, kind, report, landed } => self.reported(attempt, kind, &report, &landed, judge),
            Seen::Forge(observation) => self.forge(&observation, judge),
            Seen::Handed { item } => judge.expect(Expected::Story(item), STORY),
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

/// A tree, or none, as a failure shows it: each path, with its content.
struct Show<'a>(Option<&'a Files>);

impl Display for Show<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(files) = self.0 else {
            return f.write_str("nothing");
        };
        f.write_str("{")?;
        for (path, content) in files {
            write!(f, " {:?}: {:?}", String::from_utf8_lossy(path), String::from_utf8_lossy(content))?;
        }
        f.write_str(" }")
    }
}
