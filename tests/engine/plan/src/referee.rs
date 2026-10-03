//! The scenario's expectations (testing.md, 5.2), held by the shared
//! [`Referee`](temper_world::Referee): what they observe is what the forge
//! and the runs see (the steps a run proposed, the items made, CI, reviews,
//! decisions, verdicts as runs give them, merges, holds and releases), never
//! the engine's records, which the plan writes.
//!
//! Safety, checked on every observation:
//! - a step runs only on an open item that is not held, once every step its
//!   spec names is made and done, and so are the steps each of them added,
//!   and once a person has accepted it (since its last release) if a gate
//!   asks for that;
//! - nothing is merged unless its gates hold at that exact head: CI passed on
//!   it, the review its spec asks for (a person's approval with nobody asking
//!   for changes, or an agent's approving verdict), the approvals and the
//!   acceptance its gates ask for, its dependencies done;
//! - growth beyond a goal's envelope (counted from the steps made, against
//!   the envelope its plan was proposed with) is made only once a person
//!   accepted it, and it widens the envelope;
//! - a change is repaired for failures, and rebased, no more often than the
//!   limits allow between releases;
//! - nothing is made, opened or merged outside the deployment, no step is
//!   made twice, and no item is made that no run proposed.
//!
//! Liveness, as deadlines: every goal, and every task on its own, ends within
//! a bound of simulated time: every item of it done, or held for a person. A
//! release arms it again.
//!
//! It injects what belongs to no fake: the engine restarting as it makes an
//! application's writes, after a drawn number of them.

use std::collections::{BTreeMap, BTreeSet};

use skein_lib::{Duration, Rng};
use temper_world::{Expectations, Judge};

/// The deployment, as the scenario sets it up: for each repository, the
/// branches changes may land into.
pub type Deployment = Vec<Vec<Vec<u8>>>;

/// What the referee observes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Seen {
    /// A person opened a session: a goal, which must end.
    Began {
        item: u64,
    },
    /// A run of `item` proposed steps: a plan with its envelope, steps to add
    /// to its goal's plan, or tasks.
    Proposed {
        item: u64,
        kind: Proposal,
        steps: Vec<StepSeen>,
        envelope: Option<EnvelopeSeen>,
    },
    /// A person accepted `item`'s proposal.
    Accepted {
        item: u64,
    },
    /// The forge made an item for a step, from `by`'s outcome, under `goal`
    /// (none for a task on its own), as the child of `parent`.
    Made {
        item: u64,
        by: u64,
        goal: Option<u64>,
        parent: Option<u64>,
        name: Vec<u8>,
        repository: u32,
    },
    /// A run started for `item`.
    Ran {
        item: u64,
        run: RunSeen,
    },
    /// A run of `item` pushed `head`.
    Pushed {
        item: u64,
        head: u64,
        run: RunSeen,
    },
    /// A review run of `item` gave its verdict on `head`.
    Verdict {
        item: u64,
        head: u64,
        approve: bool,
    },
    PullOpened {
        item: u64,
        repository: u32,
        base: Vec<u8>,
    },
    Ci {
        item: u64,
        head: u64,
        passed: bool,
    },
    /// People reviewed `head`: approving it, `approvals` of them, or asking
    /// for changes.
    Reviewed {
        item: u64,
        head: u64,
        approvals: u32,
        changes: bool,
    },
    /// A person decided on the step of `item`.
    Decided {
        item: u64,
        accepted: bool,
    },
    /// The engine merged `item` at `head`.
    Merged {
        item: u64,
        head: u64,
    },
    /// A person merged `item` by hand.
    MergedByHand {
        item: u64,
    },
    Closed {
        item: u64,
    },
    Held {
        item: u64,
    },
    Released {
        item: u64,
    },
    /// The engine is about to make `writes` writes for `item`.
    Applying {
        item: u64,
        writes: usize,
    },
}

/// What kind of proposal steps came in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Proposal {
    Plan,
    Steps,
    Tasks,
}

/// Why a run runs, as the worker's assignment says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunSeen {
    Work,
    Produce,
    /// A repair for a failure: CI failed, changes were asked for.
    Repair,
    /// A rebase: the base moved, or the change conflicts.
    Rebase,
    Review,
    Turn,
}

/// A step as a run proposed it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StepSeen {
    pub name: Vec<u8>,
    pub repository: u32,
    pub after: Vec<Vec<u8>>,
    pub primitive: Primitive,
    /// The most people its gates ask to approve its head.
    pub approvals: u32,
    /// Whether a gate asks for a person's acceptance.
    pub accepted: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Primitive {
    Agent,
    /// A change into `base`, reviewed by an agent or a person.
    Change {
        base: Vec<u8>,
        agent: bool,
    },
    Wait,
    Session,
}

