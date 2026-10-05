use crate::domain::{Domain, activate, entrance, fact, publish, record, refused, snapshot, task_mut};
use crate::{
    Active, Closing, Ending, Fact, Hold, Key, Limits, Party, Phase, Refusal, Request, Result, Stage, Status, Stored,
    Stub, Was,
};
use alloc::boxed::Box;
use skein_lib::{Env, List, Queue, ReplyTo};
pub(crate) fn status(ending: &Ending) -> Status {
    match ending {
        Ending::Done(_) => Status::Done,
        Ending::Failed { .. } => Status::Failed,
        Ending::Cancelled { .. } => Status::Cancelled,
    }
}
fn below(d: &Domain, number: u64, ancestor: u64, bound: u32) -> bool {
    let mut at = Some(number);
    for _ in 0..bound {
        let Some(number) = at else {
            return false;
        };
        if number == ancestor {
            return true;
        }
        at = match record(d, number) {
            Some(task) => match task.requester {
                Party::Task(parent) => Some(parent),
                Party::Person(_) | Party::Deployment { .. } => None,
            },
            None => None,
        };
    }
    false
}
pub(crate) fn cancel(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    reason: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    if reason.len() > usize::try_from(env.limits.result_bytes).expect("u32 fits usize") {
        return refused(to, Some(number), Refusal::Reason, out);
    }
    cancel_tree(d, env, number, &reason, out);
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn cancel_delegates(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    reason: &[u8],
    out: &mut Queue<Request>,
) {
    let mut delegates = List::with_capacity(env.limits.delegates);
    for delegate in &record(d, number).expect("finishing task live").delegates {
        delegates.push(*delegate).expect("live delegates bounded");
    }
    for delegate in delegates.into_boxed() {
        cancel_tree(d, env, delegate, reason, out);
    }
}
fn previous_result(phase: &Phase) -> Option<Result> {
    match phase {
        Phase::Closing(closing) | Phase::Held { was: Was::Closing(closing), .. } => match &closing.ending {
            Ending::Done(result) => Some(result.clone()),
            Ending::Failed { reason } => Some(Result::Failure { reason: reason.clone() }),
            Ending::Cancelled { result, .. } => result.clone(),
        },
        Phase::Waiting | Phase::Active(_) | Phase::Held { was: Was::Waiting | Was::Active(_), .. } => None,
        Phase::Ended(_) => unreachable!("cancel targets live task"),
    }
}
fn cancel_tree(d: &mut Domain, env: &Env<Limits>, ancestor: u64, reason: &[u8], out: &mut Queue<Request>) {
    let mut selected = List::with_capacity(env.limits.tasks);
    for (number, _) in &d.names {
        if below(d, *number, ancestor, env.limits.tasks) {
            selected.push(*number).expect("selected subtree bounded by live set");
        }
    }
    // Reverse depth traversal gives every descendant its cancellation before
    // the parent can advance. No recursion and no mutable graph iterator.
    let mut deepest = 0_u32;
    for number in &selected {
        deepest = deepest.max(record(d, *number).expect("selected task live").depth);
    }
    for depth in (0..=deepest).rev() {
        for number in &selected {
            let old = record(d, *number).expect("selected task live");
            if old.depth != depth {
                continue;
            }
            let attempt = crate::run::run_attempt(&old.phase);
            let partial = previous_result(&old.phase);
            let stage = match &old.phase {
                Phase::Closing(closing) | Phase::Held { was: Was::Closing(closing), .. } => closing.stage,
                Phase::Waiting | Phase::Active(_) | Phase::Held { was: Was::Waiting | Was::Active(_), .. } => {
                    match attempt {
                        Some(attempt) => Stage::Run { attempt },
                        None => Stage::Delegates,
                    }
                }
                Phase::Ended(_) => unreachable!("selected task live"),
            };
            // Reuse the root's already-open closing gate; replaying Close is
            // required to associate the final cancelled result with it.
            let stage = match stage {
                Stage::Settled => Stage::Delegates,
                Stage::Run { .. } | Stage::Delegates | Stage::Effects => stage,
            };
            task_mut(d, *number).expect("selected task live").record.phase =
                Phase::Closing(Closing { stage, ending: Ending::Cancelled { reason: reason.into(), result: partial } });
            publish(d, env, *number, out);
            if let Some(attempt) = attempt {
                out.push(Request::Stop { task: *number, attempt });
            }
            if stage == Stage::Effects {
                let ending = match &record(d, *number).expect("selected task live").phase {
                    Phase::Closing(closing) => closing.ending.clone(),
                    Phase::Waiting | Phase::Active(_) | Phase::Held { .. } | Phase::Ended(_) => {
                        unreachable!("cancel installed closing")
                    }
                };
                out.push(Request::Close { task: *number, ending });
            }
        }
    }
}
pub(crate) fn settled(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    if !d.ready() {
        return;
    }
    let Some(task) = task_mut(d, number) else {
        return;
    };
    match &mut task.record.phase {
        Phase::Closing(closing) | Phase::Held { was: Was::Closing(closing), .. } if closing.stage == Stage::Effects => {
            closing.stage = Stage::Settled;
            publish(d, env, number, out);
        }
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Held { .. } | Phase::Ended(_) => {}
    }
}
pub(crate) fn progress(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    // Every pass must advance a phase or end a task to need another pass;
    // immutable dependency DAGs and delegation depth bound the cascade.
    for _ in 0..env.limits.tasks.saturating_add(1) {
        let mut changed = false;
        let numbers = snapshot(d, env.limits.tasks);
        for number in numbers.into_boxed() {
            let Some(task) = record(d, number) else {
                continue;
            };
            match &task.phase {
                Phase::Waiting => {
                    let mut ready = true;
                    let mut failed = None;
                    for dependency in &task.dependencies {
                        if d.names.contains_key(dependency) {
                            ready = false;
                        } else if let Some(stub) = d.stubs.get(dependency) {
                            match stub.status {
                                Status::Done => {}
                                Status::Failed | Status::Cancelled => {
                                    failed = Some(*dependency);
                                }
                            }
                        } else {
                            unreachable!("admitted dependency is live or retained stub");
                        }
                    }
                    if let Some(dependency) = failed {
                        task_mut(d, number).expect("waiting task live").record.phase =
                            Phase::Held { was: Was::Waiting, why: Hold::Dependency(dependency) };
                        publish(d, env, number, out);
                        crate::refs::notify(d, number, crate::Notice::Held(Hold::Dependency(dependency)), out);
                        fact(d, Fact::Held { task: number, why: Hold::Dependency(dependency) });
                        changed = true;
                    } else if ready {
                        task_mut(d, number).expect("waiting task live").record.phase = Phase::Active(Active::Due);
                        publish(d, env, number, out);
                        activate(d, number, out);
                        changed = true;
                    }
                }
                Phase::Closing(closing) => match closing.stage {
                    Stage::Delegates if task.delegates.is_empty() && task.results_due.is_empty() => {
                        let ending = closing.ending.clone();
                        let task = task_mut(d, number).expect("closing task live");
                        task.record.phase = Phase::Closing(Closing { stage: Stage::Effects, ending: ending.clone() });
                        publish(d, env, number, out);
                        out.push(Request::Close { task: number, ending });
                        changed = true;
                    }
                    Stage::Run { .. } | Stage::Effects | Stage::Delegates => {}
                    Stage::Settled => {
                        end_task(d, env, number, out);
                        changed = true;
                    }
                },
                Phase::Active(_) | Phase::Held { .. } | Phase::Ended(_) => {}
            }
        }
        if !changed {
            break;
        }
    }
}
fn end_task(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let task = record(d, number).expect("settled task live");
    let ending = match &task.phase {
        Phase::Closing(closing) => match closing.stage {
            Stage::Settled => &closing.ending,
            Stage::Run { .. } | Stage::Delegates | Stage::Effects => unreachable!("ending follows settlement"),
        },
        Phase::Waiting | Phase::Active(_) | Phase::Held { .. } | Phase::Ended(_) => {
            unreachable!("ending follows settlement")
        }
    };
    assert!(task.delegates.is_empty(), "all delegates ended before requester");
    let ending = ending.clone();
    let requester = task.requester;
    let stub = Stub {
        number,
        project: task.project,
        status: status(&ending),
        attempt: task.attempt,
        last_answer: task.last_answer,
    };
    let mut ended = task.clone();
    ended.phase = Phase::Ended(ending.clone());
    crate::inbox::archive(d, number, out);
    crate::refs::end(d, number, out);
    crate::refs::notify(d, number, crate::Notice::Ended(ending.clone()), out);
    d.wakes.cancel(number);
    let id = d.names.remove(&number).expect("ending name exists");
    d.tasks.retire(id);
    d.alarms.cancel(number);
    out.push(Request::Erase { key: Key::Live(number) });
    out.push(Request::Save { record: Stored::Ended(Box::new(ended)) });
    if needed(d, number) {
        let saved = d.stubs.insert(number, stub);
        assert!(saved == Ok(None), "ended stub room reserved at make");
        out.push(Request::Save { record: Stored::Stub(stub) });
    }
    match requester {
        Party::Task(parent) => {
            let mut remaining = List::with_capacity(env.limits.delegates);
            let task = task_mut(d, parent).expect("requester remains live until all delegates end");
            for delegate in &task.record.delegates {
                if *delegate != number {
                    remaining.push(*delegate).expect("remaining delegates bounded");
                }
            }
            task.record.delegates = remaining.into_boxed();
            publish(d, env, parent, out);
        }
        Party::Person(_) | Party::Deployment { .. } => {}
    }
    out.push(Request::Ended { task: number, requester, ending });
    fact(d, Fact::Ended { task: number, status: stub.status });
}
pub(crate) fn needed(d: &Domain, number: u64) -> bool {
    for (_, id) in &d.names {
        let task = &d.tasks.get(*id).expect("name indexes live task").record;
        if crate::batch::contains(&task.dependencies, number)
            || crate::batch::contains(&task.spec.inputs, number)
            || crate::batch::contains(&task.references, number)
            || crate::batch::contains(&task.results_due, number)
        {
            return true;
        }
    }
    for (_, envelope) in &d.messages {
        match envelope.message {
            crate::Message::Result { task, .. } | crate::Message::Notice { target: task, .. } if task == number => {
                return true;
            }
            crate::Message::Words { .. }
            | crate::Message::Question { .. }
            | crate::Message::Answer { .. }
            | crate::Message::Result { .. }
            | crate::Message::News { .. }
            | crate::Message::Notice { .. }
            | crate::Message::Timer { .. } => {}
        }
    }
    false
}
