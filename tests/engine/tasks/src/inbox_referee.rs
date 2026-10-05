//! Independent root observations: persisted rows and committed turns, never
//! child state or its admission/byte-counting helpers.
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain_tasks::{self as tasks, Ending, Key, Limits, Message, Stored};
#[derive(Default, Debug)]
pub struct Inbox {
    before: BTreeMap<Key, Stored>,
}
fn size(ending: &Ending) -> usize {
    fn result(result: &tasks::Result) -> usize {
        match result {
            tasks::Result::Report { words }
            | tasks::Result::Verdict { words, .. }
            | tasks::Result::Change { words, .. } => words.len(),
            tasks::Result::Failure { reason } => reason.len(),
        }
    }
    match ending {
        Ending::Done(value) => result(value),
        Ending::Failed { reason } => reason.len(),
        Ending::Cancelled { reason, result: value } => reason.len() + value.as_ref().map_or(0, result),
    }
}
fn bytes(message: &Message) -> usize {
    match message {
        Message::Amendment { reason: words, .. }
        | Message::Words { words }
        | Message::Question { words }
        | Message::Answer { words, .. }
        | Message::News { words, .. } => words.len(),
        Message::Result { ending, .. } => size(ending),
        Message::Notice { notice, .. } => match notice {
            tasks::Notice::Ended(ending) => size(ending),
            tasks::Notice::Held(_) => 0,
        },
        Message::Timer { .. } => 0,
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Read {
    pub task: u64,
    pub attempt: u64,
    pub turn: u32,
    pub through: Option<u64>,
}
fn occupying(rows: &BTreeMap<Key, Stored>, number: u64) -> Option<usize> {
    rows.values().find_map(|row| match row {
        Stored::Message(envelope) => match envelope.message {
            Message::News { subscription, .. }
            | Message::Notice { subscription, .. }
            | Message::Timer { subscription, .. } => (subscription == number).then_some(bytes(&envelope.message)),
            Message::Amendment { .. }
            | Message::Words { .. }
            | Message::Question { .. }
            | Message::Answer { .. }
            | Message::Result { .. } => None,
        },
        Stored::Live(_)
        | Stored::Ended(_)
        | Stored::Stub(_)
        | Stored::History(_)
        | Stored::Closure(_)
        | Stored::Funding { .. }
        | Stored::ArchivedMessage(_)
        | Stored::Receipt(_)
        | Stored::Offer(_)
        | Stored::Question(_)
        | Stored::Subscription(_) => None,
    })
}
fn references(task: &tasks::TaskRecord, l: &Limits) -> Result<(), &'static str> {
    let mut unique = BTreeSet::new();
    for reference in &task.references {
        if !unique.insert(reference) {
            return Err("duplicate reference");
        }
    }
    if task.references.len() > l.references as usize {
        return Err("reference capacity");
    }
    Ok(())
}
impl Inbox {
    /// # Errors
    /// Names the independently observed invariant broken by a durable decision.
    #[expect(clippy::too_many_lines, reason = "exhaustive independent checks of durable inbox rows")]
    pub fn committed(&mut self, rows: &BTreeMap<Key, Stored>, l: &Limits, reads: &[Read]) -> Result<(), &'static str> {
        let live = rows
            .values()
            .filter_map(|row| match row {
                Stored::Live(task) => Some((task.number, task)),
                Stored::Ended(_)
                | Stored::Stub(_)
                | Stored::Message(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. }
                | Stored::ArchivedMessage(_)
                | Stored::Receipt(_)
                | Stored::Offer(_)
                | Stored::Question(_)
                | Stored::Subscription(_) => None,
            })
            .collect::<BTreeMap<_, _>>();
        let mut controls = BTreeSet::new();
        let mut counts = BTreeMap::<u64, (usize, usize)>::new();
        let mut receipts = 0;
        let mut questions = 0;
        let mut subscriptions = 0;
        let mut offers = 0;
        for (key, row) in rows {
            if *key != row.key() {
                return Err("stored row key mismatch");
            }
            match row {
                Stored::Message(envelope) => {
                    let task = live.get(&envelope.task).ok_or("inbox recipient ended")?;
                    if envelope.number == 0 || envelope.number > task.last_message || envelope.hits == 0 {
                        return Err("invalid message identity");
                    }
                    if matches!(envelope.message, Message::News { class: tasks::NewsClass::Dropped, .. }) {
                        return Err("dropped news persisted");
                    }
                    if let Message::Amendment { revision, reason } = &envelope.message
                        && (!controls.insert(envelope.task)
                            || *revision == 0
                            || *revision > task.revision
                            || !envelope.eligible
                            || reason.len() > l.message_bytes as usize)
                    {
                        return Err("invalid amendment control slot");
                    }
                    if !matches!(envelope.message, Message::Amendment { .. }) {
                        let entry = counts.entry(envelope.task).or_default();
                        entry.0 += 1;
                        entry.1 += bytes(&envelope.message);
                    }
                }
                Stored::Offer(offer) => {
                    offers += 1;
                    let task = live.get(&offer.envelope.task).ok_or("offer recipient ended")?;
                    if task.attempt != offer.attempt || offer.attempt == 0 || offer.envelope.number > task.last_message
                    {
                        return Err("offer fences wrong attempt");
                    }
                }
                Stored::Question(question) => {
                    questions += 1;
                    if !live.contains_key(&question.asker) || !live.contains_key(&question.answerer) {
                        return Err("open question participant ended");
                    }
                    let entry = counts.entry(question.asker).or_default();
                    entry.0 += 1;
                    entry.1 += l.message_bytes as usize;
                }
                Stored::Subscription(sub) => {
                    subscriptions += 1;
                    if sub.pending {
                        return Err("root callback escaped decision");
                    }
                    if !live.contains_key(&sub.task) {
                        return Err("subscription recipient ended");
                    }
                    let occupying = occupying(rows, sub.number);
                    let cap = l.message_bytes.max(l.result_bytes * 2) as usize;
                    let entry = counts.entry(sub.task).or_default();
                    if let Some(bytes) = occupying {
                        entry.1 += cap.saturating_sub(bytes);
                    } else {
                        entry.0 += 1;
                        entry.1 += cap;
                    }
                }
                Stored::Receipt(_) => receipts += 1,
                Stored::Live(task) => {
                    references(task, l)?;
                    if task.results_due.iter().any(|number| !task.delegates.contains(number)) {
                        return Err("undelivered result escaped decision");
                    }
                    let entry = counts.entry(task.number).or_default();
                    entry.0 += task.results_due.len();
                    entry.1 += task.results_due.len() * l.result_bytes as usize * 2;
                }
                Stored::Ended(_)
                | Stored::Stub(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. }
                | Stored::ArchivedMessage(_) => {}
            }
        }
        if offers > l.offers as usize
            || questions > l.questions as usize
            || subscriptions > l.subscriptions as usize
            || receipts + questions > l.receipts as usize
        {
            return Err("message arena capacity");
        }
        if counts.values().any(|(count, bytes)| *count > l.inbox_messages as usize || *bytes > l.inbox_bytes as usize) {
            return Err("inbox promised capacity");
        }
        self.check_reads(rows, reads)?;
        self.before = rows.clone();
        Ok(())
    }
    fn check_reads(&self, rows: &BTreeMap<Key, Stored>, reads: &[Read]) -> Result<(), &'static str> {
        for read in reads {
            let old = match self.before.get(&Key::Live(read.task)) {
                Some(Stored::Live(task)) => task,
                Some(
                    Stored::Ended(_)
                    | Stored::Stub(_)
                    | Stored::Message(_)
                    | Stored::History(_)
                    | Stored::Closure(_)
                    | Stored::Funding { .. }
                    | Stored::ArchivedMessage(_)
                    | Stored::Receipt(_)
                    | Stored::Offer(_)
                    | Stored::Question(_)
                    | Stored::Subscription(_),
                )
                | None => return Err("turn missing prior task"),
            };
            if read.turn <= old.turn {
                continue;
            }
            if old.attempt != read.attempt || old.turn.checked_add(1) != Some(read.turn) {
                return Err("turn fence or order");
            }
            if let Some(number) = read.through
                && old.last_read != Some(number)
                && !matches!(self.before.get(&Key::Offer(tasks::MessageKey { task: read.task, number })), Some(Stored::Offer(offer)) if offer.attempt == read.attempt)
            {
                return Err("read was never offered");
            }
            for (key, row) in &self.before {
                if let Stored::Message(envelope) = row
                    && envelope.task == read.task
                {
                    let offered = matches!(self.before.get(&Key::Offer(envelope.key())), Some(Stored::Offer(offer)) if offer.attempt == read.attempt);
                    let should_take = offered && read.through.is_some_and(|number| envelope.number <= number);
                    if rows.contains_key(key) == should_take {
                        return Err("turn took deferred or retained read message");
                    }
                }
            }
            if let Some(Stored::Live(task)) = rows.get(&Key::Live(read.task))
                && (task.turn != read.turn || task.last_read != read.through)
            {
                return Err("turn record missing");
            }
        }
        Ok(())
    }
    pub fn reset(&mut self, rows: &BTreeMap<Key, Stored>) {
        self.before = rows.clone();
    }
}
