//! Atomic batch admission, adapting legacy plan's bounded Kahn check.
//! All new tasks have one creator; dependencies cannot be added later.
use crate::domain::{Domain, record};
use crate::{Authority, Contract, Executor, Last, Limits, New, Parameter, Party, Phase, Problem, Refusal, Spec, Was};
use skein_lib::List;
fn problem(task: Option<u64>, why: Refusal) -> Problem {
    Problem { task, why }
}
pub(crate) fn check(d: &Domain, l: &Limits, creator: Party, batch: &[New]) -> Result<(), Problem> {
    let size = u32::try_from(batch.len()).unwrap_or(u32::MAX);
    if size == 0 {
        return Err(problem(None, Refusal::Empty));
    }
    if size > l.batch {
        return Err(problem(None, Refusal::Batch));
    }
    if d.names.len().saturating_add(size) > l.tasks || d.tasks.capacity().saturating_sub(d.tasks.len()) < size {
        return Err(problem(None, Refusal::Live));
    }
    let parent = match creator {
        Party::Task(number) => {
            let Some(parent) = record(d, number) else {
                return Err(problem(Some(number), Refusal::Unknown));
            };
            match &parent.phase {
                Phase::Closing(_) | Phase::Ended(_) => return Err(problem(Some(number), Refusal::State)),
                Phase::Held { was, .. } => match was {
                    Was::Closing(_) => return Err(problem(Some(number), Refusal::State)),
                    Was::Waiting | Was::Active(_) => {}
                },
                Phase::Waiting | Phase::Active(_) => {}
            }
            if u32::try_from(parent.delegates.len()).unwrap_or(u32::MAX).saturating_add(size) > l.delegates {
                return Err(problem(Some(number), Refusal::Delegates));
            }
            if parent.depth.saturating_add(1) > l.depth {
                return Err(problem(Some(number), Refusal::Depth));
            }
            let root = record(d, parent.root).expect("a live task's root is live");
            if match root.made.checked_add(size) {
                Some(total) => total > l.tree_tasks,
                None => true,
            } {
                return Err(problem(Some(number), Refusal::Tree));
            }
            Some(parent)
        }
        Party::Person(_) | Party::Deployment { .. } => None,
    };
    check_members(d, l, creator, batch, parent)?;
    if !crate::funders::can_reserve(d, creator, batch) {
        return Err(problem(None, Refusal::Funding));
    }
    if !acyclic(d, l, creator, batch) {
        return Err(problem(Some(batch.first().expect("nonempty batch admitted").number), Refusal::Cycle));
    }
    Ok(())
}
fn check_members(
    d: &Domain,
    l: &Limits,
    creator: Party,
    batch: &[New],
    parent: Option<&crate::TaskRecord>,
) -> Result<(), Problem> {
    for (at, new) in batch.iter().enumerate() {
        let number = Some(new.number);
        if d.names.contains_key(&new.number) {
            return Err(problem(number, Refusal::Duplicate));
        }
        for earlier in batch.iter().take(at) {
            if earlier.number == new.number {
                return Err(problem(number, Refusal::Duplicate));
            }
        }
        if let Some(parent) = parent
            && new.project != parent.project
        {
            return Err(problem(number, Refusal::Project));
        }
        match creator {
            Party::Deployment { project } if project != new.project => return Err(problem(number, Refusal::Project)),
            Party::Deployment { .. } | Party::Task(_) | Party::Person(_) => {}
        }
        let mut in_project = 0_u32;
        for (_, id) in &d.names {
            if d.tasks.get(*id).expect("name indexes live task").record.project == new.project {
                in_project = in_project.saturating_add(1);
            }
        }
        for sibling in batch {
            if sibling.project == new.project {
                in_project = in_project.saturating_add(1);
            }
        }
        if in_project > l.project_tasks {
            return Err(problem(number, Refusal::Project));
        }
        match new.executor {
            Executor::Agent { charter } => {
                let mut known = false;
                for configured in &d.charters {
                    if *configured == charter {
                        known = true;
                    }
                }
                if !known {
                    return Err(problem(number, Refusal::Executor));
                }
            }
        }
        if !valid_spec(l, &new.spec) {
            return Err(problem(number, Refusal::Spec));
        }
        if !valid_contract(l, &new.contract) {
            return Err(problem(number, Refusal::Contract));
        }
        if !valid_authority(l, &new.authority) {
            return Err(problem(number, Refusal::AuthorityShape));
        }
        // Historical result admission belongs to an actual root input route.
        if !new.spec.inputs.is_empty() {
            return Err(problem(number, Refusal::Inputs));
        }
        if new.dependencies.len() > usize::try_from(l.dependencies).expect("u32 fits usize") {
            return Err(problem(number, Refusal::Dependencies));
        }
        for (index, dependency) in new.dependencies.iter().enumerate() {
            for earlier in new.dependencies.iter().take(index) {
                if earlier == dependency {
                    return Err(problem(number, Refusal::Dependencies));
                }
            }
            let mut known = false;
            for sibling in batch {
                if sibling.number == *dependency {
                    known = true;
                }
            }
            if !known && let Some(parent) = parent {
                for delegate in &parent.delegates {
                    if delegate == dependency && d.names.contains_key(dependency) {
                        known = true;
                    }
                }
            }
            if !known {
                return Err(problem(number, Refusal::Dependencies));
            }
        }
    }
    Ok(())
}
pub(crate) fn contains(numbers: &[u64], number: u64) -> bool {
    for candidate in numbers {
        if *candidate == number {
            return true;
        }
    }
    false
}
pub(crate) fn valid_spec(l: &Limits, spec: &Spec) -> bool {
    if spec.parameters.len() > usize::try_from(l.parameters).expect("u32 fits usize")
        || spec.inputs.len() > usize::try_from(l.inputs).expect("u32 fits usize")
    {
        return false;
    }
    for (at, input) in spec.inputs.iter().enumerate() {
        for earlier in spec.inputs.iter().take(at) {
            if earlier == input {
                return false;
            }
        }
    }
    let mut bytes = spec.words.len();
    for parameter in &spec.parameters {
        match parameter {
            Parameter::Bytes { value, .. } => {
                let Some(total) = bytes.checked_add(value.len()) else {
                    return false;
                };
                bytes = total;
            }
            Parameter::Number { .. } | Parameter::Resource { .. } => {}
        }
    }
    bytes <= usize::try_from(l.spec_bytes).expect("u32 fits usize")
}
pub(crate) fn valid_contract(l: &Limits, contract: &Contract) -> bool {
    match contract {
        Contract::Report { words } | Contract::Change { words, .. } => *words <= l.result_bytes,
        Contract::Verdict { choices } => {
            if choices.is_empty() || choices.len() > usize::try_from(l.contract_choices).expect("u32 fits usize") {
                return false;
            }
            for (at, choice) in choices.iter().enumerate() {
                if choice.words > l.result_bytes {
                    return false;
                }
                for earlier in choices.iter().take(at) {
                    if earlier.code == choice.code {
                        return false;
                    }
                }
            }
            true
        }
    }
}
pub(crate) fn valid_authority(l: &Limits, value: &Authority) -> bool {
    if value.grants.len() > usize::try_from(l.authority_grants).expect("u32 fits usize")
        || value.delegation.kinds.len() > usize::try_from(l.executor_kinds).expect("u32 fits usize")
    {
        return false;
    }
    let mut bytes = 0_usize;
    for grant in &value.grants {
        if grant.pattern.segments.len() > usize::try_from(l.authority_segments).expect("u32 fits usize") {
            return false;
        }
        for segment in &grant.pattern.segments {
            let Some(total) = bytes.checked_add(segment.len()) else {
                return false;
            };
            bytes = total;
        }
        match &grant.pattern.last {
            Last::None => {}
            Last::Exact(last) | Last::Open(last) => {
                let Some(total) = bytes.checked_add(last.len()) else {
                    return false;
                };
                bytes = total;
            }
        }
    }
    bytes <= usize::try_from(l.authority_bytes).expect("u32 fits usize")
}

