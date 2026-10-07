//! Typed task values admitted from bounded JSON host inputs. Root authority
//! and tasks still check these values against current durable facts.

use alloc::boxed::Box;
use skein_lib::{Duration, List, Wall};
use temper_engine_domain::engine;
use temper_engine_domain_tasks as tasks;

use crate::{Problem, json::Value};

pub(crate) fn delegate(value: &Value) -> Result<engine::Delegate, Problem> {
    let executor = executor(value.required(b"executor")?)?;
    let spec = spec(value.required(b"spec")?)?;
    let contract = contract(value.required(b"contract")?)?;
    let authority = authority(value.required(b"authority")?)?;
    let dependencies = match value.field(b"dependencies")? {
        Some(items) => {
            let items = items.array()?;
            let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
            let mut out = List::with_capacity(capacity);
            for item in items {
                let kind = item.required(b"kind")?.text()?;
                let dependency = match kind.as_ref() {
                    b"batch" => engine::Dependency::Batch(item.required(b"number")?.small()?),
                    b"existing" => engine::Dependency::Existing(item.required(b"number")?.number()?),
                    _ => return Err(Problem::Range),
                };
                let Ok(()) = out.push(dependency) else { return Err(Problem::TooLarge) };
            }
            out.into_boxed()
        }
        None => Box::new([]),
    };
    let wake = match value.field(b"wake")? {
        Some(wake) => wake_policy(wake)?,
        None => tasks::WakePolicy::DEFAULT,
    };
    Ok(engine::Delegate { executor, spec, contract, authority, dependencies, wake })
}

pub(crate) fn delegates(value: &Value) -> Result<Box<[engine::Delegate]>, Problem> {
    let items = value.array()?;
    let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for item in items {
        let Ok(()) = out.push(delegate(item)?) else { return Err(Problem::TooLarge) };
    }
    Ok(out.into_boxed())
}

pub(crate) fn amendment(value: &Value) -> Result<tasks::Amendment, Problem> {
    let spec = match value.field(b"spec")? {
        Some(value) => Some(spec(value)?),
        None => None,
    };
    let wake = match value.field(b"wake")? {
        Some(value) => Some(wake_policy(value)?),
        None => None,
    };
    let dependencies = match value.field(b"dependencies")? {
        Some(value) => Some(numbers(value)?),
        None => None,
    };
    let authority = match value.field(b"authority")? {
        Some(value) => Some(authority(value)?),
        None => None,
    };
    Ok(tasks::Amendment { spec, wake, dependencies, authority, reason: value.required(b"reason")?.text()? })
}

fn executor(value: &Value) -> Result<tasks::Executor, Problem> {
    let kind = value.required(b"kind")?.text()?;
    match kind.as_ref() {
        b"agent" => Ok(tasks::Executor::Agent { charter: value.required(b"charter")?.small()? }),
        b"procedure" => Ok(tasks::Executor::Procedure {
            connector: value.required(b"connector")?.narrow()?,
            code: value.required(b"code")?.small()?,
        }),
        b"person" => Ok(tasks::Executor::Person(tasks::PersonAddress::Person(value.required(b"person")?.number()?))),
        b"role" => Ok(tasks::Executor::Person(tasks::PersonAddress::Role(value.required(b"role")?.small()?))),
        _ => Err(Problem::Range),
    }
}

fn spec(value: &Value) -> Result<tasks::Spec, Problem> {
    let words = value.required(b"words")?.text()?;
    let parameters = match value.field(b"parameters")? {
        Some(items) => {
            let items = items.array()?;
            let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
            let mut out = List::with_capacity(capacity);
            for item in items {
                let name = item.required(b"name")?.small()?;
                let kind = item.required(b"kind")?.text()?;
                let parameter = match kind.as_ref() {
                    b"number" => tasks::Parameter::Number { name, value: item.required(b"value")?.number()? },
                    b"bytes" => tasks::Parameter::Bytes { name, value: item.required(b"value")?.text()? },
                    b"resource" => tasks::Parameter::Resource {
                        name,
                        connector: item.required(b"connector")?.narrow()?,
                        resource: item.required(b"resource")?.number()?,
                    },
                    _ => return Err(Problem::Range),
                };
                let Ok(()) = out.push(parameter) else { return Err(Problem::TooLarge) };
            }
            out.into_boxed()
        }
        None => Box::new([]),
    };
    let inputs = match value.field(b"inputs")? {
        Some(items) => numbers(items)?,
        None => Box::new([]),
    };
    Ok(tasks::Spec { words, parameters, inputs })
}

