//! One level-triggered change step (domain/forge.md, section 8).
//!
//! The top keeps the returned state and commits each decision before it sends
//! a delegate or effect. Pending states make a repeated step wait. This policy
//! never reads the forge or commits an outbox entry itself.
#![expect(clippy::trivially_copy_pass_by_ref, reason = "the policy entry point borrows one coherent standing snapshot")]
use alloc::boxed::Box;
use skein_lib::{List, Wall};

use crate::{
    Change, Decision, Delegate, Effect, EffectResult, Facts, Freshness, Gate, GateKind, Heard, Hold, Limits, Pull,
    Repair, State, Status, Stepped,
};

/// Decide one change from current forge and task facts, independent of its wake.
#[must_use]
pub fn step(change: &Change, facts: &Facts, standing: &Heard, now: Wall, limits: &Limits) -> Stepped {
    let mut next = change.clone();
    if change.gates.len() > usize::try_from(limits.gates).expect("u32 fits usize")
        || change.clean.len() > usize::try_from(limits.clean_heads).expect("u32 fits usize")
    {
        return hold(next, Hold::Failed);
    }
    match &change.state {
        State::Landed { merge } => return Stepped { change: next, decision: Decision::Finish { merge: *merge } },
        State::Held { was, why } => {
            if standing.released {
                return release(next, was, *why, facts, now);
            }
            return Stepped { change: next, decision: Decision::None };
        }
        State::Producing { .. }
        | State::Opening { .. }
        | State::Recreating { .. }
        | State::Reopening { .. }
        | State::Checking { .. }
        | State::Gating { .. }
        | State::Queued { .. }
        | State::First { .. }
        | State::Updating { .. }
        | State::Resolving { .. }
        | State::Repairing { .. }
        | State::Landing { .. } => {}
    }
    match facts.pull {
        Pull::Merged { commit } => {
            next.state = State::Landed { merge: commit };
            return Stepped { change: next, decision: Decision::Finish { merge: commit } };
        }
        Pull::Missing | Pull::Open { .. } | Pull::Closed => {}
    }
    if standing.cancelled {
        return Stepped { change: next, decision: Decision::Cancel };
    }
    if facts.writer_taken {
        return waiting(next, now, limits);
    }
    if let Some(why) = facts.drift {
        return hold(next, why);
    }
    match facts.pull {
        Pull::Closed => match &change.state {
            State::Reopening { .. } | State::Recreating { .. } => {}
            State::Producing { .. }
            | State::Opening { .. }
            | State::Checking { .. }
            | State::Gating { .. }
            | State::Queued { .. }
            | State::First { .. }
            | State::Updating { .. }
            | State::Resolving { .. }
            | State::Repairing { .. }
            | State::Landing { .. }
            | State::Landed { .. }
            | State::Held { .. } => return hold(next, Hold::PullClosed),
        },
        Pull::Open { base, .. } if base != facts.expected_base && !is_reopening(&change.state) => {
            return hold(next, Hold::Retargeted);
        }
        Pull::Open { .. } | Pull::Missing | Pull::Merged { .. } => {}
    }
    match &change.state {
        State::Producing { requested } => producing(next, *requested, facts, standing, now, limits),
        State::Opening { requested } => opening(next, *requested, facts, standing, now, limits),
        State::Recreating { head } => recreating(next, *head, facts, now, limits),
        State::Reopening { requested, retarget } => {
            reopening(next, *requested, *retarget, facts, standing, now, limits)
        }
        State::Checking { head } => checking(next, *head, facts, standing, now, limits),
        State::Gating { head, asked } => gating(next, *head, asked, facts, now, limits),
        State::Queued { head, ready_since } => queued(next, *head, *ready_since, facts),
        State::First { head, ready_since } => first(next, *head, *ready_since, facts, now, limits),
        State::Updating { from, base, ready_since: _ } => updating(next, *from, *base, facts, standing, now, limits),
        State::Resolving { requested, base } => resolving(next, *requested, *base, facts, standing, now, limits),
        State::Repairing { requested, why } => repairing(next, *requested, *why, facts, standing, now, limits),
        State::Landing { head } => landing(next, *head, standing, now, limits),
        State::Landed { .. } | State::Held { .. } => unreachable!("terminal states returned above"),
    }
}