/// Include the delegation edges added to an existing creator, as well as all
/// immutable existing dependencies: a reference to an ancestor can otherwise
/// create a wait cycle without a cycle among the new tasks themselves.
pub(crate) fn acyclic(d: &Domain, l: &Limits, creator: Party, batch: &[New]) -> bool {
    let mut ordered = List::with_capacity(l.tasks);
    let total = d.names.len().saturating_add(u32::try_from(batch.len()).unwrap_or(u32::MAX));
    for _ in 0..total {
        let mut next = None;
        for (number, _) in &d.names {
            if contains(ordered.as_slice(), *number) {
                continue;
            }
            let task = record(d, *number).expect("indexed task live");
            let mut ready = true;
            for dependency in &task.dependencies {
                if d.names.contains_key(dependency) && !contains(ordered.as_slice(), *dependency) {
                    ready = false;
                }
            }
            for delegate in &task.delegates {
                if !contains(ordered.as_slice(), *delegate) {
                    ready = false;
                }
            }
            if creator == Party::Task(*number) {
                for new in batch {
                    if !contains(ordered.as_slice(), new.number) {
                        ready = false;
                    }
                }
            }
            if ready {
                next = Some(*number);
                break;
            }
        }
        if next.is_none() {
            for new in batch {
                if contains(ordered.as_slice(), new.number) {
                    continue;
                }
                let mut ready = true;
                for dependency in &new.dependencies {
                    if (d.names.contains_key(dependency) || batch_has(batch, *dependency))
                        && !contains(ordered.as_slice(), *dependency)
                    {
                        ready = false;
                    }
                }
                if ready {
                    next = Some(new.number);
                    break;
                }
            }
        }
        let Some(number) = next else {
            return false;
        };
        ordered.push(number).expect("admitted graph bounded by live task capacity");
    }
    true
}
fn batch_has(batch: &[New], number: u64) -> bool {
    for new in batch {
        if new.number == number {
            return true;
        }
    }
    false
}