fn contract(value: &Value) -> Result<tasks::Contract, Problem> {
    let kind = value.required(b"kind")?.text()?;
    match kind.as_ref() {
        b"report" => Ok(tasks::Contract::Report { words: value.required(b"words")?.small()? }),
        b"change" => Ok(tasks::Contract::Change {
            connector: value.required(b"connector")?.narrow()?,
            kind: value.required(b"change_kind")?.narrow()?,
            words: value.required(b"words")?.small()?,
        }),
        b"verdict" => {
            let choices = value.required(b"choices")?.array()?;
            let Ok(capacity) = u32::try_from(choices.len()) else { return Err(Problem::TooLarge) };
            let mut out = List::with_capacity(capacity);
            for choice in choices {
                let Ok(()) = out.push(tasks::Verdict {
                    code: choice.required(b"code")?.small()?,
                    words: choice.required(b"words")?.small()?,
                }) else {
                    return Err(Problem::TooLarge);
                };
            }
            Ok(tasks::Contract::Verdict { choices: out.into_boxed() })
        }
        _ => Err(Problem::Range),
    }
}

pub(crate) fn authority(value: &Value) -> Result<tasks::Authority, Problem> {
    let tools = tasks::Tools(value.required(b"tools")?.number()?);
    let grants = value.required(b"grants")?.array()?;
    let Ok(capacity) = u32::try_from(grants.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for grant in grants {
        let segments = texts(grant.required(b"segments")?)?;
        let terminal = grant.required(b"terminal")?.text()?;
        let bytes = grant.required(b"last")?.text()?;
        let last = match terminal.as_ref() {
            b"exact" => tasks::Last::Exact(bytes),
            b"open" => tasks::Last::Open(bytes),
            _ => return Err(Problem::Range),
        };
        let Ok(()) = out.push(tasks::Grant {
            connector: grant.required(b"connector")?.narrow()?,
            kind: grant.required(b"kind")?.narrow()?,
            pattern: tasks::Pattern { segments, last },
        }) else {
            return Err(Problem::TooLarge);
        };
    }
    let delegation = value.required(b"delegation")?;
    let kinds = delegation.required(b"kinds")?.array()?;
    let Ok(capacity) = u32::try_from(kinds.len()) else { return Err(Problem::TooLarge) };
    let mut permitted = List::with_capacity(capacity);
    for item in kinds {
        let kind = item.required(b"kind")?.text()?;
        let executor = match kind.as_ref() {
            b"agent" => tasks::AuthorityExecutor::Charter(item.required(b"number")?.small()?),
            b"procedure" => tasks::AuthorityExecutor::Procedure(item.required(b"number")?.small()?),
            b"role" => tasks::AuthorityExecutor::Role(item.required(b"number")?.small()?),
            _ => return Err(Problem::Range),
        };
        let Ok(()) = permitted.push(executor) else { return Err(Problem::TooLarge) };
    }
    let budget = value.required(b"budget")?;
    let deadline = match budget.field(b"deadline")? {
        Some(deadline) => Some(Wall::from_nanos(deadline.number()?)),
        None => None,
    };
    let Ok(notes) = u8::try_from(value.required(b"notes")?.narrow()?) else { return Err(Problem::Range) };
    Ok(tasks::Authority {
        tools,
        grants: out.into_boxed(),
        delegation: tasks::Delegation {
            kinds: permitted.into_boxed(),
            tasks: delegation.required(b"tasks")?.small()?,
            depth: delegation.required(b"depth")?.small()?,
        },
        budget: tasks::Budget { spend: budget.required(b"spend")?.number()?, deadline },
        notes: tasks::Scopes(notes),
    })
}

fn wake_policy(value: &Value) -> Result<tasks::WakePolicy, Problem> {
    Ok(tasks::WakePolicy {
        words: wake_rule(value.required(b"words")?)?,
        notices: wake_rule(value.required(b"notices")?)?,
        news: wake_rule(value.required(b"news")?)?,
        results: match value.required(b"results")?.text()?.as_ref() {
            b"never" => tasks::ResultsWake::Never,
            b"each" => tasks::ResultsWake::Each,
            b"last_or_failure" => tasks::ResultsWake::LastOrFailure,
            _ => return Err(Problem::Range),
        },
        questions: value.required(b"questions")?.boolean()?,
        answers: value.required(b"answers")?.boolean()?,
        timers: value.required(b"timers")?.boolean()?,
    })
}

fn wake_rule(value: &Value) -> Result<tasks::WakeRule, Problem> {
    match value.required(b"kind")?.text()?.as_ref() {
        b"never" => Ok(tasks::WakeRule::Never),
        b"immediate" => Ok(tasks::WakeRule::Immediate),
        b"batch" => Ok(tasks::WakeRule::Batch {
            count: value.required(b"count")?.small()?,
            age: Duration::from_nanos(value.required(b"age")?.number()?),
        }),
        _ => Err(Problem::Range),
    }
}

fn numbers(value: &Value) -> Result<Box<[u64]>, Problem> {
    let items = value.array()?;
    let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for item in items {
        let Ok(()) = out.push(item.number()?) else { return Err(Problem::TooLarge) };
    }
    Ok(out.into_boxed())
}

fn texts(value: &Value) -> Result<Box<[Box<[u8]>]>, Problem> {
    let items = value.array()?;
    let Ok(capacity) = u32::try_from(items.len()) else { return Err(Problem::TooLarge) };
    let mut out = List::with_capacity(capacity);
    for item in items {
        let Ok(()) = out.push(item.text()?) else { return Err(Problem::TooLarge) };
    }
    Ok(out.into_boxed())
}
