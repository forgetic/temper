//! Explicit, bounded visibility and standing interests. The root finishes
//! Notify/Timer callbacks in the current decision using fresh message numbers.
use crate::domain::{Domain, entrance, publish, record, refused, task_mut};
use crate::{
    Accepted, Envelope, Interest, Key, Limits, Message, NewsClass, Notice, Party, Refusal, Request, Stored,
    Subscription, SubscriptionKind,
};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Wall};
pub(crate) fn references(d: &Domain, source: u64, target: u64) -> bool {
    let Some(task) = record(d, source) else {
        return false;
    };
    task.requester == Party::Task(target)
        || crate::batch::contains(&task.delegates, target)
        || crate::batch::contains(&task.references, target)
}
pub(crate) fn allowed(d: &Domain, from: Party, target: u64) -> bool {
    match from {
        Party::Task(source) => references(d, source, target),
        Party::Person(_) => true,
        Party::Deployment { project } => match record(d, target) {
            Some(task) => task.project == project,
            None => false,
        },
    }
}
pub(crate) fn introduce(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    by: Party,
    left: u64,
    right: u64,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, left) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(left), why, out),
    };
    let Some(right_record) = record(d, right) else {
        return refused(to, Some(right), Refusal::Unknown, out);
    };
    if left == right
        || record(d, left).expect("entrance live").project != right_record.project
        || !allowed(d, by, left)
        || !allowed(d, by, right)
    {
        return refused(to, None, Refusal::Reference, out);
    }
    for (task, other) in [(left, right), (right, left)] {
        let record = record(d, task).expect("both tasks live");
        if !crate::batch::contains(&record.references, other)
            && record.references.len() >= usize::try_from(env.limits.references).expect("u32 fits usize")
        {
            return refused(to, Some(task), Refusal::Busy, out);
        }
    }
    for (task, other) in [(left, right), (right, left)] {
        let mut refs = List::with_capacity(env.limits.references);
        let record = task_mut(d, task).expect("both tasks live");
        for target in &record.record.references {
            refs.push(*target).expect("references admitted");
        }
        if !crate::batch::contains(refs.as_slice(), other) {
            refs.push(other).expect("new reference admitted");
        }
        record.record.references = refs.into_boxed();
        publish(d, env, task, out);
    }
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn forget(d: &mut Domain, env: &Env<Limits>, to: ReplyTo, task: u64, target: u64, out: &mut Queue<Request>) {
    let to = match entrance(d, to, task) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(task), why, out),
    };
    if crate::batch::contains(&record(d, task).expect("entrance live").results_due, target) {
        return refused(to, Some(task), Refusal::Busy, out);
    }
    for (_, sub) in &d.subscriptions {
        if sub.task == task {
            match sub.kind {
                SubscriptionKind::Task { target: subscribed, .. } if subscribed == target => {
                    return refused(to, Some(task), Refusal::Busy, out);
                }
                SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } | SubscriptionKind::Topic { .. } => {}
            }
        }
    }
    let mut refs = List::with_capacity(env.limits.references);
    let record = task_mut(d, task).expect("entrance live");
    for number in &record.record.references {
        if *number != target {
            refs.push(*number).expect("references bounded");
        }
    }
    record.record.references = refs.into_boxed();
    publish(d, env, task, out);
    out.push(Request::Done { reply_to: to });
}
fn arm(d: &mut Domain, env: &Env<Limits>, sub: Subscription) {
    match sub.kind {
        SubscriptionKind::Timer { at, .. } if !sub.pending => {
            let due = env.now.saturating_add(Duration::from_nanos(at.as_nanos().saturating_sub(env.wall.as_nanos())));
            assert!(d.timers.arm(sub.number, due).is_ok(), "one timer per subscription");
        }
        SubscriptionKind::Timer { .. } | SubscriptionKind::Task { .. } | SubscriptionKind::Topic { .. } => {}
    }
}
pub(crate) fn subscribe(d: &mut Domain, env: &Env<Limits>, to: ReplyTo, sub: Subscription, out: &mut Queue<Request>) {
    let to = match entrance(d, to, sub.task) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(sub.task), why, out),
    };
    match record(d, sub.task).expect("entrance live").phase {
        crate::Phase::Closing(_) | crate::Phase::Held { was: crate::Was::Closing(_), .. } | crate::Phase::Ended(_) => {
            return refused(to, Some(sub.task), Refusal::State, out);
        }
        crate::Phase::Waiting
        | crate::Phase::Active(_)
        | crate::Phase::Held { was: crate::Was::Waiting | crate::Was::Active(_), .. } => {}
    }
    if let Some(old) = d.subscriptions.get(&sub.number) {
        if old == &sub {
            return out.push(Request::Done { reply_to: to });
        }
        return refused(to, Some(sub.task), Refusal::KeyConflict, out);
    }
    if sub.number == 0
        || sub.pending
        || (d.subscriptions.len() == d.subscriptions.capacity())
        || !crate::inbox::room(
            d,
            &env.limits,
            sub.task,
            1,
            usize::try_from(env.limits.message_bytes.max(env.limits.result_bytes.saturating_mul(2)))
                .expect("u32 fits usize"),
        )
    {
        return refused(to, Some(sub.task), Refusal::Subscription, out);
    }
    match sub.kind {
        SubscriptionKind::Task { target, .. } => {
            if !references(d, sub.task, target) {
                return refused(to, Some(sub.task), Refusal::Reference, out);
            }
        }
        SubscriptionKind::Timer { period, .. } => {
            if period == Some(Duration::ZERO) {
                return refused(to, Some(sub.task), Refusal::Subscription, out);
            }
        }
        SubscriptionKind::Topic { .. } => {}
    }
    assert!(d.subscriptions.insert(sub.number, sub) == Ok(None), "subscription room admitted");
    out.push(Request::Save { record: Stored::Subscription(sub) });
    arm(d, env, sub);
    match sub.kind {
        SubscriptionKind::Topic { .. } => out.push(Request::Topic { subscription: sub, present: true }),
        SubscriptionKind::Task { target, interest } => {
            let notice = match record(d, target) {
                Some(task) => match task.phase {
                    crate::Phase::Held { why, .. } => match interest {
                        Interest::State | Interest::StateAndResult => Some(Notice::Held(why)),
                        Interest::Result => None,
                    },
                    crate::Phase::Waiting
                    | crate::Phase::Active(_)
                    | crate::Phase::Closing(_)
                    | crate::Phase::Ended(_) => None,
                },
                None => None,
            };
            if let Some(notice) = notice {
                let sub = d.subscriptions.get_mut(&sub.number).expect("new sub exists");
                sub.pending = true;
                out.push(Request::Save { record: Stored::Subscription(*sub) });
                out.push(Request::Notify { subscription: sub.number, notice });
            }
            if d.stubs.contains_key(&target) {
                let sub = d.subscriptions.get_mut(&sub.number).expect("new sub exists");
                sub.pending = true;
                out.push(Request::Save { record: Stored::Subscription(*sub) });
                out.push(Request::Observe { subscription: sub.number, target });
            }
        }
        SubscriptionKind::Timer { .. } => {}
    }
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn unsubscribe(d: &mut Domain, to: ReplyTo, task: u64, number: u64, out: &mut Queue<Request>) {
    let to = match entrance(d, to, task) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(task), why, out),
    };
    if let Some(sub) = d.subscriptions.get(&number)
        && sub.task != task
    {
        return refused(to, Some(task), Refusal::Subscription, out);
    }
    remove(d, number, out);
    out.push(Request::Done { reply_to: to });
}
fn remove(d: &mut Domain, number: u64, out: &mut Queue<Request>) {
    if let Some(sub) = d.subscriptions.remove(&number) {
        d.timers.cancel(number);
        out.push(Request::Erase { key: Key::Subscription(number) });
        match sub.kind {
            SubscriptionKind::Topic { .. } => out.push(Request::Topic { subscription: sub, present: false }),
            SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } => {}
        }
    }
}
pub(crate) fn end(d: &mut Domain, task: u64, out: &mut Queue<Request>) {
    let mut numbers = List::with_capacity(d.subscriptions.len());
    for (number, sub) in &d.subscriptions {
        if sub.task == task {
            numbers.push(*number).expect("subscription snapshot bounded");
        }
    }
    for number in numbers.into_boxed() {
        remove(d, number, out);
    }
}
pub(crate) fn notify(d: &mut Domain, target: u64, notice: Notice, out: &mut Queue<Request>) {
    let mut numbers = List::with_capacity(d.subscriptions.len());
    for (number, sub) in &d.subscriptions {
        match sub.kind {
            SubscriptionKind::Task { target: watched, interest } if watched == target => {
                let relevant = match interest {
                    Interest::State | Interest::StateAndResult => true,
                    Interest::Result => match notice {
                        Notice::Ended(_) => true,
                        Notice::Held(_) => false,
                    },
                };
                if relevant && !sub.pending {
                    numbers.push(*number).expect("subscription snapshot bounded");
                }
            }
            SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } | SubscriptionKind::Topic { .. } => {}
        }
    }
    for number in numbers.into_boxed() {
        let sub = d.subscriptions.get_mut(&number).expect("snapshot sub exists");
        sub.pending = true;
        out.push(Request::Save { record: Stored::Subscription(*sub) });
        out.push(Request::Notify { subscription: number, notice: notice.clone() });
    }
}
fn merge(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    sub: Subscription,
    message: Message,
    out: &mut Queue<Request>,
) {
    let mut old_key = None;
    for (key, old) in &d.messages {
        match old.message {
            Message::News { subscription, .. }
            | Message::Notice { subscription, .. }
            | Message::Timer { subscription, .. }
                if subscription == sub.number =>
            {
                old_key = Some(*key);
                break;
            }
            Message::Words { .. }
            | Message::Question { .. }
            | Message::Answer { .. }
            | Message::Result { .. }
            | Message::News { .. }
            | Message::Notice { .. }
            | Message::Timer { .. } => {}
        }
    }
    let (at, hits, eligible) = match old_key {
        Some(key) => {
            let old = d.messages.remove(&key).expect("merge row exists");
            out.push(Request::Erase { key: Key::Message(key) });
            (old.at, old.hits.saturating_add(1), old.eligible)
        }
        None => (env.wall, 1, false),
    };
    crate::inbox::insert(
        d,
        env,
        Envelope { number, task: sub.task, from: Party::Task(sub.task), message, at, hits, eligible },
        out,
    );
}
pub(crate) fn notice(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    subscription: u64,
    notice: Notice,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    let Some(sub) = d.subscriptions.get(&subscription).copied() else {
        return refused(to, None, Refusal::Subscription, out);
    };
    if number <= record(d, sub.task).expect("sub recipient live").last_message {
        return out.push(Request::Sent { reply_to: to, number, accepted: Accepted::Already });
    }
    let target = match sub.kind {
        SubscriptionKind::Task { target, .. } if sub.pending => target,
        SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } | SubscriptionKind::Topic { .. } => {
            return refused(to, Some(sub.task), Refusal::Subscription, out);
        }
    };
    match &notice {
        Notice::Held(_) => {}
        Notice::Ended(ending) => {
            if !crate::inbox::valid_ending(&env.limits, ending) {
                return refused(to, Some(sub.task), Refusal::Message, out);
            }
        }
    }
    let size = match &notice {
        Notice::Held(_) => 0,
        Notice::Ended(ending) => crate::inbox::ending_bytes(ending),
    };
    if size > usize::try_from(env.limits.result_bytes).expect("u32 fits usize").saturating_mul(2) {
        return refused(to, Some(sub.task), Refusal::Message, out);
    }
    d.subscriptions.get_mut(&subscription).expect("sub exists").pending = false;
    out.push(Request::Save { record: Stored::Subscription(*d.subscriptions.get(&subscription).expect("sub exists")) });
    merge(d, env, number, sub, Message::Notice { subscription, target, notice }, out);
    out.push(Request::Sent { reply_to: to, number, accepted: Accepted::New });
}
pub(crate) fn fire(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if let Some(number) = d.timers.expire(env.now) {
        let sub = d.subscriptions.get_mut(&number).expect("timer sub exists");
        sub.pending = true;
        out.push(Request::Save { record: Stored::Subscription(*sub) });
        out.push(Request::Timer { subscription: number });
    }
}
pub(crate) fn timer(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    subscription: u64,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    let Some(sub) = d.subscriptions.get(&subscription).copied() else {
        return refused(to, None, Refusal::Subscription, out);
    };
    if number <= record(d, sub.task).expect("sub recipient live").last_message {
        return out.push(Request::Sent { reply_to: to, number, accepted: Accepted::Already });
    }
    let (at, period) = match sub.kind {
        SubscriptionKind::Timer { at, period } if sub.pending => (at, period),
        SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } | SubscriptionKind::Topic { .. } => {
            return refused(to, Some(sub.task), Refusal::Subscription, out);
        }
    };
    merge(d, env, number, sub, Message::Timer { subscription, at }, out);
    match period {
        Some(period) => {
            let next = Wall::from_nanos(env.wall.as_nanos().saturating_add(period.as_nanos()));
            let sub = d.subscriptions.get_mut(&subscription).expect("sub exists");
            sub.pending = false;
            sub.kind = SubscriptionKind::Timer { at: next, period: Some(period) };
            let sub = *sub;
            out.push(Request::Save { record: Stored::Subscription(sub) });
            arm(d, env, sub);
        }
        None => remove(d, subscription, out),
    }
    out.push(Request::Sent { reply_to: to, number, accepted: Accepted::New });
}
#[expect(
    clippy::too_many_arguments,
    reason = "the handler carries one complete boundary event plus domain, environment and output"
)]
pub(crate) fn news(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    subscription: u64,
    class: NewsClass,
    words: Box<[u8]>,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    let Some(sub) = d.subscriptions.get(&subscription).copied() else {
        return refused(to, None, Refusal::Subscription, out);
    };
    match sub.kind {
        SubscriptionKind::Topic { .. } => {}
        SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } => {
            return refused(to, Some(sub.task), Refusal::Subscription, out);
        }
    }
    let task = record(d, sub.task).expect("sub recipient live");
    if number <= task.last_message {
        return out.push(Request::Sent { reply_to: to, number, accepted: Accepted::Already });
    }
    if words.len() > usize::try_from(env.limits.message_bytes).expect("u32 fits usize") {
        return refused(to, Some(sub.task), Refusal::Message, out);
    }
    let class = match task.policy.news_ceiling {
        NewsClass::Wakes => class,
        NewsClass::Kept => match class {
            NewsClass::Wakes | NewsClass::Kept => NewsClass::Kept,
            NewsClass::Dropped => NewsClass::Dropped,
        },
        NewsClass::Dropped => NewsClass::Dropped,
    };
    match class {
        NewsClass::Wakes | NewsClass::Kept => {
            merge(d, env, number, sub, Message::News { subscription, class, words }, out);
        }
        NewsClass::Dropped => {
            task_mut(d, sub.task).expect("recipient live").record.last_message = number;
            publish(d, env, sub.task, out);
        }
    }
    out.push(Request::Sent { reply_to: to, number, accepted: Accepted::New });
}
pub(crate) fn restored(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let mut subs = List::with_capacity(d.subscriptions.len());
    for (_, sub) in &d.subscriptions {
        subs.push(*sub).expect("sub snapshot bounded");
    }
    for sub in subs.into_boxed() {
        arm(d, env, sub);
        match sub.kind {
            SubscriptionKind::Topic { .. } => out.push(Request::Topic { subscription: sub, present: true }),
            SubscriptionKind::Task { .. } | SubscriptionKind::Timer { .. } => {}
        }
    }
}