/// An envelope as a run proposed it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnvelopeSeen {
    /// Steps of each primitive: agents, changes, waits, sessions.
    pub counts: [u32; 4],
    pub repositories: Vec<u32>,
    pub into: Vec<(u32, Vec<u8>)>,
}

/// A goal, or a task on its own, by its item: what must end.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Ends(pub u64);

/// What the referee injects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stimulus {
    /// The engine restarts after `landed` of the writes it is making.
    Restart { landed: usize },
}

/// What the referee knows of an item, from what it observed.
#[derive(Debug)]
struct Seeing {
    goal: Option<u64>,
    parent: Option<u64>,
    step: StepSeen,
    closed: bool,
    held: bool,
    pull: Option<(u32, Vec<u8>, bool)>,
    /// CI on each head: passed or not.
    ci: BTreeMap<u64, bool>,
    approvals: BTreeMap<u64, u32>,
    changes: BTreeSet<u64>,
    verdicts: BTreeMap<u64, bool>,
    /// The person's latest decision since the last release.
    accepted: Option<bool>,
    repairs: u32,
    rebases: u32,
}

/// What a goal grew by, against its envelope.
#[derive(Debug)]
struct Growing {
    /// The steps of its plan as proposed: the rest is growth.
    own: BTreeSet<Vec<u8>>,
    envelope: EnvelopeSeen,
    counts: [u32; 4],
}

/// The plan world's expectations.
#[derive(Debug)]
pub struct Scenario {
    deployment: Deployment,
    repairs: u32,
    rebases: u32,
    bound: Duration,
    rng: Rng,
    restarts: u32,
    /// The steps proposed, by the proposing item and name: the latest.
    proposed: BTreeMap<(u64, Vec<u8>), (Proposal, StepSeen)>,
    envelopes: BTreeMap<u64, EnvelopeSeen>,
    /// Items whose proposal a person accepted, since they last ran.
    accepted: BTreeSet<u64>,
    items: BTreeMap<u64, Seeing>,
    /// The items of each goal's steps, by name.
    names: BTreeMap<(Option<u64>, Vec<u8>), u64>,
    goals: BTreeMap<u64, Growing>,
    /// What must end.
    ends: BTreeSet<u64>,
}

impl Scenario {
    /// Expectations of a world whose deployment is `deployment`, with the
    /// limits of repairs and rebases given, every goal ending within
    /// `bound`, and the engine restarting as it applies with the chance
    /// `restarts` per mille.
    #[must_use]
    pub fn new(
        deployment: Deployment,
        (repairs, rebases): (u32, u32),
        bound: Duration,
        seed: u64,
        restarts: u32,
    ) -> Scenario {
        Scenario {
            deployment,
            repairs,
            rebases,
            bound,
            rng: Rng::new(seed),
            restarts,
            proposed: BTreeMap::new(),
            envelopes: BTreeMap::new(),
            accepted: BTreeSet::new(),
            items: BTreeMap::new(),
            names: BTreeMap::new(),
            goals: BTreeMap::new(),
            ends: BTreeSet::new(),
        }
    }

    fn lands(&self, repository: u32, base: &[u8]) -> bool {
        let Some(bases) = self.deployment.get(usize::try_from(repository).expect("fits")) else {
            return false;
        };
        bases.iter().any(|known| known == base)
    }

    fn made(
        &mut self,
        made: (u64, u64, Option<u64>, Option<u64>),
        name: &[u8],
        repository: u32,
        judge: &mut Judge<Ends, Stimulus>,
    ) {
        let (item, by, goal, parent) = made;
        let deployed = usize::try_from(repository).expect("fits") < self.deployment.len();
        judge.check(deployed, format_args!("item {item} is made in one of the deployment's repositories"));
        let first = self.names.insert((goal, name.to_vec()), item).is_none();
        judge.check(first, format_args!("item {item}'s step {} is made once", show(name)));
        let Some((kind, step)) = self.proposed.get(&(by, name.to_vec())).cloned() else {
            judge.fail(format_args!("item {item} is made for {}, which item {by} never proposed", show(name)));
            return;
        };
        if let Primitive::Change { base, .. } = &step.primitive {
            judge.check(self.lands(repository, base), format_args!("item {item}'s change lands in the deployment"));
        }
        if let Some(goal) = goal {
            self.grew(item, by, goal, kind, &step, judge);
        } else {
            self.ends.insert(item);
            judge.expect(Ends(item), self.bound);
        }
        self.items.insert(item, seeing(goal, parent, step));
    }

