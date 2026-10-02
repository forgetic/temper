//! Between the forge's terms and the plan's: what the world's engine gathers
//! from the forge into the facts a decision reads, and how it names commits
//! and inbox events to the plan.

use temper_engine_model_plan::{
    Ci, Commit, Decided, Decision, Envelope, Facts, Inbound, Mergeable, Pull, PullState, Relations, Repair, Source,
    Step, Why,
};
use temper_lib::Time;

use crate::forge::{Forge, State};
use crate::referee::{EnvelopeSeen, Primitive, RunSeen, StepSeen};

/// A commit the forge names by `count`: the count in its first 8 bytes,
/// big-endian, as the worker's worlds name them.
#[must_use]
pub fn commit(count: u64) -> Commit {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&count.to_be_bytes());
    Commit(bytes)
}

/// The count of a commit named by [`commit`].
#[must_use]
pub fn count(commit: Commit) -> u64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&commit.0[..8]);
    u64::from_be_bytes(bytes)
}

/// Something that came into an item's inbox.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heard {
    /// A person's message, or their answer to a proposal.
    Message,
    /// One of the steps it added, finishing or held.
    Child,
    /// One of the steps it comes after, finishing.
    Dependency,
    /// Its own pull request: CI, a review, a push.
    Own,
    /// An item it subscribes to, changing.
    Subscribed,
}

/// An inbox event, as the plan reads it.
#[must_use]
pub fn inbound(heard: Heard, at: Time) -> Inbound {
    let source = match heard {
        Heard::Message => Source::Message,
        Heard::Child => Source::Child,
        Heard::Dependency => Source::Dependency,
        Heard::Own => Source::Own,
        Heard::Subscribed => Source::Subscribed,
    };
    Inbound { source, at }
}

/// What the forge shows about item `number`, as the plan reads it, with what
/// the engine holds of it in memory: whether it holds a snapshot of the
/// item's parked run, and whether its inbox wakes it.
#[must_use]
pub fn facts(forge: &Forge, number: u64, snapshot: bool, woken: bool) -> Facts {
    let item = forge.item(number);
    Facts {
        created: item.created,
        dependencies: relations(forge, &dependencies(forge, number)),
        children: relations(forge, &children(forge, number)),
        branch: item.branch.map(|pushed| commit(pushed.head)),
        pull: pull(forge, number),
        decision: item.decision.map(|(accepted, at)| Decided {
            decision: if accepted { Decision::Accepted } else { Decision::Rejected },
            at,
        }),
        closed: item.closed.is_some(),
        snapshot,
        woken,
    }
}

/// The items of the steps item `number` comes after: those of its goal named
/// in its step.
#[must_use]
pub fn dependencies(forge: &Forge, number: u64) -> Vec<u64> {
    let item = forge.item(number);
    let Some(goal) = item.goal else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for name in &item.record.step.after {
        if let Some(dependency) = named(forge, goal, name) {
            found.push(dependency);
        }
    }
    found
}

/// The item of the step named `name` under `goal`.
#[must_use]
pub fn named(forge: &Forge, goal: u64, name: &[u8]) -> Option<u64> {
    for (number, item) in &forge.items {
        if item.goal == Some(goal) && *item.record.step.name == *name {
            return Some(*number);
        }
    }
    None
}

/// The items item `number` made as its children.
#[must_use]
pub fn children(forge: &Forge, number: u64) -> Vec<u64> {
    let mut found = Vec::new();
    for (child, item) in &forge.items {
        if item.parent == Some(number) && item.goal.is_some() {
            found.push(*child);
        }
    }
    found
}

fn relations(forge: &Forge, items: &[u64]) -> Relations {
    let mut relations = Relations::NONE;
    for number in items {
        relations.total += 1;
        if let Some(closed) = forge.item(*number).closed {
            relations.done += 1;
            relations.last_done = Some(relations.last_done.map_or(closed, |last| last.max(closed)));
        }
    }
    relations
}

fn pull(forge: &Forge, number: u64) -> Option<Pull> {
    let pull = forge.pulls.get(&number)?;
    let pushed = forge.item(number).branch.expect("a pull request is opened for a pushed branch");
    let head = pushed.head;
    let base = forge.base(pull.repository, &pull.base);
    Some(Pull {
        head: commit(head),
        pushed: pushed.at,
        state: match pull.state {
            State::Open => PullState::Open,
            State::Merged => PullState::Merged,
            State::Closed => PullState::Closed,
        },
        ci: match pull.ci.get(&head) {
            None => Ci::None,
            Some(true) => Ci::Passed,
            Some(false) => Ci::Failed,
        },
        approvals: pull.approvals.get(&head).copied().unwrap_or(0),
        changes_requested: pull.changes.contains(&head),
        merge: match pull.conflicts.get(&(head, base)) {
            None => Mergeable::Unknown,
            Some(true) => Mergeable::Conflicts,
            Some(false) => Mergeable::Clean,
        },
        base_moved: pushed.on != base,
    })
}

/// A step as the referee sees it proposed.
#[must_use]
pub fn step_seen(step: &Step) -> StepSeen {
    use temper_engine_model_plan::{Gate, Review, Work};
    let primitive = match &step.work {
        Work::Agent(_) => Primitive::Agent,
        Work::Change(spec) => Primitive::Change {
            base: spec.base.to_vec(),
            agent: match spec.review {
                Review::Agent(_) => true,
                Review::Person => false,
            },
        },
        Work::Wait(_) => Primitive::Wait,
        Work::Session(_) => Primitive::Session,
    };
    let mut approvals = 0;
    let mut accepted = false;
    for gate in &step.gates {
        match gate {
            Gate::Approvals(people) => approvals = approvals.max(*people),
            Gate::Accepted => accepted = true,
        }
    }
    StepSeen {
        name: step.name.to_vec(),
        repository: step.repository.0,
        after: step.after.iter().map(|name| name.to_vec()).collect(),
        primitive,
        approvals,
        accepted,
    }
}

/// An envelope as the referee sees it proposed.
#[must_use]
pub fn envelope_seen(envelope: &Envelope) -> EnvelopeSeen {
    EnvelopeSeen {
        counts: [envelope.agents, envelope.changes, envelope.waits, envelope.sessions],
        repositories: envelope.repositories.iter().map(|repository| repository.0).collect(),
        into: envelope.into.iter().map(|target| (target.repository.0, target.base.to_vec())).collect(),
    }
}

/// Why a run runs, as the referee sees it.
#[must_use]
pub fn run_seen(why: Why) -> RunSeen {
    match why {
        Why::Work => RunSeen::Work,
        Why::Produce => RunSeen::Produce,
        Why::Repair(Repair::CiFailed | Repair::ChangesRequested) => RunSeen::Repair,
        Why::Repair(Repair::BaseMoved | Repair::Conflicts) => RunSeen::Rebase,
        Why::Review { .. } => RunSeen::Review,
        Why::Turn => RunSeen::Turn,
    }
}

/// The people a change's head needs to approve it: one if a person reviews
/// it, more if a gate asks for more.
#[must_use]
pub fn approvals_needed(step: &Step) -> u32 {
    use temper_engine_model_plan::{Gate, Review, Work};
    let mut needed = match &step.work {
        Work::Change(spec) => match spec.review {
            Review::Person => 1,
            Review::Agent(_) => 0,
        },
        Work::Agent(_) | Work::Wait(_) | Work::Session(_) => 0,
    };
    for gate in &step.gates {
        match gate {
            Gate::Approvals(people) => needed = needed.max(*people),
            Gate::Accepted => {}
        }
    }
    needed
}
