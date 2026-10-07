//! Derived person results and their committed read positions (domain/tasks.md, 5.6;
//! domain/people.md, 6). Historical ended tasks remain the only result records.

use super::{Domain, Read as RootRead, Request, admits, close, emit, ending_words, request_load, save, take_read};
use crate::{Decision, Delivery, Key, Range, Record, ResultEntry, Write};
use alloc::boxed::Box;
use jig_core_people as people;
use skein_lib::{Env, Id, List, Queue, ReplyTo, Token};
use temper_engine_domain_tasks as tasks;

#[derive(Debug)]
pub(super) enum Query {
    Named { task: u64 },
    Inbox { most: u32 },
}

#[derive(Debug)]
pub(super) struct Read {
    pub(super) to: ReplyTo,
    pub(super) person: u64,
    position: u64,
    earliest: u64,
    query: Query,
    target: Option<ResultEntry>,
    entries: List<ResultEntry>,
}

fn refuse(to: ReplyTo, why: people::Refusal, out: &mut Queue<Request>) {
    out.push(Request::Deliver(Delivery::WebReply { to, sign_in: None, reply: people::Reply::Refused(why) }));
}

pub(super) fn begin(
    domain: &mut Domain,
    env: &Env<super::Limits>,
    to: ReplyTo,
    sign_in: u64,
    query: Query,
    out: &mut Queue<Request>,
) {
    let valid = match query {
        Query::Named { task } => task != 0,
        Query::Inbox { most } => most != 0 && env.limits.people.inbox_entries != 0,
    };
    if !valid {
        return refuse(to, people::Refusal::Limit, out);
    }
    let Some(person) = domain.people.person(sign_in, env.now, env.wall) else {
        return refuse(to, people::Refusal::SignIn, out);
    };
    if !domain.ready() || !admits(domain, &env.limits) || domain.reading_results.contains_key(&person) {
        return refuse(to, people::Refusal::Busy, out);
    }
    let position = domain.people.read_position(person).expect("authenticated person restored");
    let cap = match query {
        Query::Named { .. } => 0,
        Query::Inbox { most } => most.min(env.limits.people.inbox_entries),
    };
    let read =
        Read { to, person, position, earliest: u64::MAX, query, target: None, entries: List::with_capacity(cap) };
    let id = match domain.result_reads.insert(Some(RootRead::Result(read))) {
        Ok(id) => id,
        Err(Some(RootRead::Result(read))) => return refuse(read.to, people::Refusal::Busy, out),
        Err(
            Some(
                RootRead::Escalation(_)
                | RootRead::Proposal(_)
                | RootRead::Inbox(_)
                | RootRead::Transcript { .. }
                | RootRead::Dependency(_)
                | RootRead::InputCheck(_),
            )
            | None,
        ) => unreachable!("inserted result read"),
    };
    let indexed = domain.reading_results.insert(person, id.token());
    assert!(indexed == Ok(None), "one result read per authenticated person");
    let mut decision = Decision::new(&env.limits.journal);
    emit(&mut decision, &env.limits, Delivery::ReadResult { waiter: id.token() });
    close(domain, env, decision, out);
}

pub(super) fn failed(domain: &mut Domain, waiter: Token, why: people::Refusal, out: &mut Queue<Request>) {
    let Some(read) = take_read(domain, waiter) else { return };
    let read = match read {
        RootRead::Result(read) => read,
        RootRead::Inbox(_)
        | RootRead::Escalation(_)
        | RootRead::Proposal(_)
        | RootRead::Transcript { .. }
        | RootRead::Dependency(_)
        | RootRead::InputCheck(_) => unreachable!("result load owns result read"),
    };
    let removed = domain.reading_results.remove(&read.person);
    assert!(removed == Some(waiter), "result reader index names its waiter");
    domain.result_reads.retire(Id::from_token(waiter));
    refuse(read.to, why, out);
}

