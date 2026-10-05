//! Durable whole-message inboxes. Root-issued IDs identify committed messages;
//! offers remember exactly which immutable payload an attempt could have read.
use crate::domain::{Domain, entrance, publish, record, refused, task_mut};
use crate::{
    Accepted, Ending, Envelope, Key, Limits, Message, MessageKey, Offer, Party, Phase, Question, Receipt, Refusal,
    Request, Stored, UserMessage,
};
use skein_lib::{Env, List, Queue, ReplyTo};
pub(crate) fn capacity(l: &Limits) -> Option<u32> {
    l.tasks.checked_mul(l.inbox_messages.checked_add(1)?)
}
pub(crate) fn ending_bytes(ending: &Ending) -> usize {
    match ending {
        Ending::Done(result) => crate::run::result_bytes(result),
        Ending::Failed { reason } => reason.len(),
        Ending::Cancelled { reason, result } => reason.len().saturating_add(match result {
            Some(result) => crate::run::result_bytes(result),
            None => 0,
        }),
    }
}
pub(crate) fn bytes(message: &Message) -> usize {
    match message {
        Message::Amendment { reason: words, .. }
        | Message::Words { words }
        | Message::Question { words }
        | Message::Answer { words, .. }
        | Message::News { words, .. } => words.len(),
        Message::Result { ending, .. } => ending_bytes(ending),
        Message::Notice { notice, .. } => match notice {
            crate::Notice::Held(_) => 0,
            crate::Notice::Ended(ending) => ending_bytes(ending),
        },
        Message::Timer { .. } => 0,
    }
}
/// Credits include every accepted result, answer and subscription promise.
/// Cancellation has its own durable phase cell and never needs inbox room.
pub(crate) fn room(d: &Domain, l: &Limits, task: u64, extra: u32, extra_bytes: usize) -> bool {
    let Some(record) = record(d, task) else {
        return false;
    };
    let mut count = extra;
    let mut total = extra_bytes;
    for (_, envelope) in &d.messages {
        if envelope.task == task && !is_amendment(&envelope.message) {
            count = count.saturating_add(1);
            total = total.saturating_add(bytes(&envelope.message));
        }
    }
    for _ in &record.results_due {
        count = count.saturating_add(1);
        total = total.saturating_add(usize::try_from(l.result_bytes).expect("u32 fits usize").saturating_mul(2));
    }
    for (_, question) in &d.questions {
        if question.asker == task {
            count = count.saturating_add(1);
            total = total.saturating_add(usize::try_from(l.message_bytes).expect("u32 fits usize"));
        }
    }
    for (_, subscription) in &d.subscriptions {
        if subscription.task == task {
            let cap = usize::try_from(l.message_bytes.max(l.result_bytes.saturating_mul(2))).expect("u32 fits usize");
            let mut occupied = None;
            for (_, envelope) in &d.messages {
                match envelope.message {
                    Message::News { subscription: number, .. }
                    | Message::Notice { subscription: number, .. }
                    | Message::Timer { subscription: number, .. }
                        if number == subscription.number =>
                    {
                        occupied = Some(bytes(&envelope.message));
                    }
                    Message::Amendment { .. }
                    | Message::Words { .. }
                    | Message::Question { .. }
                    | Message::Answer { .. }
                    | Message::Result { .. }
                    | Message::News { .. }
                    | Message::Notice { .. }
                    | Message::Timer { .. } => {}
                }
            }
            match occupied {
                Some(bytes) => total = total.saturating_add(cap.saturating_sub(bytes)),
                None => {
                    count = count.saturating_add(1);
                    total = total.saturating_add(cap);
                }
            }
        }
    }
    count <= l.inbox_messages && total <= usize::try_from(l.inbox_bytes).expect("u32 fits usize")
}
pub(crate) fn insert(d: &mut Domain, env: &Env<Limits>, envelope: Envelope, out: &mut Queue<Request>) {
    let task = envelope.task;
    let number = envelope.number;
    let saved = d.messages.insert(envelope.key(), envelope.clone());
    assert!(saved == Ok(None), "message room reserved before mutation");
    out.push(Request::Save { record: Stored::Message(envelope) });
    task_mut(d, task).expect("recipient live").record.last_message = number;
    publish(d, env, task, out);
}
#[expect(
    clippy::too_many_arguments,
    reason = "the handler carries one complete boundary event plus domain, environment and output"
)]
pub(crate) fn send(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    task: u64,
    from: Party,
    message: UserMessage,
    out: &mut Queue<Request>,
) {
    let receipt = Receipt { number, task, from, message };
    if !d.ready() {
        return refused(to, Some(task), Refusal::NotReady, out);
    }
    if let Some(old) = d.receipts.get(&number) {
        if old == &receipt {
            return out.push(Request::Sent { reply_to: to, number, accepted: Accepted::Already });
        }
        return refused(to, Some(task), Refusal::KeyConflict, out);
    }
    let Some(target) = record(d, task) else {
        return refused(to, Some(task), Refusal::Unknown, out);
    };
    match &target.phase {
        Phase::Closing(_) | Phase::Held { was: crate::Was::Closing(_), .. } | Phase::Ended(_) => {
            match receipt.message {
                UserMessage::Words { .. } | UserMessage::Question { .. } => {
                    return refused(to, Some(task), Refusal::State, out);
                }
                UserMessage::Answer { .. } => {}
            }
        }
        Phase::Waiting | Phase::Active(_) | Phase::Held { was: crate::Was::Waiting | crate::Was::Active(_), .. } => {}
    }
    if number == 0
        || number <= target.last_message
        || d.receipts.len().saturating_add(d.questions.len()).saturating_add(match receipt.message {
            UserMessage::Words { .. } => 1,
            UserMessage::Question { .. } => 2,
            UserMessage::Answer { .. } => 0,
        }) > d.receipts.capacity()
    {
        return refused(to, Some(task), Refusal::Busy, out);
    }
    if match receipt.message {
        UserMessage::Words { .. } | UserMessage::Question { .. } => !crate::refs::allowed(d, from, task),
        UserMessage::Answer { .. } => false,
    } {
        return refused(to, Some(task), Refusal::Reference, out);
    }
    let size = match &receipt.message {
        UserMessage::Words { words } | UserMessage::Question { words } | UserMessage::Answer { words, .. } => {
            words.len()
        }
    };
    if size > usize::try_from(env.limits.message_bytes).expect("u32 fits usize") {
        return refused(to, Some(task), Refusal::Message, out);
    }
    let (payload, question, answered) = match &receipt.message {
        UserMessage::Words { words } => (Message::Words { words: words.clone() }, None, None),
        UserMessage::Question { words } => {
            let asker = match from {
                Party::Task(asker) => asker,
                Party::Person(_) | Party::Deployment { .. } => return refused(to, Some(task), Refusal::Question, out),
            };
            if (d.questions.len() == d.questions.capacity())
                || !room(d, &env.limits, asker, 1, usize::try_from(env.limits.message_bytes).expect("u32 fits usize"))
            {
                return refused(to, Some(task), Refusal::Inbox, out);
            }
            (Message::Question { words: words.clone() }, Some(Question { number, asker, answerer: task }), None)
        }
        UserMessage::Answer { question, words } => {
            let Some(old) = d.questions.get(question) else {
                return refused(to, Some(task), Refusal::Question, out);
            };
            if old.asker != task || from != Party::Task(old.answerer) {
                return refused(to, Some(task), Refusal::Question, out);
            }
            (Message::Answer { question: *question, words: words.clone() }, None, Some(*question))
        }
    };
    if answered.is_none() && !room(d, &env.limits, task, 1, size) {
        return refused(to, Some(task), Refusal::Inbox, out);
    }
    if let Some(question) = question {
        assert!(d.questions.insert(number, question) == Ok(None), "question room admitted");
        out.push(Request::Save { record: Stored::Question(question) });
    }
    if let Some(question) = answered {
        d.questions.remove(&question);
        out.push(Request::Erase { key: Key::Question(question) });
    }
    insert(d, env, Envelope { number, task, from, message: payload, at: env.wall, hits: 1, eligible: false }, out);
    assert!(d.receipts.insert(number, receipt.clone()) == Ok(None), "receipt room admitted");
    out.push(Request::Save { record: Stored::Receipt(receipt) });
    out.push(Request::Sent { reply_to: to, number, accepted: Accepted::New });
}
pub(crate) fn peek(d: &Domain, to: ReplyTo, task: u64, budget: u32, out: &mut Queue<Request>) {
    let to = match entrance(d, to, task) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(task), why, out),
    };
    let mut picked = List::with_capacity(d.messages.len());
    let mut used = 0_usize;
    let mut more = false;
    for (_, message) in &d.messages {
        if message.task == task {
            let next = used.saturating_add(bytes(&message.message));
            if next > usize::try_from(budget).expect("u32 fits usize") {
                more = true;
                break;
            }
            used = next;
            picked.push(message.clone()).expect("bounded inbox snapshot");
        }
    }
    out.push(Request::Inbox { reply_to: to, messages: picked.into_boxed(), more });
}
pub(crate) fn claimable(d: &Domain, task: u64, readable: &[u64]) -> bool {
    if readable.len() > usize::try_from(d.messages.capacity()).expect("u32 fits usize") {
        return false;
    }
    let mut ordinary = 0_u32;
    let mut controls = 0_u32;
    for (at, number) in readable.iter().enumerate() {
        let Some(envelope) = d.messages.get(&MessageKey { task, number: *number }) else {
            return false;
        };
        for earlier in readable.iter().take(at) {
            if earlier == number {
                return false;
            }
        }
        if is_amendment(&envelope.message) {
            controls = controls.saturating_add(1);
        } else {
            ordinary = ordinary.saturating_add(1);
        }
    }
    offer_room(d, task, false, ordinary) && offer_room(d, task, true, controls)
}
/// Two immutable control offers per task: its current amendment and one prior
/// merged payload. A third amendment refuses at its entrance until a read or
/// terminal frees room, while ordinary offer pressure cannot delay a control.
pub(crate) fn offer_capacity(l: &Limits) -> Option<u32> {
    l.offers.checked_add(l.tasks.checked_mul(2)?)
}
pub(crate) fn offer_room(d: &Domain, task: u64, control: bool, extra: u32) -> bool {
    let mut used = 0_u32;
    for (_, offer) in &d.offers {
        if control {
            if offer.envelope.task == task && is_amendment(&offer.envelope.message) {
                used = used.saturating_add(1);
            }
        } else if !is_amendment(&offer.envelope.message) {
            used = used.saturating_add(1);
        }
    }
    let cap = if control { 2 } else { d.offers.capacity().saturating_sub(d.tasks.capacity().saturating_mul(2)) };
    used.saturating_add(extra) <= cap
}