fn is_reopening(state: &State) -> bool {
    match state {
        State::Reopening { .. } => true,
        State::Producing { .. }
        | State::Opening { .. }
        | State::Recreating { .. }
        | State::Checking { .. }
        | State::Gating { .. }
        | State::Queued { .. }
        | State::First { .. }
        | State::Updating { .. }
        | State::Resolving { .. }
        | State::Repairing { .. }
        | State::Landing { .. }
        | State::Landed { .. }
        | State::Held { .. } => false,
    }
}

fn release(mut change: Change, was: &State, why: Hold, facts: &Facts, now: Wall) -> Stepped {
    change.repairs = 0;
    change.resolutions = 0;
    change.updates = 0;
    change.since = now;
    change.owns_turn = false;
    match why {
        Hold::PullClosed => {
            change.state = State::Reopening { requested: true, retarget: false };
            Stepped { change, decision: Decision::Effect(Effect::Reopen) }
        }
        Hold::Retargeted => {
            change.state = State::Reopening { requested: true, retarget: true };
            Stepped { change, decision: Decision::Effect(Effect::Retarget) }
        }
        Hold::BranchMissing => match change.last_head {
            Some(head) => {
                change.state = State::Recreating { head };
                Stepped { change, decision: Decision::Effect(Effect::CreateBranch { head }) }
            }
            None => {
                change.state = State::Producing { requested: false };
                Stepped { change, decision: Decision::None }
            }
        },
        Hold::BranchMoved => {
            change.clean = Box::new([]);
            change.state = match facts.branch {
                Some(head) => State::Checking { head },
                None => State::Producing { requested: false },
            };
            Stepped { change, decision: Decision::None }
        }
        Hold::Stalled | Hold::Repairs | Hold::Resolutions | Hold::Updates | Hold::Failed | Hold::Rejected => {
            change.state = match was {
                State::Queued { head, .. } | State::First { head, .. } | State::Gating { head, .. } => {
                    State::Gating { head: *head, asked: Box::new([]) }
                }
                State::Held { .. } | State::Landed { .. } => unreachable!("held state retains a live predecessor"),
                State::Producing { .. }
                | State::Opening { .. }
                | State::Recreating { .. }
                | State::Reopening { .. }
                | State::Checking { .. }
                | State::Updating { .. }
                | State::Resolving { .. }
                | State::Repairing { .. }
                | State::Landing { .. } => was.clone(),
            };
            Stepped { change, decision: Decision::None }
        }
    }
}

fn waiting(change: Change, now: Wall, limits: &Limits) -> Stepped {
    let until = Wall::from_nanos(change.since.as_nanos().saturating_add(limits.stall.as_nanos()));
    if now >= until {
        hold(change, Hold::Stalled)
    } else {
        Stepped { change, decision: Decision::Wait { until: Some(until) } }
    }
}

fn hold(mut change: Change, why: Hold) -> Stepped {
    if change.owns_turn {
        change.owns_turn = false;
    }
    change.state = State::Held { was: Box::new(change.state.clone()), why };
    Stepped { change, decision: Decision::Hold(why) }
}

fn producing(
    mut change: Change,
    requested: bool,
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    match facts.branch {
        Some(head) => {
            change.state = State::Opening { requested: false };
            change.last_head = Some(head);
            change.since = now;
            Stepped { change, decision: Decision::None }
        }
        None if !requested => {
            change.state = State::Producing { requested: true };
            Stepped { change, decision: Decision::Delegate(Delegate::Produce) }
        }
        None => match standing.delegate {
            Status::Failed => hold(change, Hold::Failed),
            Status::Unknown | Status::Pending | Status::Passed => waiting(change, now, limits),
        },
    }
}

fn opening(
    mut change: Change,
    requested: bool,
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    let Some(head) = facts.branch else {
        return hold(change, Hold::BranchMissing);
    };
    change.last_head = Some(head);
    match facts.pull {
        Pull::Open { head: pull_head, .. } => {
            if head != pull_head {
                return hold(change, Hold::BranchMoved);
            }
            change.state = State::Checking { head };
            change.since = now;
            Stepped { change, decision: Decision::None }
        }
        Pull::Missing if !requested => {
            change.state = State::Opening { requested: true };
            Stepped { change, decision: Decision::Effect(Effect::Open) }
        }
        Pull::Missing => match standing.effect {
            EffectResult::Failed | EffectResult::Conflict => hold(change, Hold::Failed),
            EffectResult::None | EffectResult::Pending | EffectResult::Made => waiting(change, now, limits),
        },
        Pull::Closed | Pull::Merged { .. } => unreachable!("closed and merged pulls returned above"),
    }
}