#[expect(clippy::too_many_lines, reason = "one bounded archive page scan and its terminal read decision")]
pub(super) fn page(
    domain: &mut Domain,
    env: &Env<super::Limits>,
    waiter: Token,
    rows: Box<[Record]>,
    next: Option<Key>,
    out: &mut Queue<Request>,
) {
    let Some(Some(RootRead::Result(read))) = domain.result_reads.get_mut(Id::from_token(waiter)) else { return };
    for row in rows {
        let task = match row {
            Record::Tasks(tasks::Stored::Ended(task)) => task,
            Record::Tasks(
                tasks::Stored::PersonProposal(_)
                | tasks::Stored::History(_)
                | tasks::Stored::Live(_)
                | tasks::Stored::Ledger(_),
            )
            | Record::ProposalDecision(_)
            | Record::Call(_)
            | Record::EscalationDecision(_)
            | Record::Deployment(_)
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::People(_)
            | Record::Forge { .. } => unreachable!("ended-result range contains only ended tasks"),
        };
        if task.requester != tasks::Party::Person(read.person) {
            continue;
        }
        if task.result_position == 0 || task.result_position > domain.counters.deployment().messages {
            return failed(domain, waiter, people::Refusal::Limit, out);
        }
        if task.result_position <= read.position {
            continue;
        }
        domain.people.remember_result(
            &env.limits.people,
            read.person,
            people::ResultRef { task: task.number, position: task.result_position },
        );
        read.earliest = read.earliest.min(task.result_position);
        let keep = match &read.query {
            Query::Named { task: wanted } => *wanted == task.number,
            Query::Inbox { .. } => {
                if read.entries.room() > 0 {
                    true
                } else {
                    let mut latest = 0;
                    for entry in &read.entries {
                        latest = latest.max(entry.position);
                    }
                    task.result_position < latest
                }
            }
        };
        if !keep {
            continue;
        }
        let ending = match task.phase {
            tasks::Phase::Ended(ending) => ending,
            tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Closing(_) | tasks::Phase::Held { .. } => {
                unreachable!("ended row has ended phase")
            }
        };
        let entry = ResultEntry { task: task.number, position: task.result_position, words: ending_words(ending) };
        if entry.words.len() > usize::try_from(env.limits.journal.result_bytes).expect("u32 fits usize") {
            return failed(domain, waiter, people::Refusal::Limit, out);
        }
        match &read.query {
            Query::Named { .. } => read.target = Some(entry),
            Query::Inbox { .. } => {
                if read.entries.room() > 0 {
                    read.entries.push(entry).expect("page cap checked");
                } else {
                    let mut latest = 0;
                    let mut at = 0;
                    for index in 0..read.entries.len() {
                        let old = read.entries.get(index).expect("bounded entry index");
                        if old.position > latest {
                            latest = old.position;
                            at = index;
                        }
                    }
                    *read.entries.get_mut(at).expect("nonempty full page") = entry;
                }
            }
        }
    }
    if let Some(after) = next {
        request_load(domain, waiter, Range::EndedResults, Some(after), out);
        return;
    }
    let Some(RootRead::Result(read)) = take_read(domain, waiter) else { unreachable!("complete result read") };
    let removed = domain.reading_results.remove(&read.person);
    assert!(removed == Some(waiter), "result reader index names its waiter");
    domain.result_reads.retire(Id::from_token(waiter));
    let mut decision = Decision::new(&env.limits.journal);
    match read.query {
        Query::Named { .. } => {
            let Some(entry) = read.target else {
                return refuse(read.to, people::Refusal::Unknown, out);
            };
            if entry.position != read.earliest {
                return refuse(read.to, people::Refusal::Busy, out);
            }
            let row = domain.people.advance_read_position(read.person, entry.position).expect("new earliest result");
            save(&mut decision, &env.limits, Write::Save(Record::People(row)));
            emit(
                &mut decision,
                &env.limits,
                Delivery::ResultReply {
                    to: read.to,
                    person: read.person,
                    task: entry.task,
                    position: entry.position,
                    words: entry.words,
                },
            );
        }
        Query::Inbox { .. } => {
            let mut entries = read.entries.into_boxed();
            entries.sort_unstable();
            if let Some(last) = entries.last() {
                let row = domain.people.advance_read_position(read.person, last.position).expect("new page position");
                save(&mut decision, &env.limits, Write::Save(Record::People(row)));
            }
            emit(&mut decision, &env.limits, Delivery::InboxPage { to: read.to, person: read.person, entries });
        }
    }
    close(domain, env, decision, out);
}
