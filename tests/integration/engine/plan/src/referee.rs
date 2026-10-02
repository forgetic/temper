//! The scenario's expectations (testing-pyramid.md, 5.2): a step machine in
//! the world's loop that observes what the forge and the runs see, never the
//! engine's memory, and steps on those observations and on its own deadlines.
//!
//! Safety, checked on every observation: a step runs only once the steps it
//! comes after, and the steps they added, are done, and once a person has
//! accepted it if a gate asks for that; nothing is merged unless its gates
//! hold at that exact head (CI passed on it, the review its step asks for,
//! the approvals and the acceptance its gates ask for, its dependencies
//! done); growth beyond a goal's envelope is never applied without a person's
//! acceptance; no item is made, and no pull request opened or merged,
//! outside the deployment's repositories and branches; no step is made twice.
//!
//! Liveness, as deadlines of its own: every goal, and every task on its own,
//! ends (done, or held for a person) within a bound of simulated time. The
//! world's faults all let it.
//!
//! It injects what belongs to no fake: the engine restarting while it applies
//! an outcome, after its forge writes and before its record's.

use std::collections::{BTreeMap, BTreeSet};

use temper_engine_model_plan::{Config, Gate, Review, Reviewed, Target, Verdict, Work};
use temper_lib::{Duration, Rng, Time};

use crate::forge::Forge;
use crate::translate::{children, commit, dependencies};

/// What the referee observes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Seen {
    /// A goal's session, or a task, was made: it must end.
    Began(u64),
    /// An item was made for a step.
    Made(u64),
    /// A run started for an item.
    Ran(u64),
    PullOpened(u64),
    Merged {
        item: u64,
        head: u64,
    },
    /// The goal's plan grew by the steps of `added`, accepted by a person or
    /// not.
    Grown {
        goal: u64,
        added: Vec<u64>,
        accepted: bool,
    },
    /// An item was closed, or held.
    Ended(u64),
}

#[derive(Debug)]
pub struct Referee {
    config: Config,
    rng: Rng,
    /// The chance, per mille, that the engine restarts as it applies an
    /// outcome.
    restarts: u32,
    bound: Duration,
    /// When each goal or task must have ended by.
    deadlines: BTreeMap<u64, Time>,
    /// The steps made, by goal and name.
    made: BTreeSet<(Option<u64>, Vec<u8>)>,
    /// Observations checked.
    pub checked: u32,
}

impl Referee {
    #[must_use]
    pub fn new(config: Config, seed: u64, restarts: u32, bound: Duration) -> Referee {
        Referee {
            config,
            rng: Rng::new(seed),
            restarts,
            bound,
            deadlines: BTreeMap::new(),
            made: BTreeSet::new(),
            checked: 0,
        }
    }

    /// Steps on an observation: checks safety, and arms or disarms
    /// liveness.
    pub fn observe(&mut self, now: Time, forge: &Forge, seen: &Seen) {
        self.checked += 1;
        match seen {
            Seen::Began(item) => {
                self.deadlines.insert(*item, now.saturating_add(self.bound));
            }
            Seen::Made(item) => self.made(forge, *item),
            Seen::Ran(item) => ran(forge, *item),
            Seen::PullOpened(item) => {
                let pull = &forge.pulls[item];
                assert!(self.lands(pull.repository, &pull.base), "item {item}'s pull request lands in the deployment");
            }
            Seen::Merged { item, head } => self.merged(forge, *item, *head),
            Seen::Grown { goal, added, accepted } => grown(forge, *goal, added, *accepted),
            Seen::Ended(_) => {}
        }
        let mut ended = Vec::new();
        for goal in self.deadlines.keys() {
            if has_ended(forge, *goal) {
                ended.push(*goal);
            }
        }
        for goal in ended {
            self.deadlines.remove(&goal);
        }
    }

    /// Whether the engine restarts as it applies this outcome.
    pub fn restarts(&mut self) -> bool {
        self.rng.chance(self.restarts)
    }