fn recreating(mut change: Change, head: [u8; 32], facts: &Facts, now: Wall, limits: &Limits) -> Stepped {
    match facts.branch {
        Some(observed) if observed == head => {
            change.state = match facts.pull {
                Pull::Closed => State::Reopening { requested: false, retarget: false },
                Pull::Missing | Pull::Open { .. } | Pull::Merged { .. } => State::Opening { requested: false },
            };
            Stepped { change, decision: Decision::None }
        }
        Some(_) => hold(change, Hold::BranchMoved),
        None => waiting(change, now, limits),
    }
}

fn reopening(
    mut change: Change,
    requested: bool,
    retarget: bool,
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    let Some(head) = facts.branch else {
        return hold(change, Hold::BranchMissing);
    };
    match facts.pull {
        Pull::Open { head: pull_head, base } if pull_head == head && base == facts.expected_base => {
            change.state = State::Checking { head };
            change.since = now;
            Stepped { change, decision: Decision::None }
        }
        Pull::Open { .. } | Pull::Closed | Pull::Missing if !requested => {
            change.state = State::Reopening { requested: true, retarget };
            let effect = if retarget { Effect::Retarget } else { Effect::Reopen };
            Stepped { change, decision: Decision::Effect(effect) }
        }
        Pull::Open { .. } | Pull::Closed | Pull::Missing => match standing.effect {
            EffectResult::Failed | EffectResult::Conflict => hold(change, Hold::Failed),
            EffectResult::None | EffectResult::Pending | EffectResult::Made => waiting(change, now, limits),
        },
        Pull::Merged { .. } => unreachable!("merged pull returned above"),
    }
}

fn checking(
    mut change: Change,
    head: [u8; 32],
    facts: &Facts,
    _standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    let Some(branch) = facts.branch else {
        return hold(change, Hold::BranchMissing);
    };
    if branch != head {
        return hold(change, Hold::BranchMoved);
    }
    change.last_head = Some(head);
    if facts.ci.head != head {
        return waiting(change, now, limits);
    }
    match facts.ci.status {
        Status::Passed => {
            change.state = State::Gating { head, asked: Box::new([]) };
            Stepped { change, decision: Decision::None }
        }
        Status::Failed => {
            let why = if change.clean.is_empty() { Repair::Ci } else { Repair::Semantic };
            repair_due(change, why, facts, now, limits)
        }
        Status::Unknown | Status::Pending => {
            if has_eager_gate(&change) {
                change.state = State::Gating { head, asked: Box::new([]) };
                Stepped { change, decision: Decision::None }
            } else {
                waiting(change, now, limits)
            }
        }
    }
}

fn has_eager_gate(change: &Change) -> bool {
    for gate in &change.gates {
        if gate.eager {
            return true;
        }
    }
    false
}

fn valid(change: &Change, gate: Gate, report: crate::GateReport, head: [u8; 32]) -> bool {
    if report.number != gate.number {
        return false;
    }
    if report.head == head {
        return true;
    }
    match gate.freshness {
        Freshness::Exact => false,
        Freshness::Clean => change.clean.contains(&report.head),
    }
}

fn verdict(change: &Change, gate: Gate, facts: &Facts, head: [u8; 32]) -> Status {
    let mut passed = false;
    let mut pending = false;
    let mut failed = false;
    for report in &facts.gates {
        if !valid(change, gate, *report, head) {
            continue;
        }
        match report.status {
            Status::Passed => passed = true,
            Status::Pending => pending = true,
            Status::Failed => failed = true,
            Status::Unknown => {}
        }
    }
    if failed {
        Status::Failed
    } else if pending {
        Status::Pending
    } else if passed {
        Status::Passed
    } else {
        Status::Unknown
    }
}

