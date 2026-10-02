//! The checks: what each rule reads, and what it answers.
//!
//! | Rule | Checked on | Reads | Answers |
//! |---|---|---|---|
//! | writes go only to the deployment's repositories | a push granted, every write, a person's request | the repository | refuse: `Elsewhere` |
//! | only a landing reaches a protected branch | a push granted, a branch deleted | the branch | refuse: `Protected` |
//! | green CI on the exact head | a landing on a protected branch, or gated `Ci` | CI and the head it ran on | refuse: `Unchecked` |
//! | an approving review on the exact head | a landing on a protected branch (at the rules' reviewer permission), or gated `Review` (at its own) | each reviewer's latest review, its head and their permission | a request for changes: refuse, `ChangesRequested`; no approval: accept, `Unreviewed` |
//! | plans past a size or an estimate | a plan's items created | steps, estimate | accept: `PlanSize`, `PlanSpend` |
//! | notes wider than a goal | a note written | its scope | accept, where configured: `WideNote` |
//! | spending bounded per run, per goal, per deployment | a run started (its budget on top of what was spent); a plan's items created (its estimate, per goal) | spend so far | refuse: `Overspent` |
//! | a plan's own bounds | a run started (gated `Spend`, `GoalSpend`); a plan's items created (gated `GoalSpend`) | spend so far | refuse: `Overspent` |
//! | read-only steps | a push granted, a pull request opened, a landing, a branch deleted (gated `ReadOnly`) | the gates | refuse: `ReadOnly` |
//! | acts follow the person's permission | a person's request | their permission on the repository | refuse: `Unpermitted` |
//! | the limits | every check, first | sizes | refuse: `Oversized`, nothing else checked |
//!
//! A gate only ever adds a finding, so a check with gates answers at least as
//! strictly as one without, and finds everything that one finds.

use temper_lib::Queue;

use crate::boundary::{
    Act, Bound, Ci, Decision, Finding, Gate, Grant, Landing, Oversized, Permission, Repository, Request, Run, Scope,
    Stance, Write,
};
use crate::limits::{Limits, within};
use crate::rules::Rules;

/// Checks the run the engine is about to start, under the step's `gates`:
/// writes into `out` why it is not simply allowed, and answers the strictest.
/// `out` has room for [`max_out`](crate::max_out) findings.
pub fn check_run(rules: &Rules, limits: &Limits, run: &Run, gates: &[Gate], out: &mut Queue<Finding>) -> Decision {
    if let Some(oversized) = oversized_run(limits, run, gates) {
        return refuse(out, Finding::Oversized(oversized));
    }
    let mut decision = Decision::Allow;
    let read_only = is_read_only(gates);
    let mut pushes = false;
    for grant in &run.grants {
        match grant {
            Grant::Read { .. } => {}
            Grant::Push { repository, branch } => {
                pushes = true;
                if !ours(rules, *repository) {
                    find(&mut decision, out, Finding::Elsewhere);
                } else if protected(rules, *repository, branch) {
                    find(&mut decision, out, Finding::Protected);
                }
            }
        }
    }
    if read_only && pushes {
        find(&mut decision, out, Finding::ReadOnly);
    }
    let goal_spent = run.goal_spent.unwrap_or(0);
    if run.budget > rules.run_spend {
        find(&mut decision, out, Finding::Overspent(Bound::Run));
    }
    if run.goal_spent.is_some() && over(goal_spent, run.budget, rules.goal_spend) {
        find(&mut decision, out, Finding::Overspent(Bound::Goal));
    }
    if over(run.deployment_spent, run.budget, rules.deployment_spend) {
        find(&mut decision, out, Finding::Overspent(Bound::Deployment));
    }
    for gate in gates {
        match gate {
            Gate::Spend { most } => {
                if run.budget > *most {
                    find(&mut decision, out, Finding::Overspent(Bound::Step));
                }
            }
            Gate::GoalSpend { most } => {
                if over(goal_spent, run.budget, *most) {
                    find(&mut decision, out, Finding::Overspent(Bound::Plan));
                }
            }
            Gate::ReadOnly | Gate::Ci | Gate::Review { .. } => {}
        }
    }
    decision
}