    /// When the earliest deadline is.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.deadlines.values().min().copied()
    }

    /// Steps on the deadlines due by `now`: a goal past its bound fails the
    /// test.
    pub fn fire(&mut self, now: Time, forge: &Forge) {
        let mut due = Vec::new();
        for (goal, at) in &self.deadlines {
            if *at <= now {
                assert!(
                    has_ended(forge, *goal),
                    "goal {goal} ended within {:?}: {}",
                    self.bound,
                    pending(forge, *goal)
                );
                due.push(*goal);
            }
        }
        for goal in due {
            self.deadlines.remove(&goal);
        }
    }

    /// The verdict, once nothing is left to happen: every goal ended.
    pub fn verdict(&self, forge: &Forge) {
        if let Some(goal) = self.deadlines.keys().next() {
            panic!("goal {goal} never ended: {}", pending(forge, *goal));
        }
    }

    fn made(&mut self, forge: &Forge, number: u64) {
        let item = forge.item(number);
        let repositories = u32::try_from(self.config.repositories.len()).expect("fits");
        assert!(item.repository < repositories, "item {number} is made in one of the deployment's repositories");
        let name = item.record.step.name.to_vec();
        assert!(self.made.insert((item.goal, name)), "item {number}'s step is made once");
        if let Work::Change(spec) = &item.record.step.work {
            assert!(self.lands(item.repository, &spec.base), "item {number}'s change lands in the deployment");
        }
    }

    fn lands(&self, repository: u32, base: &[u8]) -> bool {
        let Some(repo) = self.config.repositories.get(usize::try_from(repository).expect("fits")) else {
            return false;
        };
        repo.bases.iter().any(|known| **known == *base)
    }

    fn merged(&self, forge: &Forge, number: u64, head: u64) {
        let item = forge.item(number);
        let pull = &forge.pulls[&number];
        assert!(self.lands(pull.repository, &pull.base), "item {number} lands in the deployment");
        assert_eq!(pull.ci.get(&head), Some(&true), "item {number} is merged with CI passed on its head {head}");
        let approvals = pull.approvals.get(&head).copied().unwrap_or(0);
        let Work::Change(spec) = &item.record.step.work else {
            panic!("item {number} is merged, and it is not a change");
        };
        match &spec.review {
            Review::Person => {
                assert!(approvals > 0, "item {number} is merged with a person's approval of its head {head}");
                assert!(!pull.changes.contains(&head), "item {number} is merged with no changes asked for on {head}");
            }
            Review::Agent(_) => assert_eq!(
                item.record.progress.review,
                Some(Reviewed { head: commit(head), verdict: Verdict::Approve }),
                "item {number} is merged with an agent's approval of its head {head}"
            ),
        }
        for gate in &item.record.step.gates {
            match gate {
                Gate::Approvals(people) => {
                    assert!(approvals >= *people, "item {number} is merged with {people} approvals of {head}");
                }
                Gate::Accepted => assert_eq!(
                    item.decision.map(|(accepted, _)| accepted),
                    Some(true),
                    "item {number} is merged once accepted"
                ),
            }
        }
        done_before(forge, number);
    }
}

/// Growth beyond the goal's envelope, in the number of steps of a primitive
/// or where a change lands, is applied only once a person accepted it.
fn grown(forge: &Forge, goal: u64, added: &[u64], accepted: bool) {
    let plan = forge.item(goal).record.goal.as_ref().expect("a goal's item holds its plan");
    let (envelope, growth) = (&plan.envelope, plan.growth);
    let mut within = growth.agents <= envelope.agents
        && growth.changes <= envelope.changes
        && growth.waits <= envelope.waits
        && growth.sessions <= envelope.sessions;
    for number in added {
        let item = forge.item(*number);
        if let Work::Change(spec) = &item.record.step.work {
            let target = Target { repository: item.record.step.repository, base: spec.base.clone() };
            within = within && envelope.into.contains(&target);
        }
    }
    assert!(within || accepted, "goal {goal} grows beyond its envelope only once a person accepts it");
}

/// A step runs only once the steps it comes after are done, and once a person
/// has accepted it if a gate asks for that.
fn ran(forge: &Forge, number: u64) {
    done_before(forge, number);
    let item = forge.item(number);
    let starts_gated = match &item.record.step.work {
        Work::Change(_) => false,
        Work::Agent(_) | Work::Wait(_) | Work::Session(_) => true,
    };
    if starts_gated && item.record.step.gates.contains(&Gate::Accepted) {
        assert_eq!(
            item.decision.map(|(accepted, _)| accepted),
            Some(true),
            "item {number} runs once a person accepts it"
        );
    }
}

/// The steps item `number` comes after are done, and so are the steps each
/// of them added.
fn done_before(forge: &Forge, number: u64) {
    for dependency in dependencies(forge, number) {
        assert!(forge.is_closed(dependency), "item {number} acts after item {dependency} is done");
        for child in children(forge, dependency) {
            assert!(forge.is_closed(child), "item {number} acts after item {child}, added by {dependency}, is done");
        }
    }
}

/// Whether goal `goal` (or a task) has ended: closed, done, or with an item
/// held for a person.
fn has_ended(forge: &Forge, goal: u64) -> bool {
    if forge.is_closed(goal) {
        return true;
    }
    for (number, item) in &forge.items {
        if (*number == goal || item.goal == Some(goal)) && item.held.is_some() {
            return true;
        }
    }
    false
}

/// What of goal `goal` is still open.
fn pending(forge: &Forge, goal: u64) -> String {
    let mut open = Vec::new();
    for (number, item) in &forge.items {
        if (*number == goal || item.goal == Some(goal)) && item.closed.is_none() {
            open.push(format!("{number} {}", String::from_utf8_lossy(&item.record.step.name)));
        }
    }
    format!("open: {}", open.join(", "))
}
