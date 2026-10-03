//! The checks: what each rule reads, and what it answers.
//!
//! | Rule | Checked on | Reads | Answers |
//! |---|---|---|---|
//! | writes go only to the deployment's repositories | a push granted, every write and a plan's targets, a person's request | the repository | refuse: `Elsewhere` |
//! | only a landing reaches a protected branch | a push granted, a branch deleted | the branch | refuse: `Protected` |
//! | green CI on the exact head | a landing on a protected branch, or gated `Ci` | CI and the head it ran on | wait while it is to come, refuse once failed: `Unchecked` |
//! | an approving review on the exact head | a landing on a protected branch (at the rules' reviewer permission), or gated `Review` (at its own); write at least | each reviewer's latest review, its head and their permission | no approval: wait, `Unreviewed`; a request for changes standing, on any head: refuse, `ChangesRequested` |
//! | approvals from enough people | a landing gated `Approvals` | approving reviews on the exact head, by distinct people with write at least | wait: `Unapproved` |
//! | a person accepts the step | a run or a write gated `Accepted` | who accepted, if anyone did | accept, at the permission accepting needs: `Unaccepted` |
//! | plans past a size or an estimate, or landing on a protected branch | a plan's items created | steps, estimate, targets, who accepted | accept: `PlanSize`, `PlanSpend`, `PlanLands` |
//! | notes wider than a goal | a note written | its scope, who accepted | accept, where configured: `WideNote` |
//! | spending bounded per run, per goal, per deployment | a run started (its budget on top of what was spent); a plan's items created (its estimate on top of its goal's spend) | spend so far | refuse: `Overspent` |
//! | a plan's own bounds | a run started (gated `Spend`, `GoalSpend`); a plan's items created (gated `GoalSpend`) | spend so far | refuse: `Overspent` |
//! | read-only steps | a push granted, a pull request opened, a landing, a branch deleted (gated `ReadOnly`) | the gates | refuse: `ReadOnly` |
//! | acts follow the person's permission | a person's request | their permission on the repository | refuse: `Unpermitted` |
//! | the limits | every check, first | sizes | refuse: `Oversized`, nothing else checked |
//!
//! An acceptance clears only what answers accept: a check that waits asks
//! for facts the forge has to report, a review or CI on the exact head,
//! which no person's acceptance stands in for. A gate only ever adds a
//! finding, so a check with gates answers at least as strictly as one
//! without, and finds everything that one finds.

use temper_lib::Queue;

use crate::boundary::{
    Act, Bound, Ci, Decision, Finding, Gate, Goal, Grant, Landing, Oversized, Permission, Plan, Repository, Request,
    Run, Scope, Stance, Write,
};
use crate::limits::{Limits, within};
use crate::rules::Rules;

/// The least permission a review counts at, whatever the rules or a gate
/// say: a reader's review never counts.
const REVIEWER: Permission = Permission::Write;

/// Checks the run the engine is about to start, under the step's `gates`,
/// `accepted` being the permission of the person who accepted the step, if
/// one did: writes into `out` why it is not simply allowed, and answers the
/// strictest. `out` has room for [`max_out`](crate::max_out) findings.
pub fn check_run(
    rules: &Rules,
    limits: &Limits,
    run: &Run,
    accepted: Option<Permission>,
    gates: &[Gate],
    out: &mut Queue<Finding>,
) -> Decision {
    if let Some(oversized) = oversized_run(limits, run, gates) {
        return refuse(out, Finding::Oversized(oversized));
    }
    let mut decision = Decision::Allow;
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
    if pushes && is_read_only(gates) {
        find(&mut decision, out, Finding::ReadOnly);
    }
    if run.budget > rules.run_spend {
        find(&mut decision, out, Finding::Overspent(Bound::Run));
    }
    let goal_spent = match run.goal {
        Goal::Outside => None,
        Goal::Spent(spent) => Some(spent),
    };
    if let Some(spent) = goal_spent
        && over(spent, run.budget, rules.goal_spend)
    {
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
                if over(goal_spent.unwrap_or(0), run.budget, *most) {
                    find(&mut decision, out, Finding::Overspent(Bound::Plan));
                }
            }
            Gate::ReadOnly | Gate::Ci | Gate::Review { .. } | Gate::Approvals(_) | Gate::Accepted => {}
        }
    }
    unaccepted(rules, accepted, gates, &mut decision, out);
    decision
}