fn gating(mut change: Change, head: [u8; 32], asked: &[u64], facts: &Facts, now: Wall, limits: &Limits) -> Stepped {
    if facts.branch != Some(head) {
        return hold(change, Hold::BranchMoved);
    }
    if facts.ci.head == head && facts.ci.status == Status::Failed {
        let why = if change.clean.is_empty() { Repair::Ci } else { Repair::Semantic };
        return repair_due(change, why, facts, now, limits);
    }
    let ci_passed = facts.ci.head == head && facts.ci.status == Status::Passed;
    let mut waiting_gate = false;
    for index in 0..change.gates.len() {
        let gate = *change.gates.get(index).expect("index is in bounds");
        if !ci_passed && !gate.eager {
            continue;
        }
        match verdict(&change, gate, facts, head) {
            Status::Failed if gate.blocking => match gate.kind {
                GateKind::Person => return hold(change, Hold::Rejected),
                GateKind::Agent | GateKind::Check => {
                    return repair_due(change, Repair::Gate(gate.number), facts, now, limits);
                }
            },
            Status::Unknown if !asked.contains(&gate.number) => {
                let mut collected = List::with_capacity(limits.gates);
                for number in asked {
                    collected.push(*number).expect("asked gates fit the limit");
                }
                collected.push(gate.number).expect("gate admission checked");
                change.state = State::Gating { head, asked: collected.into_boxed() };
                return Stepped { change, decision: Decision::Delegate(Delegate::Gate { number: gate.number, head }) };
            }
            Status::Unknown | Status::Pending if gate.blocking => waiting_gate = true,
            Status::Failed | Status::Passed | Status::Unknown | Status::Pending => {}
        }
    }
    if !ci_passed || waiting_gate {
        return waiting(change, now, limits);
    }
    let since = match change.ready_since {
        Some(since) => since,
        None => {
            change.ready_since = Some(now);
            now
        }
    };
    if change.owns_turn {
        change.state = State::First { head, ready_since: since };
    } else {
        change.state = State::Queued { head, ready_since: since };
    }
    Stepped { change, decision: Decision::Ready }
}

fn queued(mut change: Change, head: [u8; 32], ready_since: Wall, facts: &Facts) -> Stepped {
    if facts.branch != Some(head) {
        return hold(change, Hold::BranchMoved);
    }
    if facts.ci.head != head || facts.ci.status != Status::Passed || !blocking_gates_pass(&change, facts, head) {
        change.state = State::Gating { head, asked: Box::new([]) };
        change.owns_turn = false;
        return Stepped { change, decision: Decision::None };
    }
    if facts.first {
        change.state = State::First { head, ready_since };
        change.owns_turn = true;
        Stepped { change, decision: Decision::None }
    } else {
        Stepped { change, decision: Decision::Wait { until: None } }
    }
}

fn blocking_gates_pass(change: &Change, facts: &Facts, head: [u8; 32]) -> bool {
    for gate in &change.gates {
        if gate.blocking && verdict(change, *gate, facts, head) != Status::Passed {
            return false;
        }
    }
    true
}

fn first(mut change: Change, head: [u8; 32], ready_since: Wall, facts: &Facts, now: Wall, limits: &Limits) -> Stepped {
    if facts.branch != Some(head) {
        return hold(change, Hold::BranchMoved);
    }
    if facts.ci.head != head || facts.ci.status != Status::Passed || !blocking_gates_pass(&change, facts, head) {
        change.state = State::Gating { head, asked: Box::new([]) };
        change.owns_turn = false;
        return Stepped { change, decision: Decision::None };
    }
    if facts.base_ci.head == facts.base_tip && facts.base_ci.status == Status::Failed {
        return if facts.queue_repair_active {
            Stepped { change, decision: Decision::Wait { until: None } }
        } else {
            Stepped { change, decision: Decision::QueueRepair }
        };
    }
    match facts.contains_base {
        Status::Failed => return update_due(change, head, facts.base_tip, ready_since, now, limits),
        Status::Unknown | Status::Pending => return waiting(change, now, limits),
        Status::Passed => {}
    }
    match facts.mergeable {
        Status::Passed => {
            if !facts.may_merge {
                return Stepped { change, decision: Decision::Wait { until: None } };
            }
            change.state = State::Landing { head };
            Stepped { change, decision: Decision::Effect(Effect::Merge { head, base: facts.expected_base }) }
        }
        Status::Failed => resolving_due(change, facts.base_tip, now, limits),
        Status::Unknown | Status::Pending => waiting(change, now, limits),
    }
}