    /// A step made under `goal`: the plan's own, or growth, which is within
    /// the envelope or was accepted by a person, and then widens it.
    fn grew(
        &mut self,
        item: u64,
        by: u64,
        goal: u64,
        kind: Proposal,
        step: &StepSeen,
        judge: &mut Judge<Ends, Stimulus>,
    ) {
        if kind == Proposal::Plan {
            let envelope = self.envelopes.get(&by).cloned().expect("a plan is proposed with its envelope");
            let growing = self.goals.entry(goal).or_insert(Growing { own: BTreeSet::new(), envelope, counts: [0; 4] });
            growing.own.insert(step.name.clone());
            return;
        }
        let accepted = self.accepted.contains(&by);
        let Some(growing) = self.goals.get_mut(&goal) else {
            judge.fail(format_args!("item {item} grows goal {goal}, which has no plan"));
            return;
        };
        let primitive = match &step.primitive {
            Primitive::Agent => 0,
            Primitive::Change { .. } => 1,
            Primitive::Wait => 2,
            Primitive::Session => 3,
        };
        growing.counts[primitive] += 1;
        let envelope = &mut growing.envelope;
        let mut within =
            growing.counts[primitive] <= envelope.counts[primitive] && envelope.repositories.contains(&step.repository);
        if let Primitive::Change { base, .. } = &step.primitive {
            within = within && envelope.into.contains(&(step.repository, base.clone()));
        }
        judge.check(
            within || accepted,
            format_args!("goal {goal} grows by item {item} beyond its envelope only once a person accepts it"),
        );
        if !within {
            envelope.counts[primitive] = envelope.counts[primitive].max(growing.counts[primitive]);
            if !envelope.repositories.contains(&step.repository) {
                envelope.repositories.push(step.repository);
            }
            if let Primitive::Change { base, .. } = &step.primitive
                && !envelope.into.contains(&(step.repository, base.clone()))
            {
                envelope.into.push((step.repository, base.clone()));
            }
        }
    }

    /// The items of the steps `item` comes after, each made, and done with
    /// the steps it added: or why not.
    fn done_before(&self, item: u64, judge: &mut Judge<Ends, Stimulus>) {
        let seeing = &self.items[&item];
        for name in &seeing.step.after {
            let Some(dependency) = self.names.get(&(seeing.goal, name.clone())) else {
                judge.fail(format_args!("item {item} acts before its dependency {} is made", show(name)));
                continue;
            };
            judge
                .check(self.items[dependency].closed, format_args!("item {item} acts after item {dependency} is done"));
            for (child, other) in &self.items {
                if other.parent == Some(*dependency) {
                    judge.check(
                        other.closed,
                        format_args!("item {item} acts after {child}, added by {dependency}, is done"),
                    );
                }
            }
        }
    }

    fn ran(&mut self, item: u64, run: RunSeen, judge: &mut Judge<Ends, Stimulus>) {
        self.accepted.remove(&item);
        let seeing = &self.items[&item];
        judge.check(!seeing.closed && !seeing.held, format_args!("item {item} runs while it is open and not held"));
        let fits = match &seeing.step.primitive {
            Primitive::Agent => run == RunSeen::Work,
            Primitive::Change { agent, .. } => match run {
                RunSeen::Produce | RunSeen::Repair | RunSeen::Rebase => true,
                RunSeen::Review => *agent,
                RunSeen::Work | RunSeen::Turn => false,
            },
            Primitive::Wait => false,
            Primitive::Session => run == RunSeen::Turn,
        };
        judge.check(fits, format_args!("item {item} runs a {run:?} its step has"));
        self.done_before(item, judge);
        let seeing = &self.items[&item];
        if seeing.step.accepted && !is_change(&seeing.step.primitive) {
            judge.check(seeing.accepted == Some(true), format_args!("item {item} runs once a person accepts it"));
        }
    }

    fn pushed(&mut self, item: u64, run: RunSeen, judge: &mut Judge<Ends, Stimulus>) {
        let (repairs, rebases) = (self.repairs, self.rebases);
        let seeing = self.items.get_mut(&item).expect("a change pushed was made");
        match run {
            RunSeen::Repair => {
                seeing.repairs += 1;
                judge.check(seeing.repairs <= repairs, format_args!("item {item} is repaired at most {repairs} times"));
            }
            RunSeen::Rebase => {
                seeing.rebases += 1;
                judge.check(seeing.rebases <= rebases, format_args!("item {item} is rebased at most {rebases} times"));
            }
            RunSeen::Produce | RunSeen::Work | RunSeen::Review | RunSeen::Turn => {}
        }
    }