/// Checks the write the engine is about to make, under the `gates` of the
/// step it is for: writes into `out` why it is not simply allowed, and
/// answers the strictest. `out` has room for [`max_out`](crate::max_out)
/// findings.
pub fn check_write(
    rules: &Rules,
    limits: &Limits,
    write: &Write,
    gates: &[Gate],
    out: &mut Queue<Finding>,
) -> Decision {
    if let Some(oversized) = oversized_write(limits, write, gates) {
        return refuse(out, Finding::Oversized(oversized));
    }
    let mut decision = Decision::Allow;
    let read_only = is_read_only(gates);
    match write {
        Write::Item { repository } => {
            if !ours(rules, *repository) {
                find(&mut decision, out, Finding::Elsewhere);
            }
        }
        Write::Open { repository, .. } => {
            if !ours(rules, *repository) {
                find(&mut decision, out, Finding::Elsewhere);
            }
            if read_only {
                find(&mut decision, out, Finding::ReadOnly);
            }
        }
        Write::Land(landing) => land(rules, landing, gates, &mut decision, out),
        Write::Delete { repository, branch } => {
            if !ours(rules, *repository) {
                find(&mut decision, out, Finding::Elsewhere);
            } else if protected(rules, *repository, branch) {
                find(&mut decision, out, Finding::Protected);
            }
            if read_only {
                find(&mut decision, out, Finding::ReadOnly);
            }
        }
        Write::Plan { repository, steps, spend } => {
            if !ours(rules, *repository) {
                find(&mut decision, out, Finding::Elsewhere);
            }
            if *steps > rules.plan_steps {
                find(&mut decision, out, Finding::PlanSize { permission: rules.plan_acceptance });
            }
            if *spend > rules.plan_spend {
                find(&mut decision, out, Finding::PlanSpend { permission: rules.plan_acceptance });
            }
            if *spend > rules.goal_spend {
                find(&mut decision, out, Finding::Overspent(Bound::Goal));
            }
            for gate in gates {
                match gate {
                    Gate::GoalSpend { most } => {
                        if *spend > *most {
                            find(&mut decision, out, Finding::Overspent(Bound::Plan));
                        }
                    }
                    Gate::ReadOnly | Gate::Ci | Gate::Review { .. } | Gate::Spend { .. } => {}
                }
            }
        }
        Write::Note { repository, scope } => {
            if !ours(rules, *repository) {
                find(&mut decision, out, Finding::Elsewhere);
            }
            let wanted = match scope {
                Scope::Goal => None,
                Scope::Repository => rules.repository_notes,
                Scope::Deployment => rules.deployment_notes,
            };
            if let Some(permission) = wanted {
                find(&mut decision, out, Finding::WideNote { permission });
            }
        }
    }
    decision
}

/// Checks a person's request: writes into `out` why it is not simply allowed,
/// and answers the strictest. `out` has room for [`max_out`](crate::max_out)
/// findings.
pub fn check_request(rules: &Rules, request: &Request, out: &mut Queue<Finding>) -> Decision {
    let mut decision = Decision::Allow;
    if !ours(rules, request.repository) {
        find(&mut decision, out, Finding::Elsewhere);
    }
    let acts = &rules.acts;
    let needs = match request.act {
        Act::Open => acts.open,
        Act::Steer => acts.steer,
        Act::Accept { permission } | Act::Reject { permission } => acts.accept.max(permission),
        Act::Cancel => acts.cancel,
        Act::Release => acts.release,
        Act::Watch => acts.watch,
    };
    if request.permission < needs {
        find(&mut decision, out, Finding::Unpermitted { needs, has: request.permission });
    }
    decision
}

/// A landing: on a deployment's repository, never by a read-only step; on a
/// protected branch, or where gated, only with green CI on the exact head and
/// an approving review there that nobody outweighs.
fn land(rules: &Rules, landing: &Landing, gates: &[Gate], decision: &mut Decision, out: &mut Queue<Finding>) {
    if !ours(rules, landing.repository) {
        find(decision, out, Finding::Elsewhere);
    }
    let mut checked = protected(rules, landing.repository, &landing.base);
    for gate in gates {
        match gate {
            Gate::Ci => checked = true,
            Gate::ReadOnly | Gate::Review { .. } | Gate::Spend { .. } | Gate::GoalSpend { .. } => {}
        }
    }
    if is_read_only(gates) {
        find(decision, out, Finding::ReadOnly);
    }
    if checked {
        let ci = if landing.ci_head == landing.head { landing.ci } else { Ci::None };
        match ci {
            Ci::Passed => {}
            Ci::None | Ci::Pending | Ci::Failed => find(decision, out, Finding::Unchecked { ci }),
        }
    }
    if protected(rules, landing.repository, &landing.base)
        && let Some(finding) = reviewed(rules, landing, rules.reviewer)
    {
        find(decision, out, finding);
    }
    for gate in gates {
        match gate {
            Gate::Review { permission } => {
                if let Some(finding) = reviewed(rules, landing, *permission) {
                    find(decision, out, finding);
                }
            }
            Gate::ReadOnly | Gate::Ci | Gate::Spend { .. } | Gate::GoalSpend { .. } => {}
        }
    }
}