fn updating(
    mut change: Change,
    from: [u8; 32],
    base: [u8; 32],
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    match standing.effect {
        EffectResult::Conflict => resolving_due(change, base, now, limits),
        EffectResult::Failed => hold(change, Hold::Failed),
        EffectResult::Made => {
            let Some(head) = facts.branch else { return waiting(change, now, limits) };
            if head == from {
                return waiting(change, now, limits);
            }
            if change.clean.len() >= usize::try_from(limits.clean_heads).expect("u32 fits usize") {
                return hold(change, Hold::Updates);
            }
            let mut clean = List::with_capacity(limits.clean_heads);
            for predecessor in &change.clean {
                clean.push(*predecessor).expect("clean history admitted");
            }
            clean.push(from).expect("clean history has room");
            change.clean = clean.into_boxed();
            change.state = State::Checking { head };
            change.last_head = Some(head);
            change.since = now;
            Stepped { change, decision: Decision::None }
        }
        EffectResult::None | EffectResult::Pending => waiting(change, now, limits),
    }
}

fn repair_due(mut change: Change, why: Repair, facts: &Facts, now: Wall, limits: &Limits) -> Stepped {
    match facts.contains_base {
        Status::Failed => match facts.branch {
            Some(head) => {
                let ready_since = change.ready_since.unwrap_or(now);
                return update_due(change, head, facts.base_tip, ready_since, now, limits);
            }
            None => return hold(change, Hold::BranchMissing),
        },
        Status::Unknown | Status::Pending => return waiting(change, now, limits),
        Status::Passed => {}
    }
    if change.repairs >= limits.repairs {
        return hold(change, Hold::Repairs);
    }
    change.repairs = change.repairs.checked_add(1).expect("repair count below limit");
    change.owns_turn = false;
    change.clean = Box::new([]);
    change.state = State::Repairing { requested: true, why };
    change.since = now;
    Stepped { change, decision: Decision::Delegate(Delegate::Repair(why)) }
}

fn update_due(
    mut change: Change,
    head: [u8; 32],
    base: [u8; 32],
    ready_since: Wall,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    if change.updates >= limits.updates {
        return hold(change, Hold::Updates);
    }
    change.updates = change.updates.checked_add(1).expect("update count below limit");
    change.state = State::Updating { from: head, base, ready_since };
    change.since = now;
    Stepped { change, decision: Decision::Effect(Effect::Update { head, base }) }
}

fn resolving_due(mut change: Change, base: [u8; 32], now: Wall, limits: &Limits) -> Stepped {
    if change.resolutions >= limits.resolutions {
        return hold(change, Hold::Resolutions);
    }
    change.resolutions = change.resolutions.checked_add(1).expect("resolution count below limit");
    change.owns_turn = false;
    change.clean = Box::new([]);
    change.state = State::Resolving { requested: true, base };
    change.since = now;
    Stepped { change, decision: Decision::Delegate(Delegate::Resolve { base }) }
}

fn repairing(
    change: Change,
    requested: bool,
    why: Repair,
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    delegated(change, requested, Delegate::Repair(why), facts, standing, now, limits)
}

fn resolving(
    change: Change,
    requested: bool,
    base: [u8; 32],
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    delegated(change, requested, Delegate::Resolve { base }, facts, standing, now, limits)
}

fn delegated(
    mut change: Change,
    requested: bool,
    delegate: Delegate,
    facts: &Facts,
    standing: &Heard,
    now: Wall,
    limits: &Limits,
) -> Stepped {
    if !requested {
        change.state = match delegate {
            Delegate::Repair(why) => State::Repairing { requested: true, why },
            Delegate::Resolve { base } => State::Resolving { requested: true, base },
            Delegate::Produce | Delegate::Gate { .. } => unreachable!("delegated phase has one kind"),
        };
        return Stepped { change, decision: Decision::Delegate(delegate) };
    }
    match standing.delegate {
        Status::Passed => {
            let Some(head) = facts.branch else { return waiting(change, now, limits) };
            change.clean = Box::new([]);
            change.owns_turn = false;
            change.state = State::Checking { head };
            change.last_head = Some(head);
            change.since = now;
            Stepped { change, decision: Decision::None }
        }
        Status::Failed => hold(change, Hold::Failed),
        Status::Unknown | Status::Pending => waiting(change, now, limits),
    }
}

fn landing(mut change: Change, head: [u8; 32], standing: &Heard, now: Wall, limits: &Limits) -> Stepped {
    match standing.effect {
        EffectResult::Failed | EffectResult::Conflict => {
            change.state = State::First { head, ready_since: change.ready_since.unwrap_or(now) };
            Stepped { change, decision: Decision::None }
        }
        EffectResult::None | EffectResult::Pending | EffectResult::Made => waiting(change, now, limits),
    }
}
