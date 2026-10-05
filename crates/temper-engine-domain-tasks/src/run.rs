use crate::domain::{Domain, activate, entrance, fact, publish, record, refused, task_mut};
use crate::{
    Accepted, Active, Class, Closing, Contract, End, Ending, Fact, Hold, Limits, Phase, Refusal, Request, Result,
    Stage, Tries, Was,
};
use skein_lib::{Env, Queue, ReplyTo};
pub(crate) fn prepare(d: &mut Domain, env: &Env<Limits>, to: ReplyTo, number: u64, out: &mut Queue<Request>) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    let task = task_mut(d, number).expect("entrance names task");
    if task.record.phase != Phase::Active(Active::Due) {
        return refused(to, Some(number), Refusal::State, out);
    }
    task.record.phase = Phase::Active(Active::Preparing);
    publish(d, env, number, out);
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn claim(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    attempt: u64,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    let task = task_mut(d, number).expect("entrance names task");
    if attempt <= task.record.attempt {
        return refused(to, Some(number), Refusal::Attempt, out);
    }
    if task.record.phase != Phase::Active(Active::Preparing) {
        return refused(to, Some(number), Refusal::State, out);
    }
    task.record.attempt = attempt;
    task.record.phase = Phase::Active(Active::Claimed { attempt });
    publish(d, env, number, out);
    fact(d, Fact::Claimed { task: number, attempt });
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn started(d: &mut Domain, env: &Env<Limits>, number: u64, attempt: u64, out: &mut Queue<Request>) {
    if !d.ready() {
        return;
    }
    if let Some(task) = task_mut(d, number)
        && task.record.phase == Phase::Active(Active::Claimed { attempt })
    {
        task.record.phase = Phase::Active(Active::Running { attempt });
        publish(d, env, number, out);
    }
}
pub(crate) fn run_attempt(phase: &Phase) -> Option<u64> {
    match phase {
        Phase::Active(Active::Claimed { attempt } | Active::Running { attempt })
        | Phase::Closing(Closing { stage: Stage::Run { attempt }, .. })
        | Phase::Held {
            was:
                Was::Active(Active::Claimed { attempt } | Active::Running { attempt })
                | Was::Closing(Closing { stage: Stage::Run { attempt }, .. }),
            ..
        } => Some(*attempt),
        Phase::Waiting
        | Phase::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. })
        | Phase::Closing(Closing { stage: Stage::Delegates | Stage::Effects | Stage::Settled, .. })
        | Phase::Held {
            was:
                Was::Waiting
                | Was::Active(Active::Idle | Active::Due | Active::Preparing | Active::BackingOff { .. })
                | Was::Closing(Closing { stage: Stage::Delegates | Stage::Effects | Stage::Settled, .. }),
            ..
        }
        | Phase::Ended(_) => None,
    }
}
pub(crate) fn valid_result(contract: &Contract, result: &Result, l: &Limits) -> bool {
    match result {
        Result::Failure { reason } => match contract {
            Contract::Report { .. } | Contract::Verdict { .. } | Contract::Change { .. } => {
                reason.len() <= usize::try_from(l.result_bytes).expect("u32 fits usize")
            }
        },
        Result::Report { words } => match contract {
            Contract::Report { words: limit } => words.len() <= usize::try_from(*limit).expect("u32 fits usize"),
            Contract::Verdict { .. } | Contract::Change { .. } => false,
        },
        Result::Verdict { code, words } => match contract {
            Contract::Verdict { choices } => {
                for choice in choices {
                    if choice.code == *code && words.len() <= usize::try_from(choice.words).expect("u32 fits usize") {
                        return true;
                    }
                }
                false
            }
            Contract::Report { .. } | Contract::Change { .. } => false,
        },
        Result::Change { connector: actual_connector, kind: actual_kind, words, .. } => match contract {
            Contract::Change { connector, kind, words: limit } => {
                connector == actual_connector
                    && kind == actual_kind
                    && words.len() <= usize::try_from(*limit).expect("u32 fits usize")
            }
            Contract::Report { .. } | Contract::Verdict { .. } => false,
        },
    }
}
pub(crate) fn result_bytes(result: &Result) -> usize {
    match result {
        Result::Report { words } | Result::Verdict { words, .. } | Result::Change { words, .. } => words.len(),
        Result::Failure { reason } => reason.len(),
    }
}
fn ending(result: Result) -> Ending {
    match result {
        Result::Failure { reason } => Ending::Failed { reason },
        Result::Report { .. } | Result::Verdict { .. } | Result::Change { .. } => Ending::Done(result),
    }
}
pub(crate) fn activation(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    attempt: u64,
    end: End,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, Some(number), Refusal::NotReady, out);
    }
    let Some(old) = record(d, number) else {
        if let Some(stub) = d.stubs.get(&number)
            && stub.last_answer == Some(attempt)
        {
            return out.push(Request::Acknowledged {
                reply_to: to,
                task: number,
                attempt,
                accepted: Accepted::Already,
            });
        }
        return refused(to, Some(number), Refusal::Unknown, out);
    };
    if old.last_answer == Some(attempt) {
        return out.push(Request::Acknowledged { reply_to: to, task: number, attempt, accepted: Accepted::Already });
    }
    if run_attempt(&old.phase) != Some(attempt) {
        return refused(to, Some(number), Refusal::Attempt, out);
    }
    let held = match old.phase {
        Phase::Held { why, .. } => Some(why),
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Ended(_) => None,
    };
    let closing = match &old.phase {
        Phase::Closing(closing) | Phase::Held { was: Was::Closing(closing), .. } => Some(closing.clone()),
        Phase::Waiting
        | Phase::Active(_)
        | Phase::Held { was: Was::Waiting | Was::Active(_), .. }
        | Phase::Ended(_) => None,
    };
    let end = match end {
        End::Finished { result, cancel_delegates } => {
            if valid_result(&old.contract, &result, &env.limits) {
                End::Finished { result, cancel_delegates }
            } else {
                End::Failed(Class::Invalid)
            }
        }
        End::Parked | End::Failed(_) | End::Refused => end,
    };
    let refuses_live = match &end {
        End::Finished { cancel_delegates, .. } => !cancel_delegates,
        End::Parked | End::Failed(_) | End::Refused => false,
    };
    if closing.is_none() && refuses_live && !old.delegates.is_empty() {
        return refused(to, Some(number), Refusal::LiveDelegates, out);
    }
    let next = if let Some(mut closing) = closing {
        closing.stage = Stage::Delegates;
        match &mut closing.ending {
            Ending::Cancelled { result: partial, .. } => match end {
                End::Finished { result, .. } => *partial = Some(result),
                End::Parked | End::Failed(_) | End::Refused => {}
            },
            Ending::Done(_) | Ending::Failed { .. } => {}
        }
        Phase::Closing(closing)
    } else {
        match end {
            End::Finished { result, cancel_delegates } => {
                if cancel_delegates {
                    crate::closing::cancel_delegates(d, env, number, b"", out);
                }
                Phase::Closing(Closing { stage: Stage::Delegates, ending: ending(result) })
            }
            End::Parked => {
                let task = task_mut(d, number).expect("terminal names live task");
                task.record.tries = Tries::NONE;
                task.record.refusals = 0;
                Phase::Active(Active::Idle)
            }
            End::Failed(class) => failure(d, env, number, class),
            End::Refused => pause(d, env, number),
        }
    };
    let task = task_mut(d, number).expect("terminal names live task");
    task.record.last_answer = Some(attempt);
    task.record.phase = match held {
        Some(why) => Phase::Held { was: was(next), why },
        None => next,
    };
    publish(d, env, number, out);
    out.push(Request::Acknowledged { reply_to: to, task: number, attempt, accepted: Accepted::New });
}
pub(crate) fn was(phase: Phase) -> Was {
    match phase {
        Phase::Waiting => Was::Waiting,
        Phase::Active(active) => Was::Active(active),
        Phase::Closing(closing) => Was::Closing(closing),
        Phase::Held { was, .. } => was,
        Phase::Ended(_) => unreachable!("ended task cannot be held"),
    }
}
fn wall_after(env: &Env<Limits>, duration: skein_lib::Duration) -> skein_lib::Wall {
    skein_lib::Wall::from_nanos(env.wall.as_nanos().saturating_add(duration.as_nanos()))
}
fn failure(d: &mut Domain, env: &Env<Limits>, number: u64, class: Class) -> Phase {
    let task = task_mut(d, number).expect("failure names live task");
    task.record.tries.add(class);
    task.record.refusals = 0;
    let times = task.record.tries.of(class);
    let retry = env.limits.retries.of(class);
    fact(d, Fact::Failed { task: number, class });
    if times > retry.retries {
        fact(d, Fact::Held { task: number, why: Hold::Failures(class) });
        Phase::Held { was: Was::Active(Active::Due), why: Hold::Failures(class) }
    } else {
        let duration = crate::failures::backoff(times, retry, &mut d.rng);
        Phase::Active(Active::BackingOff { until: wall_after(env, duration) })
    }
}
fn pause(d: &mut Domain, env: &Env<Limits>, number: u64) -> Phase {
    let task = task_mut(d, number).expect("pause names live task");
    task.record.refusals = task.record.refusals.saturating_add(1);
    let times = task.record.refusals;
    let duration = crate::failures::backoff(times, env.limits.retries.transient, &mut d.rng);
    Phase::Active(Active::BackingOff { until: wall_after(env, duration) })
}
pub(crate) fn preparation_failed(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    if !d.ready() {
        return;
    }
    if let Some(task) = record(d, number)
        && task.phase == Phase::Active(Active::Preparing)
    {
        let phase = pause(d, env, number);
        task_mut(d, number).expect("prepared task exists").record.phase = phase;
        publish(d, env, number, out);
    }
}
pub(crate) fn hold(d: &mut Domain, env: &Env<Limits>, number: u64, why: Hold, out: &mut Queue<Request>) {
    if !d.ready() {
        return;
    }
    let Some(task) = record(d, number) else {
        return;
    };
    match task.phase {
        Phase::Held { .. } | Phase::Ended(_) => return,
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) => {}
    }
    if let Some(attempt) = run_attempt(&task.phase) {
        out.push(Request::Stop { task: number, attempt });
    }
    let task = task_mut(d, number).expect("hold names live task");
    let old = core::mem::replace(&mut task.record.phase, Phase::Waiting);
    task.record.phase = Phase::Held { was: was(old), why };
    publish(d, env, number, out);
    fact(d, Fact::Held { task: number, why });
}
pub(crate) fn release(d: &mut Domain, env: &Env<Limits>, to: ReplyTo, number: u64, out: &mut Queue<Request>) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    let old = record(d, number).expect("entrance names task");
    if run_attempt(&old.phase).is_some() {
        return refused(to, Some(number), Refusal::Busy, out);
    }
    let previous = match &old.phase {
        Phase::Held { was, .. } => was,
        Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Ended(_) => {
            return refused(to, Some(number), Refusal::Unheld, out);
        }
    };
    let next = match previous {
        Was::Waiting => Phase::Waiting,
        Was::Active(Active::Preparing | Active::BackingOff { .. }) => Phase::Active(Active::Due),
        Was::Active(active) => Phase::Active(*active),
        Was::Closing(closing) => Phase::Closing(closing.clone()),
    };
    let task = task_mut(d, number).expect("entrance names task");
    task.record.phase = next;
    task.record.tries = Tries::NONE;
    task.record.refusals = 0;
    let due = task.record.phase == Phase::Active(Active::Due);
    publish(d, env, number, out);
    if due {
        activate(d, number, out);
    }
    fact(d, Fact::Released { task: number });
    out.push(Request::Done { reply_to: to });
}
