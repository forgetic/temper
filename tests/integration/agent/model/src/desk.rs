//! What people hand in: issues whose step is an agent step or a change, as
//! the engine takes them in when it finds them tracked with a record of its
//! own (engine-model.md, 4.6), the way it made the tasks a session asked for
//! or a plan's steps. An agent's run cannot take a session's turns yet
//! (inbound events, waiting, parking and relayed calls are not built on its
//! side), so nothing here is handed in to start a session: no issue carries
//! the hand-in label.
//!
//! Each issue is opened by a person; the record, which says the step and that
//! it waits for its first run, is the engine's own comment, written as an
//! earlier life of the engine would have; and the tracking label goes on
//! last, so that the engine never finds the issue tracked without its record.
//!
//! A step's guidance starts with the word that cues its job's script
//! ([`crate::script`]); a change reviewed by an agent gives its reviewer the
//! review's.

use temper_engine_model::plan::{self, AgentSpec, Budget, ChangeSpec, Grants, Progress, Review, Step};
use temper_engine_model::work::{Failures, Lifecycle, Phase};
use temper_engine_model::{Record, Relations};
use temper_engine_model_tests::deployment::MAIN;
use temper_lib::{Duration, Time};

use crate::script::{self, Job};

/// An issue a person hands in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hand {
    /// When, from the world's start.
    pub at: Duration,
    /// The deployment's repository it is in.
    pub repository: u32,
    /// The script its runs play.
    pub job: Job,
    pub work: Work,
    pub grants: Grants,
    pub budget: Budget,
}

/// The step an issue carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Work {
    /// An agent step: a run that reports.
    Agent,
    /// A change, its checks to pass before a push if `checks`, reviewed by a
    /// person or by an agent's run.
    Change { checks: bool, reviewer: Reviewer },
}

/// Who reviews a change: a person, or an agent's run that reads, or one
/// that may also write and run commands, in a checkout of its own that it
/// may push from, though it finishes only with a verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reviewer {
    Person,
    Agent,
    Editor,
}

/// What a coding run may do: write, run commands, and ask for sub-agents.
pub const CODING: Grants = Grants { modify: true, shell: true, forge: false, subagents: true, note: false };

/// What a reviewing run may do: read.
pub const READING: Grants = Grants { modify: false, shell: false, forge: false, subagents: false, note: false };

/// What a reviewing run that may write does: write, and run commands.
pub const EDITING: Grants = Grants { modify: true, shell: true, forge: false, subagents: false, note: false };

/// The issue's title.
#[must_use]
pub fn title(hand: &Hand) -> Vec<u8> {
    format!("{:?} in the answer's repository", hand.job).into_bytes()
}

/// The record the engine finds on the issue: its step, waiting for its first
/// run, taken in at `created`.
#[must_use]
pub fn record(hand: &Hand, created: Time) -> Record {
    let produce = charter(guidance(hand.job), hand.grants, hand.budget);
    let work = match hand.work {
        Work::Agent => plan::Work::Agent(AgentSpec { charter: produce, grows: false }),
        Work::Change { checks, reviewer } => {
            let review = match reviewer {
                Reviewer::Person => Review::Person,
                Reviewer::Agent => Review::Agent(charter(guidance(Job::Review), READING, hand.budget)),
                Reviewer::Editor => Review::Agent(charter(guidance(Job::Review), EDITING, hand.budget)),
            };
            plan::Work::Change(ChangeSpec { base: MAIN.into(), produce, checks, review })
        }
    };
    let step = Step {
        name: b"step".as_slice().into(),
        repository: plan::Repository(hand.repository),
        work,
        after: Box::new([]),
        gates: Box::new([]),
    };
    Record {
        lifecycle: Lifecycle { phase: Phase::Waiting, attempts: 0, failures: Failures::NONE },
        step: plan::Record { step, progress: Progress::NEW, goal: None },
        relations: Relations {
            created,
            goal: None,
            parent: None,
            pull: None,
            branch: None,
            dependencies: Box::new([]),
            children: Box::new([]),
            decision: None,
            accepted: None,
            snapshot: false,
            spent: 0,
        },
    }
}

fn charter(instructions: Vec<u8>, grants: Grants, budget: Budget) -> plan::Charter {
    plan::Charter { instructions: instructions.into(), template: None, grants, budget }
}

/// A step's guidance for `job`: its cue first, if it has one.
#[must_use]
pub fn guidance(job: Job) -> Vec<u8> {
    let words: &[u8] = match job {
        Job::Coding | Job::Delegating => b"Make the answer 43.",
        Job::Review => b"Review the change.",
        Job::Reporting => b"Report what the answer is.",
        Job::Spending => b"Read everything.",
        Job::Wandering => b"Look around.",
    };
    match script::cue(job) {
        Some(cue) => [cue, b" ", words].concat(),
        None => words.to_vec(),
    }
}