/// Checks the write the engine is about to make, under the `gates` of the
/// step it is for, `accepted` being the permission of the person who
/// accepted the step or the proposal it applies, if one did: writes into
/// `out` why it is not simply allowed, and answers the strictest. `out` has
/// room for [`max_out`](crate::max_out) findings.
pub fn check_write(
    rules: &Rules,
    limits: &Limits,
    write: &Write,
    accepted: Option<Permission>,
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
        Write::Plan(plan) => grow(rules, plan, accepted, gates, &mut decision, out),
        Write::Note { repository, scope } => {
            if !ours(rules, *repository) {
                find(&mut decision, out, Finding::Elsewhere);
            }
            let wanted = match scope {
                Scope::Goal => None,
                Scope::Repository => rules.repository_notes,
                Scope::Deployment => rules.deployment_notes,
            };
            if let Some(permission) = wanted
                && accepted < Some(permission)
            {
                find(&mut decision, out, Finding::WideNote { permission });
            }
        }
    }
    unaccepted(rules, accepted, gates, &mut decision, out);
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
/// protected branch, or where gated, only with green CI on the exact head,
/// approving reviews there and no request for changes standing.
fn land(rules: &Rules, landing: &Landing, gates: &[Gate], decision: &mut Decision, out: &mut Queue<Finding>) {
    if !ours(rules, landing.repository) {
        find(decision, out, Finding::Elsewhere);
    }
    let protected = protected(rules, landing.repository, &landing.base);
    let mut checked = protected;
    for gate in gates {
        match gate {
            Gate::Ci => checked = true,
            Gate::ReadOnly
            | Gate::Review { .. }
            | Gate::Approvals(_)
            | Gate::Accepted
            | Gate::Spend { .. }
            | Gate::GoalSpend { .. } => {}
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
    if protected && let Some(finding) = reviewed(rules, landing, rules.reviewer) {
        find(decision, out, finding);
    }
    for gate in gates {
        match gate {
            Gate::Review { permission } => {
                if let Some(finding) = reviewed(rules, landing, *permission) {
                    find(decision, out, finding);
                }
            }
            Gate::Approvals(want) => {
                let have = approvals(rules, landing);
                if have < *want {
                    find(decision, out, Finding::Unapproved { have, want: *want });
                }
            }
            Gate::ReadOnly | Gate::Ci | Gate::Accepted | Gate::Spend { .. } | Gate::GoalSpend { .. } => {}
        }
    }
}

/// What the reviews on a landing by people with at least `permission`, and
/// write at least (the engine's own aside), say against it: a request for
/// changes, standing until the same reviewer approves, whatever head it was
/// on; or no approval on the exact head.
fn reviewed(rules: &Rules, landing: &Landing, permission: Permission) -> Option<Finding> {
    let permission = permission.max(REVIEWER);
    let mut approved = false;
    for review in &landing.reviews {
        if review.person == rules.engine || review.permission < permission {
            continue;
        }
        match review.stance {
            Stance::RequestChanges => return Some(Finding::ChangesRequested { permission }),
            Stance::Approve => approved |= review.head == landing.head,
        }
    }
    if approved { None } else { Some(Finding::Unreviewed { permission }) }
}

/// How many people, with write at least (the engine aside), approve a
/// landing's exact head.
fn approvals(rules: &Rules, landing: &Landing) -> u32 {
    let mut people: u32 = 0;
    for (at, review) in landing.reviews.iter().enumerate() {
        let counts = review.person != rules.engine
            && review.permission >= REVIEWER
            && review.head == landing.head
            && review.stance == Stance::Approve;
        if !counts {
            continue;
        }
        // Each person once, should the forge list them twice.
        let earlier = landing.reviews.get(..at).unwrap_or(&[]);
        let mut again = false;
        for before in earlier {
            again |= before.person == review.person;
        }
        if !again {
            people = people.saturating_add(1);
        }
    }
    people
}

/// A plan's items created: in the deployment's repositories, landing in
/// them; past a size, an estimate, or onto a protected branch, only once a
/// person accepted it; and within its goal's bounds.
fn grow(
    rules: &Rules,
    plan: &Plan,
    accepted: Option<Permission>,
    gates: &[Gate],
    decision: &mut Decision,
    out: &mut Queue<Finding>,
) {
    let mut elsewhere = !ours(rules, plan.repository);
    let mut lands = false;
    for target in &plan.lands {
        elsewhere |= !ours(rules, target.repository);
        lands |= protected(rules, target.repository, &target.branch);
    }
    if elsewhere {
        find(decision, out, Finding::Elsewhere);
    }
    let permission = rules.plan_acceptance;
    let accepted = accepted >= Some(permission);
    if plan.steps > rules.plan_steps && !accepted {
        find(decision, out, Finding::PlanSize { permission });
    }
    if plan.spend > rules.plan_spend && !accepted {
        find(decision, out, Finding::PlanSpend { permission });
    }
    if lands && !accepted {
        find(decision, out, Finding::PlanLands { permission });
    }
    let spent = match plan.goal {
        Goal::Outside => 0,
        Goal::Spent(spent) => spent,
    };
    if over(spent, plan.spend, rules.goal_spend) {
        find(decision, out, Finding::Overspent(Bound::Goal));
    }
    for gate in gates {
        match gate {
            Gate::GoalSpend { most } => {
                if over(spent, plan.spend, *most) {
                    find(decision, out, Finding::Overspent(Bound::Plan));
                }
            }
            Gate::ReadOnly
            | Gate::Ci
            | Gate::Review { .. }
            | Gate::Approvals(_)
            | Gate::Accepted
            | Gate::Spend { .. } => {}
        }
    }
}

/// A step gated `Accepted` waits for a person's acceptance at the permission
/// accepting needs, unless one gave it.
fn unaccepted(
    rules: &Rules,
    accepted: Option<Permission>,
    gates: &[Gate],
    decision: &mut Decision,
    out: &mut Queue<Finding>,
) {
    let mut wanted = false;
    for gate in gates {
        match gate {
            Gate::Accepted => wanted = true,
            Gate::ReadOnly
            | Gate::Ci
            | Gate::Review { .. }
            | Gate::Approvals(_)
            | Gate::Spend { .. }
            | Gate::GoalSpend { .. } => {}
        }
    }
    let permission = rules.acts.accept;
    if wanted && accepted < Some(permission) {
        find(decision, out, Finding::Unaccepted { permission });
    }
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
        Write::Item { .. } | Write::Note { .. } => None,
        Write::Open { base, .. } => Some(base),
        Write::Delete { branch, .. } => Some(branch),
        Write::Land(landing) => {
            if !within(landing.reviews.len(), limits.reviews) {
                return Some(Oversized::Reviews);
            }
            Some(&landing.base)
        }
        Write::Plan(plan) => {
            if !within(plan.lands.len(), limits.lands) {
                return Some(Oversized::Lands);
            }
            for target in &plan.lands {
                if !within(target.branch.len(), limits.branch_bytes) {
                    return Some(Oversized::Branch);
                }
            }
            None
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
            Gate::Ci
            | Gate::Review { .. }
            | Gate::Approvals(_)
            | Gate::Accepted
            | Gate::Spend { .. }
            | Gate::GoalSpend { .. } => {}
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