/// What the reviews on a landing's exact head by people with at least
/// `permission` (the engine's own aside) say against it: a request for
/// changes, or no approval.
fn reviewed(rules: &Rules, landing: &Landing, permission: Permission) -> Option<Finding> {
    let mut approved = false;
    for review in &landing.reviews {
        if review.person == rules.engine || review.head != landing.head || review.permission < permission {
            continue;
        }
        match review.stance {
            Stance::RequestChanges => return Some(Finding::ChangesRequested { permission }),
            Stance::Approve => approved = true,
        }
    }
    if approved { None } else { Some(Finding::Unreviewed { permission }) }
}

/// What of a run, or of its gates, is more than the limits let in.
fn oversized_run(limits: &Limits, run: &Run, gates: &[Gate]) -> Option<Oversized> {
    if !within(gates.len(), limits.gates) {
        return Some(Oversized::Gates);
    }
    if !within(run.grants.len(), limits.grants) {
        return Some(Oversized::Grants);
    }
    for grant in &run.grants {
        match grant {
            Grant::Read { .. } => {}
            Grant::Push { branch, .. } => {
                if !within(branch.len(), limits.branch_bytes) {
                    return Some(Oversized::Branch);
                }
            }
        }
    }
    None
}

/// What of a write, or of its gates, is more than the limits let in.
fn oversized_write(limits: &Limits, write: &Write, gates: &[Gate]) -> Option<Oversized> {
    if !within(gates.len(), limits.gates) {
        return Some(Oversized::Gates);
    }
    let branch = match write {
        Write::Item { .. } | Write::Plan { .. } | Write::Note { .. } => None,
        Write::Open { base, .. } => Some(base),
        Write::Delete { branch, .. } => Some(branch),
        Write::Land(landing) => {
            if !within(landing.reviews.len(), limits.reviews) {
                return Some(Oversized::Reviews);
            }
            Some(&landing.base)
        }
    };
    if let Some(branch) = branch
        && !within(branch.len(), limits.branch_bytes)
    {
        return Some(Oversized::Branch);
    }
    None
}

/// Whether `repository` is one of the deployment's.
fn ours(rules: &Rules, repository: Repository) -> bool {
    match repository {
        Repository::Deployment(index) => index < rules.repositories,
        Repository::Elsewhere => false,
    }
}

/// Whether `branch` of `repository` is protected.
fn protected(rules: &Rules, repository: Repository, branch: &[u8]) -> bool {
    let index = match repository {
        Repository::Deployment(index) => index,
        Repository::Elsewhere => return false,
    };
    for protected in &rules.protected {
        if protected.repository == index && *protected.name == *branch {
            return true;
        }
    }
    false
}

/// Whether a gate makes the step read-only.
fn is_read_only(gates: &[Gate]) -> bool {
    for gate in gates {
        match gate {
            Gate::ReadOnly => return true,
            Gate::Ci | Gate::Review { .. } | Gate::Spend { .. } | Gate::GoalSpend { .. } => {}
        }
    }
    false
}

/// Whether spending `more` on top of `spent` passes `most`.
fn over(spent: u64, more: u64, most: u64) -> bool {
    match spent.checked_add(more) {
        Some(total) => total > most,
        None => true,
    }
}

/// Writes `finding` into `out`, and makes `decision` as strict as it.
fn find(decision: &mut Decision, out: &mut Queue<Finding>, finding: Finding) {
    *decision = (*decision).max(finding.decision());
    out.push(finding);
}

/// Refuses for `finding` alone.
fn refuse(out: &mut Queue<Finding>, finding: Finding) -> Decision {
    let mut decision = Decision::Allow;
    find(&mut decision, out, finding);
    decision
}