    fn merged(&self, item: u64, head: u64, judge: &mut Judge<Ends, Stimulus>) {
        let seeing = &self.items[&item];
        if let Some((repository, base, _)) = &seeing.pull {
            judge.check(self.lands(*repository, base), format_args!("item {item} lands in the deployment"));
        }
        judge
            .check(seeing.ci.get(&head) == Some(&true), format_args!("item {item} is merged with CI passed on {head}"));
        let approvals = seeing.approvals.get(&head).copied().unwrap_or(0);
        match &seeing.step.primitive {
            Primitive::Change { agent: false, .. } => {
                judge.check(approvals > 0, format_args!("item {item} is merged with a person's approval of {head}"));
                let asked = seeing.changes.contains(&head);
                judge.check(!asked, format_args!("item {item} is merged with no changes asked for on {head}"));
            }
            Primitive::Change { agent: true, .. } => {
                let approved = seeing.verdicts.get(&head) == Some(&true);
                judge.check(approved, format_args!("item {item} is merged with an agent's approval of {head}"));
            }
            Primitive::Agent | Primitive::Wait | Primitive::Session => {
                judge.fail(format_args!("item {item} is merged, and it is not a change"));
            }
        }
        let wanted = seeing.step.approvals;
        judge.check(approvals >= wanted, format_args!("item {item} is merged with {wanted} approvals of {head}"));
        if seeing.step.accepted {
            judge.check(seeing.accepted == Some(true), format_args!("item {item} is merged once accepted"));
        }
        self.done_before(item, judge);
    }

    /// Meets the expectation of the goal (or task) `item` belongs to if every
    /// item of it is done or held; arms it again if not.
    fn settle(&self, item: u64, judge: &mut Judge<Ends, Stimulus>) {
        let Some(seeing) = self.items.get(&item) else {
            return;
        };
        let goal = seeing.goal.unwrap_or(item);
        if !self.ends.contains(&goal) {
            return;
        }
        if self.ended(goal) {
            judge.meet(&Ends(goal));
        } else if !judge.is_pending(&Ends(goal)) {
            judge.rearm(Ends(goal), self.bound);
        }
    }

    /// Whether goal `goal` has ended: every item of it, the goal's own
    /// included, is done, held, or waits only on items that are held, through
    /// the steps it comes after and the steps it added.
    fn ended(&self, goal: u64) -> bool {
        let of: Vec<u64> = self
            .items
            .iter()
            .filter(|(number, seeing)| (**number == goal || seeing.goal == Some(goal)) && !seeing.closed)
            .map(|(number, _)| *number)
            .collect();
        let mut stuck: BTreeSet<u64> = of.iter().copied().filter(|number| self.items[number].held).collect();
        // Until nothing more is found stuck: as many rounds as items, at most.
        for _ in 0..=of.len() {
            let mut found = false;
            for number in &of {
                if !stuck.contains(number) && self.waits_only_on(*number, &stuck) {
                    stuck.insert(*number);
                    found = true;
                }
            }
            if !found {
                break;
            }
        }
        of.iter().all(|number| stuck.contains(number))
    }

    /// Whether item `number` waits on something, and only on items of
    /// `stuck`: steps it comes after that are not done, steps it added that
    /// are not.
    fn waits_only_on(&self, number: u64, stuck: &BTreeSet<u64>) -> bool {
        let seeing = &self.items[&number];
        let mut waits = false;
        for name in &seeing.step.after {
            match self.names.get(&(seeing.goal, name.clone())) {
                Some(dependency) if self.items[dependency].closed => {}
                Some(dependency) if stuck.contains(dependency) => waits = true,
                Some(_) | None => return false,
            }
        }
        for (child, other) in &self.items {
            if other.parent == Some(number) && !other.closed {
                if !stuck.contains(child) {
                    return false;
                }
                waits = true;
            }
        }
        waits
    }

    /// Steps made, by the referee's count: that it saw something.
    #[must_use]
    pub fn made_count(&self) -> usize {
        self.names.len()
    }
}

impl Expectations for Scenario {
    type Seen = Seen;
    type Name = Ends;
    type Stimulus = Stimulus;