pub(crate) fn offer(d: &mut Domain, task: u64, attempt: u64, numbers: &[u64], out: &mut Queue<Request>) {
    for number in numbers {
        let key = MessageKey { task, number: *number };
        let envelope = d.messages.get(&key).expect("offered inbox row exists").clone();
        let offer = Offer { attempt, envelope };
        assert!(d.offers.insert(key, offer.clone()) == Ok(None), "offer capacity admitted");
        out.push(Request::Save { record: Stored::Offer(offer) });
    }
}
pub(crate) fn clear_offers(d: &mut Domain, task: u64, out: &mut Queue<Request>) {
    let mut keys = List::with_capacity(d.offers.len());
    for (key, _) in &d.offers {
        if key.task == task {
            keys.push(*key).expect("offer snapshot bounded");
        }
    }
    for key in keys.into_boxed() {
        d.offers.remove(&key);
        out.push(Request::Erase { key: Key::Offer(key) });
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "the handler carries one complete boundary event plus domain, environment and output"
)]
pub(crate) fn turn(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    task: u64,
    attempt: u64,
    turn: u32,
    read: Option<u64>,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, task) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(task), why, out),
    };
    let old = record(d, task).expect("entrance names task");
    if old.attempt != attempt || attempt == 0 {
        return refused(to, Some(task), Refusal::Attempt, out);
    }
    if turn != 0 && turn <= old.turn {
        return out.push(Request::TurnAcknowledged { reply_to: to, task, attempt, turn, accepted: Accepted::Already });
    }
    if crate::run::run_attempt(&old.phase) != Some(attempt) || old.turn.checked_add(1) != Some(turn) {
        return refused(to, Some(task), Refusal::Turn, out);
    }
    if let Some(number) = read
        && old.last_read != Some(number)
        && match d.offers.get(&MessageKey { task, number }) {
            Some(offer) => offer.attempt != attempt,
            None => true,
        }
    {
        return refused(to, Some(task), Refusal::Read, out);
    }
    let mut keys = List::with_capacity(d.offers.len());
    for (key, offer) in &d.offers {
        if key.task == task
            && offer.attempt == attempt
            && match read {
                Some(number) => key.number <= number,
                None => false,
            }
        {
            keys.push(*key).expect("offer snapshot bounded");
        }
    }
    for key in keys.into_boxed() {
        d.offers.remove(&key);
        out.push(Request::Erase { key: Key::Offer(key) });
        if d.messages.remove(&key).is_some() {
            out.push(Request::Erase { key: Key::Message(key) });
        }
    }
    let old = task_mut(d, task).expect("turn recipient live");
    old.record.turn = turn;
    old.record.last_read = read;
    publish(d, env, task, out);
    out.push(Request::TurnAcknowledged { reply_to: to, task, attempt, turn, accepted: Accepted::New });
}
#[expect(
    clippy::too_many_arguments,
    reason = "the handler carries one complete boundary event plus domain, environment and output"
)]
pub(crate) fn result(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    task: u64,
    delegate: u64,
    ending: Ending,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, task) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(task), why, out),
    };
    let old = record(d, task).expect("recipient exists");
    if number <= old.last_message {
        return out.push(Request::Sent { reply_to: to, number, accepted: Accepted::Already });
    }
    if !crate::batch::contains(&old.results_due, delegate)
        || d.names.contains_key(&delegate)
        || !valid_ending(&env.limits, &ending)
    {
        return refused(to, Some(task), Refusal::Message, out);
    }
    let mut due = List::with_capacity(env.limits.delegates);
    for candidate in &old.results_due {
        if *candidate != delegate {
            due.push(*candidate).expect("credits bounded");
        }
    }
    task_mut(d, task).expect("recipient exists").record.results_due = due.into_boxed();
    insert(
        d,
        env,
        Envelope {
            number,
            task,
            from: Party::Task(delegate),
            message: Message::Result { task: delegate, ending },
            at: env.wall,
            hits: 1,
            eligible: false,
        },
        out,
    );
    out.push(Request::Sent { reply_to: to, number, accepted: Accepted::New });
}
pub(crate) fn forget_receipt(d: &mut Domain, to: ReplyTo, number: u64, out: &mut Queue<Request>) {
    if !d.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    if d.receipts.remove(&number).is_some() {
        out.push(Request::Erase { key: Key::Receipt(number) });
    }
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn archive(d: &mut Domain, task: u64, out: &mut Queue<Request>) {
    clear_offers(d, task, out);
    let mut keys = List::with_capacity(d.messages.len());
    for (key, _) in &d.messages {
        if key.task == task {
            keys.push(*key).expect("inbox snapshot bounded");
        }
    }
    for key in keys.into_boxed() {
        let envelope = d.messages.remove(&key).expect("snapshot row exists");
        out.push(Request::Erase { key: Key::Message(key) });
        out.push(Request::Save { record: Stored::ArchivedMessage(envelope) });
    }
    let mut questions = List::with_capacity(d.questions.len());
    for (number, question) in &d.questions {
        if question.asker == task || question.answerer == task {
            questions.push(*number).expect("question snapshot bounded");
        }
    }
    for number in questions.into_boxed() {
        d.questions.remove(&number);
        out.push(Request::Erase { key: Key::Question(number) });
    }
}
pub(crate) fn valid_ending(l: &Limits, ending: &Ending) -> bool {
    let cap = usize::try_from(l.result_bytes).expect("u32 fits usize");
    match ending {
        Ending::Done(result) => crate::run::result_bytes(result) <= cap,
        Ending::Failed { reason } => reason.len() <= cap,
        Ending::Cancelled { reason, result } => {
            reason.len() <= cap
                && match result {
                    Some(result) => crate::run::result_bytes(result) <= cap,
                    None => true,
                }
        }
    }
}
fn valid_envelope(l: &Limits, envelope: &Envelope) -> bool {
    if envelope.number == 0 || envelope.hits == 0 {
        return false;
    }
    match &envelope.message {
        Message::Words { words } | Message::Question { words } => {
            words.len() <= usize::try_from(l.message_bytes).expect("u32 fits usize")
        }
        Message::Answer { question, words } => {
            *question != 0 && words.len() <= usize::try_from(l.message_bytes).expect("u32 fits usize")
        }
        Message::News { class, words, .. } => {
            *class != crate::NewsClass::Dropped
                && words.len() <= usize::try_from(l.message_bytes).expect("u32 fits usize")
        }
        Message::Result { ending, .. } => valid_ending(l, ending),
        Message::Notice { notice, .. } => match notice {
            crate::Notice::Held(_) => true,
            crate::Notice::Ended(ending) => valid_ending(l, ending),
        },
        Message::Timer { .. } => true,
        Message::Amendment { revision, reason } => {
            envelope.eligible
                && *revision != 0
                && reason.len() <= usize::try_from(l.message_bytes).expect("u32 fits usize")
        }
    }
}
pub(crate) fn restore(d: &mut Domain, env: &Env<Limits>, stored: Stored) -> bool {
    match stored {
        Stored::Message(envelope) => {
            if !valid_envelope(&env.limits, &envelope) || d.messages.contains_key(&envelope.key()) {
                return false;
            }
            d.messages.insert(envelope.key(), envelope).is_ok()
        }
        Stored::Offer(offer) => {
            if offer.attempt == 0
                || !valid_envelope(&env.limits, &offer.envelope)
                || d.offers.contains_key(&offer.envelope.key())
            {
                return false;
            }
            d.offers.insert(offer.envelope.key(), offer).is_ok()
        }
        Stored::Receipt(receipt) => {
            let size = match &receipt.message {
                UserMessage::Words { words } | UserMessage::Question { words } | UserMessage::Answer { words, .. } => {
                    words.len()
                }
            };
            if receipt.number == 0
                || size > usize::try_from(env.limits.message_bytes).expect("u32 fits usize")
                || d.receipts.contains_key(&receipt.number)
            {
                return false;
            }
            d.receipts.insert(receipt.number, receipt).is_ok()
        }
        Stored::Question(question) => {
            if question.number == 0 || question.asker == question.answerer || d.questions.contains_key(&question.number)
            {
                return false;
            }
            d.questions.insert(question.number, question).is_ok()
        }
        Stored::Subscription(sub) => {
            if sub.number == 0 || sub.pending || d.subscriptions.contains_key(&sub.number) {
                return false;
            }
            match sub.kind {
                crate::SubscriptionKind::Timer { period: Some(period), .. } if period == skein_lib::Duration::ZERO => {
                    return false;
                }
                crate::SubscriptionKind::Task { .. }
                | crate::SubscriptionKind::Timer { .. }
                | crate::SubscriptionKind::Topic { .. } => {}
            }
            d.subscriptions.insert(sub.number, sub).is_ok()
        }
        Stored::History(_)
        | Stored::Closure(_)
        | Stored::Funding { .. }
        | Stored::ArchivedMessage(_)
        | Stored::Live(_)
        | Stored::Ended(_)
        | Stored::Stub(_) => false,
    }
}
pub(crate) fn links(d: &Domain, env: &Env<Limits>) -> bool {
    if d.receipts.len().saturating_add(d.questions.len()) > env.limits.receipts {
        return false;
    }
    if !offer_room(d, 0, false, 0) {
        return false;
    }
    for (number, _) in &d.names {
        if !offer_room(d, *number, true, 0) {
            return false;
        }
        if !room(d, &env.limits, *number, 0, 0) {
            return false;
        }
        let task = record(d, *number).expect("indexed live");
        let mut amendments = 0_u32;
        for (_, envelope) in &d.messages {
            if envelope.task == *number && is_amendment(&envelope.message) {
                amendments = amendments.saturating_add(1);
            }
        }
        if amendments > 1 {
            return false;
        }
        for delegate in &task.delegates {
            if !crate::batch::contains(&task.results_due, *delegate) {
                return false;
            }
        }
        for credit in &task.results_due {
            if !crate::batch::contains(&task.delegates, *credit) {
                return false;
            }
        }
    }
    for (_, envelope) in &d.messages {
        let Some(task) = record(d, envelope.task) else {
            return false;
        };
        match envelope.message {
            Message::Amendment { revision, .. } if revision > task.revision => return false,
            Message::Amendment { .. }
            | Message::Words { .. }
            | Message::Question { .. }
            | Message::Answer { .. }
            | Message::News { .. }
            | Message::Result { .. }
            | Message::Notice { .. }
            | Message::Timer { .. } => {}
        }
        if envelope.number > task.last_message {
            return false;
        }
    }
    for (_, offer) in &d.offers {
        let Some(task) = record(d, offer.envelope.task) else {
            return false;
        };
        if crate::run::run_attempt(&task.phase) != Some(offer.attempt) || offer.envelope.number > task.last_message {
            return false;
        }
    }
    for (_, question) in &d.questions {
        if record(d, question.asker).is_none() || record(d, question.answerer).is_none() {
            return false;
        }
    }
    for (_, sub) in &d.subscriptions {
        if record(d, sub.task).is_none() {
            return false;
        }
        match sub.kind {
            crate::SubscriptionKind::Task { target, .. } => {
                if !crate::refs::references(d, sub.task, target) {
                    return false;
                }
            }
            crate::SubscriptionKind::Timer { .. } | crate::SubscriptionKind::Topic { .. } => {}
        }
    }
    true
}

pub(crate) fn is_amendment(message: &Message) -> bool {
    match message {
        Message::Amendment { .. } => true,
        Message::Words { .. }
        | Message::Question { .. }
        | Message::Answer { .. }
        | Message::Result { .. }
        | Message::News { .. }
        | Message::Notice { .. }
        | Message::Timer { .. } => false,
    }
}
