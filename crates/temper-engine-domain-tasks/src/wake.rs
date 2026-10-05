//! Wake policy is inert data; one bounded pass chooses live relays or a wake.
use crate::domain::{Alarm, Domain, activate, publish, record, snapshot, task_mut};
use crate::{Active, Ending, Envelope, Limits, Message, NewsClass, Party, Phase, Request, ResultsWake, Rule};
use skein_lib::{Duration, Env, List, Queue, Wall};
pub(crate) fn valid(policy: &crate::WakePolicy) -> bool {
    for rule in [policy.words, policy.news, policy.notices] {
        match rule {
            Rule::Never | Rule::Immediate => {}
            Rule::Batch { count, age } => {
                if count == 0 || age == Duration::ZERO {
                    return false;
                }
            }
        }
    }
    true
}
fn rule(d: &Domain, envelope: &Envelope) -> Rule {
    let task = record(d, envelope.task).expect("inbox recipient live");
    match &envelope.message {
        Message::Words { .. } => match envelope.from {
            Party::Person(_) => Rule::Immediate,
            Party::Task(_) | Party::Deployment { .. } => task.policy.words,
        },
        Message::Question { .. } => {
            if task.policy.questions {
                Rule::Immediate
            } else {
                Rule::Never
            }
        }
        Message::Answer { .. } => {
            if task.policy.answers {
                Rule::Immediate
            } else {
                Rule::Never
            }
        }
        Message::Result { ending, .. } => match task.policy.results {
            ResultsWake::Never => Rule::Never,
            ResultsWake::Each => Rule::Immediate,
            ResultsWake::LastOrFailure => match ending {
                Ending::Failed { .. } | Ending::Cancelled { .. } => Rule::Immediate,
                Ending::Done(_) => {
                    if task.delegates.is_empty() && task.results_due.is_empty() {
                        Rule::Immediate
                    } else {
                        Rule::Never
                    }
                }
            },
        },
        Message::News { class, .. } => match class {
            NewsClass::Wakes => task.policy.news,
            NewsClass::Kept | NewsClass::Dropped => Rule::Never,
        },
        Message::Notice { .. } => task.policy.notices,
        Message::Timer { .. } => {
            if task.policy.timers {
                Rule::Immediate
            } else {
                Rule::Never
            }
        }
    }
}
fn kind(message: &Message) -> u8 {
    match message {
        Message::Words { .. } => 0,
        Message::Question { .. } => 1,
        Message::Answer { .. } => 2,
        Message::Result { .. } => 3,
        Message::News { .. } => 4,
        Message::Notice { .. } => 5,
        Message::Timer { .. } => 6,
    }
}
fn batch(d: &Domain, envelope: &Envelope, age: Duration) -> (u32, Wall, bool) {
    let mut hits = 0_u32;
    let mut oldest = envelope.at;
    let mut latched = false;
    for (_, candidate) in &d.messages {
        if candidate.task == envelope.task
            && kind(&candidate.message) == kind(&envelope.message)
            && match rule(d, candidate) {
                Rule::Batch { .. } => true,
                Rule::Never | Rule::Immediate => false,
            }
        {
            hits = hits.saturating_add(candidate.hits);
            oldest = oldest.min(candidate.at);
            latched |= candidate.eligible;
        }
    }
    (hits, Wall::from_nanos(oldest.as_nanos().saturating_add(age.as_nanos())), latched)
}
fn batched(
    d: &Domain,
    env: &Env<Limits>,
    envelope: &Envelope,
    count: u32,
    age: Duration,
    old: &[Option<Alarm>; 3],
    alarms: &mut [Option<Alarm>; 3],
) -> bool {
    let (hits, until, latched) = batch(d, envelope, age);
    let slot = match &envelope.message {
        Message::Words { .. } => 0,
        Message::News { .. } => 1,
        Message::Notice { .. } => 2,
        Message::Question { .. } | Message::Answer { .. } | Message::Result { .. } | Message::Timer { .. } => {
            unreachable!("only these three kinds may batch")
        }
    };
    let due = match *old.get(slot).expect("batch slot exists") {
        Some(alarm) if alarm.until == until => alarm.due,
        Some(_) | None => {
            env.now.saturating_add(Duration::from_nanos(until.as_nanos().saturating_sub(env.wall.as_nanos())))
        }
    };
    if latched || hits >= count || due <= env.now {
        true
    } else {
        let alarm = Alarm { until, due };
        let next = alarms.get_mut(slot).expect("batch slot exists");
        *next = match *next {
            Some(old) if old.due <= due => Some(old),
            Some(_) | None => Some(alarm),
        };
        false
    }
}
pub(crate) fn progress(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let numbers = snapshot(d, env.limits.tasks);
    for number in numbers.into_boxed() {
        let phase = record(d, number).expect("snapshot live").phase.clone();
        let running = match phase {
            Phase::Active(Active::Running { attempt }) => Some(attempt),
            Phase::Waiting
            | Phase::Active(
                Active::Idle | Active::Due | Active::Preparing | Active::Claimed { .. } | Active::BackingOff { .. },
            )
            | Phase::Closing(_)
            | Phase::Held { .. }
            | Phase::Ended(_) => None,
        };
        let old_alarms =
            d.tasks.get(*d.names.get(&number).expect("snapshot live")).expect("name indexes live").wake_alarms;
        let mut eligible = List::with_capacity(env.limits.inbox_messages);
        let mut alarms: [Option<Alarm>; 3] = [None; 3];
        for (key, envelope) in &d.messages {
            if key.task != number {
                continue;
            }
            let ready = match rule(d, envelope) {
                Rule::Never => false,
                Rule::Immediate => true,
                Rule::Batch { count, age } => batched(d, env, envelope, count, age, &old_alarms, &mut alarms),
            };
            if ready {
                eligible.push(key.number).expect("per-task inbox bounded");
            }
        }
        // Persist reached thresholds, including while held. A wall correction
        // or restart cannot make an already eligible hint wait again.
        for message in &eligible {
            let key = crate::MessageKey { task: number, number: *message };
            let envelope = d.messages.get_mut(&key).expect("eligible row exists");
            if !envelope.eligible {
                envelope.eligible = true;
                out.push(Request::Save { record: crate::Stored::Message(envelope.clone()) });
            }
        }
        task_mut(d, number).expect("snapshot live").wake_alarms = alarms;
        let mut next: Option<Alarm> = None;
        for alarm in alarms.into_iter().flatten() {
            next = match next {
                Some(old) if old.due <= alarm.due => Some(old),
                Some(_) | None => Some(alarm),
            };
        }
        match next {
            Some(alarm) => assert!(d.wakes.arm(number, alarm.due).is_ok(), "one wake alarm per task"),
            None => {
                d.wakes.cancel(number);
            }
        }
        if !eligible.is_empty() && phase == Phase::Active(Active::Idle) {
            task_mut(d, number).expect("snapshot live").record.phase = Phase::Active(Active::Due);
            publish(d, env, number, out);
            activate(d, number, out);
        }
        if let Some(attempt) = running {
            for message in eligible.into_boxed() {
                let key = crate::MessageKey { task: number, number: message };
                if d.offers.contains_key(&key) || (d.offers.len() == d.offers.capacity()) {
                    continue;
                }
                crate::inbox::offer(d, number, attempt, &[message], out);
                out.push(Request::Relay {
                    task: number,
                    attempt,
                    envelope: d.messages.get(&key).expect("eligible inbox row exists").clone(),
                });
            }
        }
    }
}