    fn observe(&mut self, seen: Seen, judge: &mut Judge<Ends, Stimulus>) {
        let settles = settles(&seen);
        match seen {
            Seen::Began { item } => {
                let step = StepSeen {
                    name: Vec::new(),
                    repository: 0,
                    after: Vec::new(),
                    primitive: Primitive::Session,
                    approvals: 0,
                    accepted: false,
                };
                self.items.insert(item, seeing(None, None, step));
                self.ends.insert(item);
                judge.expect(Ends(item), self.bound);
            }
            Seen::Proposed { item, kind, steps, envelope } => {
                if let Some(envelope) = envelope {
                    self.envelopes.insert(item, envelope);
                }
                for step in steps {
                    self.proposed.insert((item, step.name.clone()), (kind, step));
                }
            }
            Seen::Accepted { item } => {
                self.accepted.insert(item);
            }
            Seen::Made { item, by, goal, parent, name, repository } => {
                self.made((item, by, goal, parent), &name, repository, judge);
            }
            Seen::Ran { item, run } => self.ran(item, run, judge),
            Seen::Pushed { item, run, .. } => self.pushed(item, run, judge),
            Seen::Verdict { item, head, approve } => {
                if let Some(seeing) = self.items.get_mut(&item) {
                    seeing.verdicts.insert(head, approve);
                }
            }
            Seen::PullOpened { item, repository, base } => {
                judge.check(
                    self.lands(repository, &base),
                    format_args!("item {item}'s pull request lands in the deployment"),
                );
                let seeing = self.items.get_mut(&item).expect("a change was made");
                judge.check(seeing.pull.is_none(), format_args!("item {item}'s pull request is opened once"));
                seeing.pull = Some((repository, base, false));
            }
            Seen::Ci { item, head, passed } => {
                self.items.get_mut(&item).expect("a change was made").ci.insert(head, passed);
            }
            Seen::Reviewed { item, head, approvals, changes } => {
                let seeing = self.items.get_mut(&item).expect("a change was made");
                if changes {
                    seeing.changes.insert(head);
                } else {
                    seeing.approvals.insert(head, approvals);
                }
            }
            Seen::Decided { item, accepted } => {
                if let Some(seeing) = self.items.get_mut(&item) {
                    seeing.accepted = Some(accepted);
                }
            }
            Seen::Merged { item, head } => self.merged(item, head, judge),
            Seen::MergedByHand { .. } => {}
            Seen::Closed { item } => {
                if let Some(seeing) = self.items.get_mut(&item) {
                    seeing.closed = true;
                }
            }
            Seen::Held { item } => {
                if let Some(seeing) = self.items.get_mut(&item) {
                    seeing.held = true;
                }
            }
            Seen::Released { item } => {
                if let Some(seeing) = self.items.get_mut(&item) {
                    seeing.held = false;
                    seeing.accepted = None;
                    seeing.repairs = 0;
                    seeing.rebases = 0;
                }
            }
            Seen::Applying { writes, .. } => {
                if self.rng.chance(self.restarts) {
                    let landed =
                        usize::try_from(self.rng.below(u64::try_from(writes).expect("fits") + 1)).expect("fits");
                    judge.inject_now(Stimulus::Restart { landed });
                }
            }
        }
        if let Some(item) = settles {
            self.settle(item, judge);
        }
    }
}

/// The item whose goal `seen` may end, or arm again.
fn settles(seen: &Seen) -> Option<u64> {
    match seen {
        Seen::Made { item, .. }
        | Seen::Closed { item }
        | Seen::Held { item }
        | Seen::Released { item }
        | Seen::Began { item } => Some(*item),
        Seen::Proposed { .. }
        | Seen::Accepted { .. }
        | Seen::Ran { .. }
        | Seen::Pushed { .. }
        | Seen::Verdict { .. }
        | Seen::PullOpened { .. }
        | Seen::Ci { .. }
        | Seen::Reviewed { .. }
        | Seen::Decided { .. }
        | Seen::Merged { .. }
        | Seen::MergedByHand { .. }
        | Seen::Applying { .. } => None,
    }
}

fn seeing(goal: Option<u64>, parent: Option<u64>, step: StepSeen) -> Seeing {
    Seeing {
        goal,
        parent,
        step,
        closed: false,
        held: false,
        pull: None,
        ci: BTreeMap::new(),
        approvals: BTreeMap::new(),
        changes: BTreeSet::new(),
        verdicts: BTreeMap::new(),
        accepted: None,
        repairs: 0,
        rebases: 0,
    }
}

fn is_change(primitive: &Primitive) -> bool {
    match primitive {
        Primitive::Change { .. } => true,
        Primitive::Agent | Primitive::Wait | Primitive::Session => false,
    }
}

fn show(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}
