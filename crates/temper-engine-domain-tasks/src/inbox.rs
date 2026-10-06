//! Bounded person-chat words and atomic turn reads (domain/tasks.md, section 7).
use crate::domain::{Domain, publish, record, refused, task_mut};
use crate::{Active, Limits, MessageKind, Party, Phase, Refusal, Request, Status, Word};
use skein_lib::{Env, List, Queue, ReplyTo};

/// Account for messages already waiting and the result credit reserved for
/// every live direct delegate (domain/tasks.md, section 7.2).
pub(crate) fn room(domain: &Domain, limits: &Limits, task: u64, count: u32, bytes: usize) -> bool {
    let Some(record) = record(domain, task) else {
        return false;
    };
    let reserved = record.delegates.len();
    let Some(total_count) = record.inbox.len().checked_add(reserved) else {
        return false;
    };
    let Some(total_count) = total_count.checked_add(usize::try_from(count).expect("u32 fits usize")) else {
        return false;
    };
    let mut total_bytes = bytes;
    for message in &record.inbox {
        let Some(sum) = total_bytes.checked_add(message.words.len()) else {
            return false;
        };
        total_bytes = sum;
    }
    let Some(result_bytes) = reserved.checked_mul(usize::try_from(limits.result_bytes).expect("u32 fits usize")) else {
        return false;
    };
    let Some(total_bytes) = total_bytes.checked_add(result_bytes) else {
        return false;
    };
    total_count <= usize::try_from(limits.inbox_messages).expect("u32 fits usize")
        && total_bytes <= usize::try_from(limits.inbox_bytes).expect("u32 fits usize")
}

pub(crate) fn message(
    domain: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    project: u32,
    number: u64,
    word: Word,
    out: &mut Queue<Request>,
) {
    if !domain.ready() {
        return refused(to, Some(number), Refusal::NotReady, out);
    }
    let Some(task) = record(domain, number) else {
        return refused(to, Some(number), Refusal::Unknown, out);
    };
    let requester = match task.requester {
        Party::Person(person) => word.from == Party::Person(person),
        Party::Task(_) | Party::Deployment { .. } => false,
    };
    if task.project != project || !requester || word.kind != MessageKind::Words {
        return refused(to, Some(number), Refusal::State, out);
    }
    let active = match task.phase {
        Phase::Active(_) => true,
        Phase::Waiting | Phase::Closing(_) | Phase::Held { .. } | Phase::Ended(_) => false,
    };
    if !active {
        return refused(to, Some(number), Refusal::State, out);
    }
    if word.number == 0
        || word.number <= task.last_message
        || word.words.is_empty()
        || word.words.len() > usize::try_from(env.limits.message_bytes).expect("u32 fits usize")
    {
        return refused(to, Some(number), Refusal::Read, out);
    }
    if !room(domain, &env.limits, number, 1, word.words.len()) {
        return refused(to, Some(number), Refusal::Busy, out);
    }
    let phase = task.phase.clone();
    let previous = match task.last_message {
        0 => None,
        number => Some(number),
    };
    let mut inbox = List::with_capacity(env.limits.inbox_messages);
    for item in &task.inbox {
        inbox.push(item.clone()).expect("preflighted inbox count");
    }
    inbox.push(word.clone()).expect("preflighted inbox count");
    let task = task_mut(domain, number).expect("admitted task still live");
    task.record.last_message = word.number;
    task.record.inbox = inbox.into_boxed();
    let mut wake = false;
    let mut relay = None;
    match phase {
        Phase::Active(Active::Idle) => wake = true,
        Phase::Active(Active::Claimed { attempt } | Active::Running { attempt }) => relay = Some(attempt),
        Phase::Active(Active::Due | Active::Preparing | Active::BackingOff { .. }) => {}
        Phase::Waiting | Phase::Closing(_) | Phase::Held { .. } | Phase::Ended(_) => unreachable!("active entrance"),
    }
    if wake {
        task.record.phase = Phase::Active(Active::Due);
    }
    publish(domain, env, number, out);
    if wake {
        super::domain::activate(domain, number, out);
    }
    if let Some(attempt) = relay {
        out.push(Request::Relay { task: number, attempt, previous, word: word.clone() });
    }
    out.push(Request::Sent { reply_to: to, task: number, word });
}

/// Root's same-decision result handoff after a delegate settled. Its reserved
/// inbox credit cannot be consumed by ordinary words.
pub(crate) fn delegate_result(
    domain: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    word: Word,
    out: &mut Queue<Request>,
) {
    let Some(task) = record(domain, number) else {
        return;
    };
    let status = match word.kind {
        MessageKind::Result(
            crate::ResultKind::Report | crate::ResultKind::Verdict { .. } | crate::ResultKind::Change { .. },
        ) => Status::Done,
        MessageKind::Result(crate::ResultKind::Failed) => Status::Failed,
        MessageKind::Result(crate::ResultKind::Cancelled) => Status::Cancelled,
        MessageKind::Words => return,
    };
    if !match word.from {
        Party::Task(_) => true,
        Party::Person(_) | Party::Deployment { .. } => false,
    } || word.number == 0
        || word.number <= task.last_message
        || word.words.len() > usize::try_from(env.limits.result_bytes).expect("u32 fits usize")
        || !room(domain, &env.limits, number, 1, word.words.len())
    {
        return;
    }
    let phase = task.phase.clone();
    let last_delegate = task.delegates.is_empty();
    let previous = if task.last_message == 0 { None } else { Some(task.last_message) };
    let mut inbox = List::with_capacity(env.limits.inbox_messages);
    for item in &task.inbox {
        inbox.push(item.clone()).expect("reserved inbox count");
    }
    inbox.push(word.clone()).expect("reserved result count");
    let task = task_mut(domain, number).expect("requester remains live");
    task.record.last_message = word.number;
    task.record.inbox = inbox.into_boxed();
    let mut wake = false;
    let mut relay = None;
    match phase {
        Phase::Active(Active::Idle) => wake = last_delegate || status != Status::Done,
        Phase::Active(Active::Claimed { attempt } | Active::Running { attempt }) => relay = Some(attempt),
        Phase::Waiting
        | Phase::Active(Active::Due | Active::Preparing | Active::BackingOff { .. })
        | Phase::Closing(_)
        | Phase::Held { .. }
        | Phase::Ended(_) => {}
    }
    if wake {
        task.record.phase = Phase::Active(Active::Due);
    }
    publish(domain, env, number, out);
    if wake {
        super::domain::activate(domain, number, out);
    }
    if let Some(attempt) = relay {
        out.push(Request::Relay { task: number, attempt, previous, word });
    }
}

pub(crate) fn readable(task: &crate::TaskRecord, read: Option<u64>) -> bool {
    match read {
        None => true,
        Some(number) => {
            for word in &task.inbox {
                if word.number == number {
                    return true;
                }
            }
            false
        }
    }
}

pub(crate) fn take(task: &mut crate::TaskRecord, read: Option<u64>) {
    if let Some(number) = read {
        let mut kept = List::with_capacity(u32::try_from(task.inbox.len()).expect("bounded inbox count"));
        for word in &task.inbox {
            if word.number > number {
                kept.push(word.clone()).expect("retained subset fits");
            }
        }
        task.inbox = kept.into_boxed();
    }
}
