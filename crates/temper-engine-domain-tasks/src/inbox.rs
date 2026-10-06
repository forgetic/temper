//! Bounded person-chat words and atomic turn reads (domain/tasks.md, section 7).
use crate::domain::{Domain, publish, record, refused, task_mut};
use crate::{Active, Limits, Party, Phase, Refusal, Request, Word};
use skein_lib::{Env, List, Queue, ReplyTo};

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
    if task.project != project || !requester {
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
    let mut bytes = 0_usize;
    for item in &task.inbox {
        let Some(total) = bytes.checked_add(item.words.len()) else {
            return refused(to, Some(number), Refusal::Busy, out);
        };
        bytes = total;
    }
    let room = match bytes.checked_add(word.words.len()) {
        Some(total) => total <= usize::try_from(env.limits.inbox_bytes).expect("u32 fits usize"),
        None => false,
    };
    if task.inbox.len() >= usize::try_from(env.limits.inbox_messages).expect("u32 fits usize") || !room {
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
