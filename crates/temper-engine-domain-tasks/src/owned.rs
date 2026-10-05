//! Borrowed durable-row heap accounting for root journal/load admission
//! (domain/tasks.md, 2; domain/engine.md, 5.6). This measures existing ownership
//! and allocates nothing; shape/authority admission remains with the hub/root.
use crate::{Authority, Contract, Ending, Last, Parameter, Phase, Spec, Stored, TaskRecord, TaskResult, Was};
use core::mem::{size_of, size_of_val};

/// Root measures a borrowed durable task row before journal/load byte admission.
/// Counts every owned allocation, including the boxed task record, slice backing
/// arrays and all nested bytes. Excludes the inline `Stored` slot, which the root
/// counts separately. Returns `None` if the exact sum cannot fit `u64`; no clone,
/// mutation or allocation occurs (domain/tasks.md, 2; domain/engine.md, 5.6).
#[must_use]
pub fn stored_bytes(record: &Stored) -> Option<u64> {
    match record {
        Stored::Live(task) | Stored::Ended(task) => task_bytes(task),
        Stored::Closure(_) | Stored::Ledger(_) => Some(0),
    }
}

fn bytes(length: usize) -> Option<u64> {
    u64::try_from(length).ok()
}

fn spec_bytes(spec: &Spec) -> Option<u64> {
    let mut total = bytes(spec.words.len())?
        .checked_add(bytes(size_of_val(&*spec.parameters))?)?
        .checked_add(bytes(size_of_val(&*spec.inputs))?)?;
    for parameter in &spec.parameters {
        match parameter {
            Parameter::Bytes { value, .. } => total = total.checked_add(bytes(value.len())?)?,
            Parameter::Number { .. } | Parameter::Resource { .. } => {}
        }
    }
    Some(total)
}

fn authority_bytes(authority: &Authority) -> Option<u64> {
    let mut total =
        bytes(size_of_val(&*authority.grants))?.checked_add(bytes(size_of_val(&*authority.delegation.kinds))?)?;
    for grant in &authority.grants {
        total = total.checked_add(bytes(size_of_val(&*grant.pattern.segments))?)?;
        for segment in &grant.pattern.segments {
            total = total.checked_add(bytes(segment.len())?)?;
        }
        match &grant.pattern.last {
            Last::None => {}
            Last::Exact(word) | Last::Open(word) => total = total.checked_add(bytes(word.len())?)?,
        }
    }
    Some(total)
}

fn contract_bytes(contract: &Contract) -> Option<u64> {
    match contract {
        Contract::Verdict { choices } => bytes(size_of_val(&**choices)),
        Contract::Report { .. } | Contract::Change { .. } => Some(0),
    }
}

fn result_bytes(result: &TaskResult) -> Option<u64> {
    match result {
        TaskResult::Report { words } | TaskResult::Verdict { words, .. } | TaskResult::Change { words, .. } => {
            bytes(words.len())
        }
        TaskResult::Failure { reason } => bytes(reason.len()),
    }
}

fn ending_bytes(ending: &Ending) -> Option<u64> {
    match ending {
        Ending::Done(result) => result_bytes(result),
        Ending::Failed { reason } => bytes(reason.len()),
        Ending::Cancelled { reason, result } => {
            let reason = bytes(reason.len())?;
            match result {
                Some(result) => reason.checked_add(result_bytes(result)?),
                None => Some(reason),
            }
        }
    }
}

fn phase_bytes(phase: &Phase) -> Option<u64> {
    match phase {
        Phase::Closing(closing) => ending_bytes(&closing.ending),
        Phase::Ended(ending) => ending_bytes(ending),
        Phase::Held { was, .. } => match was {
            Was::Closing(closing) => ending_bytes(&closing.ending),
            Was::Waiting | Was::Active(_) => Some(0),
        },
        Phase::Waiting | Phase::Active(_) => Some(0),
    }
}

fn task_bytes(task: &TaskRecord) -> Option<u64> {
    bytes(size_of::<TaskRecord>())?
        .checked_add(spec_bytes(&task.spec)?)?
        .checked_add(authority_bytes(&task.authority)?)?
        .checked_add(contract_bytes(&task.contract)?)?
        .checked_add(phase_bytes(&task.phase)?)?
        .checked_add(bytes(size_of_val(&*task.dependencies))?)?
        .checked_add(bytes(size_of_val(&*task.delegates))?)?
        .checked_add(bytes(size_of_val(&*task.waiting_on))?)
}
