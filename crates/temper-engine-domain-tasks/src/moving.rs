//! Requester-tree moves and bottom-up normalization of the actual funding
//! graph, independent of requester links. All checks precede the first write.
use crate::domain::{Domain, entrance, publish, record, refused, task_mut};
use crate::{Authorization, Balance, Change, Funder, Limits, Numbers, Party, Refusal, Request};
use alloc::boxed::Box;
use skein_lib::{Env, List, Queue, ReplyTo};
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Transfer {
    pub task: u64,
    pub before: Funder,
    pub after: Funder,
}
/// The root verifies role/move rights and authority on replacement reservations
/// against these exact authentic balances before sending this event.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Movement {
    pub to: Party,
    pub transfers: Box<[Transfer]>,
    pub balances: Box<[Balance]>,
    pub reason: Box<[u8]>,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Normalized {
    task: u64,
    closed: Numbers,
    replacement: Numbers,
    funder: Funder,
}
fn transfer(transfers: &[Transfer], number: u64, old: Funder) -> Funder {
    for transfer in transfers {
        if transfer.task == number {
            return transfer.after;
        }
    }
    old
}
#[expect(clippy::manual_find, reason = "step code uses bounded iteration without closures")]
fn normalized(nodes: &List<Normalized>, number: u64) -> Option<&Normalized> {
    for node in nodes {
        if node.task == number {
            return Some(node);
        }
    }
    None
}
fn affected(d: &Domain, transfers: &[Transfer], number: u64, bound: u32) -> bool {
    let mut at = Some(number);
    for _ in 0..bound {
        let Some(number) = at else {
            return false;
        };
        let Some(task) = record(d, number) else {
            return false;
        };
        for transfer in transfers {
            if transfer.task == number && transfer.after != transfer.before {
                return true;
            }
        }
        at = match task.funder {
            Funder::Task(parent) => Some(parent),
            Funder::Pool { .. } | Funder::Period { .. } => None,
        };
    }
    false
}
fn funding_links(d: &Domain, project: u32, transfers: &[Transfer], bound: u32) -> bool {
    for (number, _) in &d.names {
        let task = record(d, *number).expect("live name");
        if task.project != project {
            continue;
        }
        let mut at = Some(*number);
        for hop in 0..bound {
            let Some(current) = at else {
                break;
            };
            let Some(node) = record(d, current) else {
                return false;
            };
            if node.project != project {
                return false;
            }
            at = match transfer(transfers, current, node.funder) {
                Funder::Task(parent) => {
                    if parent == *number || hop.saturating_add(1) == bound {
                        return false;
                    }
                    Some(parent)
                }
                Funder::Pool { project: other, .. } | Funder::Period { project: other, .. } => {
                    if other != project {
                        return false;
                    }
                    None
                }
            };
        }
    }
    true
}
fn normalize(d: &Domain, l: &Limits, number: u64, movement: &Movement) -> Result<List<Normalized>, Refusal> {
    let project = record(d, number).expect("target live").project;
    // Validate original links as well as their proposed replacements before
    // following them, and reject cycles in either graph.
    if !funding_links(d, project, &[], l.tasks) || !funding_links(d, project, &movement.transfers, l.tasks) {
        return Err(Refusal::Funding);
    }
    for (at, transfer) in movement.transfers.iter().enumerate() {
        for earlier in movement.transfers.iter().take(at) {
            if earlier.task == transfer.task {
                return Err(Refusal::Duplicate);
            }
        }
        let Some(task) = record(d, transfer.task) else {
            return Err(Refusal::Unknown);
        };
        if !crate::amend::below(d, transfer.task, number, l.tasks) || task.funder != transfer.before {
            return Err(Refusal::Funding);
        }
    }
    let mut wanted = 0_u32;
    for (task, _) in &d.names {
        if affected(d, &movement.transfers, *task, l.tasks) {
            if !crate::amend::below(d, *task, number, l.tasks) {
                return Err(Refusal::Funding);
            }
            wanted = wanted.saturating_add(1);
        }
    }
    let mut nodes = List::with_capacity(l.tasks);
    // Bounded leaf removal in the actual old funding graph.
    for _ in 0..l.tasks {
        for (task, _) in &d.names {
            if !affected(d, &movement.transfers, *task, l.tasks) || normalized(&nodes, *task).is_some() {
                continue;
            }
            let old = record(d, *task).expect("live name");
            if old.allotment == u64::MAX || old.revision == u64::MAX {
                return Err(Refusal::Revision);
            }
            let mut closed = old.numbers;
            let mut replacement_reserved = 0_u64;
            let mut waiting = false;
            for (child, _) in &d.names {
                let child_task = record(d, *child).expect("live name");
                if child_task.funder == Funder::Task(*task) {
                    match normalized(&nodes, *child) {
                        Some(node) => {
                            closed.reserved =
                                closed.reserved.checked_sub(child_task.numbers.budget).ok_or(Refusal::Funding)?;
                            closed.spent_below = closed
                                .spent_below
                                .checked_add(crate::funders::total(node.closed).ok_or(Refusal::Funding)?)
                                .ok_or(Refusal::Funding)?;
                        }
                        None => {
                            waiting = true;
                        }
                    }
                }
            }
            if waiting {
                continue;
            }
            if closed.reserved != 0 {
                return Err(Refusal::Funding);
            }
            let retained = crate::funders::remaining(closed).ok_or(Refusal::Funding)?;
            old.historical_spend
                .checked_add(crate::funders::total(closed).ok_or(Refusal::Funding)?)
                .ok_or(Refusal::Funding)?;
            // Sum every child which will actually be funded here afterwards;
            // changed descendants can detach to separate external components.
            for node in &nodes {
                if node.funder == Funder::Task(*task) {
                    replacement_reserved =
                        replacement_reserved.checked_add(node.replacement.budget).ok_or(Refusal::Funding)?;
                }
            }
            if replacement_reserved > retained {
                return Err(Refusal::Funding);
            }
            nodes
                .push(Normalized {
                    task: *task,
                    closed,
                    replacement: Numbers { budget: retained, spent: 0, spent_below: 0, reserved: replacement_reserved },
                    funder: transfer(&movement.transfers, *task, old.funder),
                })
                .expect("live nodes bounded");
        }
    }
    if nodes.len() != wanted {
        return Err(Refusal::Funding);
    }
    complete_reservations(l, &nodes)
}
fn complete_reservations(l: &Limits, nodes: &List<Normalized>) -> Result<List<Normalized>, Refusal> {
    // Transfers into a node normalized before its newly funded child was
    // visited need the complete final reservation, independently of old order.
    let mut complete = List::with_capacity(l.tasks);
    for node in nodes {
        let mut replacement = node.replacement;
        replacement.reserved = 0;
        for child in nodes {
            if child.funder == Funder::Task(node.task) {
                replacement.reserved =
                    replacement.reserved.checked_add(child.replacement.budget).ok_or(Refusal::Funding)?;
            }
        }
        if crate::funders::available(replacement).is_none() {
            return Err(Refusal::Funding);
        }
        complete.push(Normalized { replacement, ..*node }).expect("live nodes bounded");
    }
    Ok(complete)
}
fn balances(d: &Domain, movement: &Movement, nodes: &List<Normalized>, project: u32, bound: u32) -> bool {
    if !crate::funders::validate_balances(d, project, &movement.balances, bound.saturating_mul(2)) {
        return false;
    }
    for balance in &movement.balances {
        match balance.funder {
            Funder::Task(task) if normalized(nodes, task).is_some() => return false,
            Funder::Task(_) | Funder::Pool { .. } | Funder::Period { .. } => {}
        }
        let mut expected = balance.before;
        for node in nodes {
            let old = record(d, node.task).expect("normalized node live");
            if old.funder == balance.funder {
                let Some(reserved) = expected.reserved.checked_sub(old.numbers.budget) else {
                    return false;
                };
                expected.reserved = reserved;
                let Some(total) = crate::funders::total(node.closed) else {
                    return false;
                };
                let Some(spent) = expected.spent_below.checked_add(total) else {
                    return false;
                };
                expected.spent_below = spent;
            }
            if node.funder == balance.funder {
                let Some(reserved) = expected.reserved.checked_add(node.replacement.budget) else {
                    return false;
                };
                expected.reserved = reserved;
            }
        }
        if expected != balance.after {
            return false;
        }
    }
    for node in nodes {
        let old = record(d, node.task).expect("node live");
        for funder in [old.funder, node.funder] {
            let internal = match funder {
                Funder::Task(task) => normalized(nodes, task).is_some(),
                Funder::Pool { .. } | Funder::Period { .. } => false,
            };
            if internal {
                continue;
            }
            let mut covered = false;
            for balance in &movement.balances {
                if balance.funder == funder {
                    covered = true;
                }
            }
            if !covered {
                return false;
            }
        }
    }
    true
}
fn requester(d: &Domain, number: u64, moved: u64, to: Party) -> Party {
    if number == moved { to } else { record(d, number).expect("live graph name").requester }
}
fn acyclic(d: &Domain, l: &Limits, moved: u64, to: Party) -> bool {
    let mut removed = List::with_capacity(l.tasks);
    for _ in 0..l.tasks {
        let mut progress = false;
        for (number, _) in &d.names {
            if crate::batch::contains(removed.as_slice(), *number) {
                continue;
            }
            let task = record(d, *number).expect("live name");
            let mut ready = true;
            for dependency in &task.dependencies {
                if d.names.contains_key(dependency) && !crate::batch::contains(removed.as_slice(), *dependency) {
                    ready = false;
                }
            }
            for (child, _) in &d.names {
                if requester(d, *child, moved, to) == Party::Task(*number)
                    && !crate::batch::contains(removed.as_slice(), *child)
                {
                    ready = false;
                }
            }
            if ready {
                removed.push(*number).expect("one live graph node");
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    removed.len() == d.names.len()
}
fn append(old: &[u64], number: u64, capacity: u32) -> Box<[u64]> {
    let mut values = List::with_capacity(capacity);
    for old in old {
        values.push(*old).expect("bounded old values");
    }
    if !crate::batch::contains(old, number) {
        values.push(number).expect("append checked");
    }
    values.into_boxed()
}
fn remove(old: &[u64], number: u64, capacity: u32) -> Box<[u64]> {
    let mut values = List::with_capacity(capacity);
    for old in old {
        if *old != number {
            values.push(*old).expect("subset bounded");
        }
    }
    values.into_boxed()
}
pub(crate) fn move_task(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    authorization: Authorization,
    movement: Movement,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    if !crate::amend::standing(d, number, authorization, env.limits.tasks) {
        return refused(to, Some(number), Refusal::Standing, out);
    }
    let nodes = match check(d, &env.limits, number, &movement) {
        Ok(nodes) => nodes,
        Err(why) => return refused(to, Some(number), why, out),
    };
    let old = record(d, number).expect("move validated");
    let from = old.requester;
    let old_depth = old.depth;
    let made = old.made;
    let (root, depth) = match movement.to {
        Party::Task(parent) => {
            let parent = record(d, parent).expect("destination validated");
            (parent.root, parent.depth.saturating_add(1))
        }
        Party::Person(_) | Party::Deployment { .. } => (number, 0),
    };
    let mut selected = List::with_capacity(env.limits.tasks);
    for (child, _) in &d.names {
        if crate::amend::below(d, *child, number, env.limits.tasks) {
            selected.push(*child).expect("subtree bounded");
        }
    }
    let mut old_ancestors = List::with_capacity(env.limits.tasks);
    let mut at = from;
    for _ in 0..env.limits.tasks {
        match at {
            Party::Task(parent) => {
                old_ancestors.push(parent).expect("tree bounded");
                at = record(d, parent).expect("ancestor live").requester;
            }
            Party::Person(_) | Party::Deployment { .. } => break,
        }
    }
    commit_funding(d, env, movement.balances.as_ref(), &nodes, out);
    match from {
        Party::Task(parent) => {
            let old = task_mut(d, parent).expect("old requester live");
            old.record.delegates = remove(&old.record.delegates, number, env.limits.delegates);
            old.record.results_due = remove(&old.record.results_due, number, env.limits.delegates);
            publish(d, env, parent, out);
            let moved = task_mut(d, number).expect("target live");
            moved.record.references = append(&moved.record.references, parent, env.limits.references);
        }
        Party::Person(_) | Party::Deployment { .. } => {}
    }
    match movement.to {
        Party::Task(parent) => {
            let task = task_mut(d, parent).expect("destination live");
            task.record.delegates = append(&task.record.delegates, number, env.limits.delegates);
            task.record.results_due = append(&task.record.results_due, number, env.limits.delegates);
            task.record.references = append(&task.record.references, number, env.limits.references);
            let mut at = Some(parent);
            for _ in 0..env.limits.tasks {
                let Some(parent) = at else {
                    break;
                };
                let task = task_mut(d, parent).expect("ancestor live");
                if !crate::batch::contains(old_ancestors.as_slice(), parent) {
                    task.record.made = task.record.made.checked_add(made).expect("tree capacity checked");
                }
                at = match task.record.requester {
                    Party::Task(parent) => Some(parent),
                    Party::Person(_) | Party::Deployment { .. } => None,
                };
                publish(d, env, parent, out);
            }
        }
        Party::Person(_) | Party::Deployment { .. } => {}
    }
    task_mut(d, number).expect("target live").record.requester = movement.to;
    for child in selected.into_boxed() {
        let task = task_mut(d, child).expect("subtree live");
        task.record.root = root;
        task.record.depth = task
            .record
            .depth
            .checked_sub(old_depth)
            .expect("descendant depth")
            .checked_add(depth)
            .expect("depth checked");
        let previous = if child == number { from } else { record(d, child).expect("subtree live").requester };
        let next = record(d, child).expect("subtree live").requester;
        crate::amend::history(
            d,
            child,
            authorization.party(),
            &movement.reason,
            Change::Moved { from: previous, to: next },
            out,
        );
        publish(d, env, child, out);
    }
    out.push(Request::Done { reply_to: to });
}
fn check(d: &Domain, l: &Limits, number: u64, movement: &Movement) -> Result<List<Normalized>, Refusal> {
    let task = record(d, number).expect("target live");
    if !crate::amend::mutable(&task.phase) || movement.to == task.requester {
        return Err(Refusal::State);
    }
    if movement.transfers.len() > usize::try_from(l.tasks).expect("u32 fits usize")
        || movement.reason.len() > usize::try_from(l.message_bytes).expect("u32 fits usize")
    {
        return Err(Refusal::Reason);
    }
    let depth = destination(d, l, number, movement)?;
    match task.requester {
        Party::Task(parent) => {
            if !crate::batch::contains(&task.references, parent)
                && task.references.len() >= usize::try_from(l.references).expect("u32 fits usize")
            {
                return Err(Refusal::Reference);
            }
        }
        Party::Person(_) | Party::Deployment { .. } => {}
    }
    for (child, _) in &d.names {
        if !crate::amend::below(d, *child, number, l.tasks) {
            continue;
        }
        let child = record(d, *child).expect("live name");
        if !crate::amend::mutable(&child.phase) {
            return Err(Refusal::State);
        }
        if match child.depth.checked_sub(task.depth) {
            Some(relative) => match relative.checked_add(depth) {
                Some(depth) => depth > l.depth,
                None => true,
            },
            None => true,
        } {
            return Err(Refusal::Depth);
        }
        if child.revision == u64::MAX {
            return Err(Refusal::Revision);
        }
        match movement.to {
            Party::Task(_) if child.tracked.is_some() => return Err(Refusal::Tracked),
            Party::Task(_) | Party::Person(_) | Party::Deployment { .. } => {}
        }
        // Every old task funding link entering the requester subtree must be
        // explicitly considered. Stable external periods/pools remain intact.
        match child.funder {
            Funder::Task(parent) if !crate::amend::below(d, parent, number, l.tasks) => {
                let mut explicit = false;
                for transfer in &movement.transfers {
                    if transfer.task == child.number {
                        explicit = true;
                    }
                }
                if !explicit {
                    return Err(Refusal::Funding);
                }
                match transfer(&movement.transfers, child.number, child.funder) {
                    Funder::Task(after) if crate::amend::below(d, number, after, l.tasks) => {
                        let still_above = match movement.to {
                            Party::Task(destination) => crate::amend::below(d, destination, after, l.tasks),
                            Party::Person(_) | Party::Deployment { .. } => false,
                        };
                        if !still_above {
                            return Err(Refusal::Funding);
                        }
                    }
                    Funder::Task(_) | Funder::Pool { .. } | Funder::Period { .. } => {}
                }
            }
            Funder::Task(_) | Funder::Pool { .. } | Funder::Period { .. } => {}
        }
    }
    if !acyclic(d, l, number, movement.to) {
        return Err(Refusal::Cycle);
    }
    let nodes = normalize(d, l, number, movement)?;
    if !balances(d, movement, &nodes, task.project, l.tasks) {
        return Err(Refusal::Funding);
    }
    Ok(nodes)
}

pub(crate) fn worst_case(bound: u32) -> Option<u64> {
    List::<Normalized>::worst_case(bound)?.checked_mul(2)
}

fn commit_funding(
    d: &mut Domain,
    env: &Env<Limits>,
    balances: &[Balance],
    nodes: &List<Normalized>,
    out: &mut Queue<Request>,
) {
    crate::funders::apply_balances(d, env, balances, out);
    for node in nodes {
        let old = record(d, node.task).expect("node live");
        crate::funders::closure(node.task, old.allotment, old.funder, node.closed, out);
        let task = task_mut(d, node.task).expect("node live");
        task.record.allotment = task.record.allotment.checked_add(1).expect("generation checked");
        task.record.historical_spend = task
            .record
            .historical_spend
            .checked_add(crate::funders::total(node.closed).expect("total checked"))
            .expect("history checked");
        task.record.funder = node.funder;
        task.record.numbers = node.replacement;
        publish(d, env, node.task, out);
    }
}

fn destination(d: &Domain, l: &Limits, number: u64, movement: &Movement) -> Result<u32, Refusal> {
    let task = record(d, number).expect("target live");
    let incoming = transfer(&movement.transfers, number, task.funder);
    let destination_funded = match movement.to {
        Party::Person(person) => match incoming {
            Funder::Pool { project, person: owner, .. } => project == task.project && owner == person,
            Funder::Task(_) | Funder::Period { .. } => false,
        },
        Party::Task(parent) => incoming == Funder::Task(parent),
        Party::Deployment { project } => match incoming {
            Funder::Period { project: owner, .. } => owner == project,
            Funder::Task(_) | Funder::Pool { .. } => false,
        },
    };
    if !destination_funded {
        return Err(Refusal::Funding);
    }
    let depth = match movement.to {
        Party::Task(parent) => {
            let Some(parent_task) = record(d, parent) else {
                return Err(Refusal::Unknown);
            };
            if parent_task.project != task.project {
                return Err(Refusal::Project);
            }
            if !crate::amend::mutable(&parent_task.phase) {
                return Err(Refusal::State);
            }
            if crate::amend::below(d, parent, number, l.tasks) {
                return Err(Refusal::Cycle);
            }
            if parent_task.delegates.len() >= usize::try_from(l.delegates).expect("u32 fits usize")
                || parent_task.results_due.len() >= usize::try_from(l.delegates).expect("u32 fits usize")
            {
                return Err(Refusal::Delegates);
            }
            if !crate::batch::contains(&parent_task.references, number)
                && parent_task.references.len() >= usize::try_from(l.references).expect("u32 fits usize")
            {
                return Err(Refusal::Reference);
            }
            if !crate::inbox::room(
                d,
                l,
                parent,
                1,
                usize::try_from(l.result_bytes).expect("u32 fits usize").saturating_mul(2),
            ) {
                return Err(Refusal::Inbox);
            }
            let mut at = Some(parent);
            for _ in 0..l.tasks {
                let Some(parent) = at else {
                    break;
                };
                let ancestor = record(d, parent).expect("ancestor live");
                if !crate::amend::below(d, number, parent, l.tasks)
                    && match ancestor.made.checked_add(task.made) {
                        Some(count) => count > l.tree_tasks,
                        None => true,
                    }
                {
                    return Err(Refusal::Tree);
                }
                at = match ancestor.requester {
                    Party::Task(parent) => Some(parent),
                    Party::Person(_) | Party::Deployment { .. } => None,
                };
            }
            parent_task.depth.checked_add(1).ok_or(Refusal::Depth)?
        }
        Party::Person(_) => 0,
        Party::Deployment { project } => {
            if project != task.project {
                return Err(Refusal::Project);
            }
            0
        }
    };
    Ok(depth)
}
