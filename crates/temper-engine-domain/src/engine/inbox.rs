//! Volatile person inbox projections from authoritative task rows.
//! Tasks retain the waiting state; people caches bounded references and the
//! root rebuilds the cache from restored rows (domain/people.md, section 6).

use super::{
    Decision, Delivery, Domain, Env, Id, Key, Range, Read as RootRead, Record, ReplyTo, Request, Token, admits, close,
    emit, ending_words, people, request_load, save, take_read, tasks,
};
use crate::Write;
use alloc::boxed::Box;
use skein_lib::{List, Queue};

/// Project one durable person-origin goal proposal into policy or proposer inboxes.
pub(super) fn person_proposal_entries(domain: &Domain, row: &tasks::PersonProposal) -> Box<[people::Entry]> {
    domain.core.person_proposal_entries(row)
}

#[derive(Clone, Copy, Debug)]
enum Stage {
    Live,
    Ended,
}

/// One bounded store scan for a person's newest-first whole inbox page.
#[derive(Debug)]
pub(super) struct Read {
    to: ReplyTo,
    person: u64,
    before: Option<crate::InboxCursor>,
    position: u64,
    high: u64,
    frozen_high: bool,
    stage: Stage,
    entries: List<crate::InboxViewEntry>,
    more: bool,
}

fn refused(to: ReplyTo, why: people::Refusal, out: &mut Queue<Request>) {
    out.push(Request::Deliver(Delivery::WebReply { to, sign_in: None, reply: people::Reply::Refused(why) }));
}

pub(super) fn begin(
    domain: &mut Domain,
    env: &Env<super::Limits>,
    to: ReplyTo,
    sign_in: u64,
    most: u32,
    before: Option<crate::InboxCursor>,
    out: &mut Queue<Request>,
) {
    if most == 0
        || env.limits.people.inbox_entries == 0
        || match before {
            Some(cursor) => cursor.read_high > domain.core.counters.deployment().messages,
            None => false,
        }
    {
        return refused(to, people::Refusal::Limit, out);
    }
    let Some(person) = domain.core.people.person(sign_in, env.now, env.wall) else {
        return refused(to, people::Refusal::SignIn, out);
    };
    if !domain.ready() || !admits(domain, &env.limits) || domain.core.reading_results.contains_key(&person) {
        return refused(to, people::Refusal::Busy, out);
    }
    let read = Read {
        to,
        person,
        before,
        position: domain.core.people.read_position(person).expect("authenticated person restored"),
        high: match before {
            Some(cursor) => cursor.read_high,
            None => 0,
        },
        frozen_high: before.is_some(),
        stage: Stage::Live,
        entries: List::with_capacity(most.min(env.limits.people.inbox_entries)),
        more: false,
    };
    let id = match domain.result_reads.insert(Some(RootRead::Inbox(read))) {
        Ok(id) => id,
        Err(Some(RootRead::Inbox(read))) => return refused(read.to, people::Refusal::Busy, out),
        Err(_) => unreachable!("inserted inbox read"),
    };
    let indexed = domain.core.reading_results.insert(person, id.token());
    assert!(indexed == Ok(None), "one inbox read per person");
    let mut decision = Decision::new(&env.limits.journal);
    emit(&mut decision, &env.limits, Delivery::BeginInboxView { waiter: id.token() });
    close(domain, env, decision, out);
}

pub(super) fn failed(domain: &mut Domain, waiter: Token, why: people::Refusal, out: &mut Queue<Request>) {
    let read = match take_read(domain, waiter) {
        Some(RootRead::Inbox(read)) => read,
        Some(
            RootRead::Result(_)
            | RootRead::Escalation(_)
            | RootRead::Proposal(_)
            | RootRead::Transcript { .. }
            | RootRead::Dependency(_)
            | RootRead::InputCheck(_),
        ) => {
            unreachable!("inbox load owns its read")
        }
        None => return,
    };
    let removed = domain.core.reading_results.remove(&read.person);
    assert!(removed == Some(waiter), "inbox reader index names its waiter");
    domain.result_reads.retire(Id::from_token(waiter));
    refused(read.to, why, out);
}

fn visible(domain: &Domain, person: u64, entry: people::Entry) -> bool {
    match entry.whom {
        people::Whom::Person(named) => named == person,
        people::Whom::Role { project, role } => match domain.core.people.role(person, project) {
            Some(holding) => super::escalation::role_number(holding) == role,
            None => false,
        },
    }
}

fn keep(read: &mut Read, entry: crate::InboxViewEntry) {
    let cursor = entry.cursor();
    if let Some(before) = read.before
        && (cursor.at, cursor.task, cursor.kind, cursor.number) >= (before.at, before.task, before.kind, before.number)
    {
        return;
    }
    let mut next = List::with_capacity(read.entries.capacity());
    let mut inserted = false;
    for old in &read.entries {
        let old_cursor = old.cursor();
        if (old_cursor.at, old_cursor.task, old_cursor.kind, old_cursor.number)
            == (cursor.at, cursor.task, cursor.kind, cursor.number)
        {
            return;
        }
        if !inserted
            && (cursor.at, cursor.task, cursor.kind, cursor.number)
                > (old_cursor.at, old_cursor.task, old_cursor.kind, old_cursor.number)
        {
            if next.room() > 0 {
                next.push(entry.clone()).expect("page room before older entry");
            }
            inserted = true;
        }
        if next.room() > 0 {
            next.push(old.clone()).expect("page room for old entry");
        } else {
            read.more = true;
        }
    }
    if !inserted {
        if next.room() > 0 {
            next.push(entry).expect("page room after old entries");
        } else {
            read.more = true;
        }
    }
    read.entries = next;
}

/// Consume one validated store page; request the next range or deliver one page.
#[expect(clippy::too_many_lines, reason = "one bounded two-stage store scan preserves its read slot")]
pub(super) fn page(
    domain: &mut Domain,
    env: &Env<super::Limits>,
    waiter: Token,
    rows: Box<[Record]>,
    next: Option<Key>,
    out: &mut Queue<Request>,
) {
    let mut read = match take_read(domain, waiter) {
        Some(RootRead::Inbox(read)) => read,
        Some(
            RootRead::Result(_)
            | RootRead::Escalation(_)
            | RootRead::Proposal(_)
            | RootRead::Transcript { .. }
            | RootRead::Dependency(_)
            | RootRead::InputCheck(_),
        ) => {
            unreachable!("inbox page owns its read")
        }
        None => return,
    };
    for row in rows {
        match read.stage {
            Stage::Live => match row {
                Record::Tasks(tasks::Stored::PersonProposal(row)) => {
                    for entry in person_proposal_entries(domain, &row) {
                        if !visible(domain, read.person, entry) {
                            continue;
                        }
                        match entry.kind {
                            people::EntryKind::Reply { message } => {
                                if message == 0 || message > domain.core.counters.deployment().messages {
                                    return failed_owned(domain, waiter, read, people::Refusal::Limit, out);
                                }
                                if message > read.position && (!read.frozen_high || message <= read.high) {
                                    if !read.frozen_high {
                                        read.high = read.high.max(message);
                                    }
                                    keep(&mut read, crate::InboxViewEntry::Waiting(entry));
                                }
                            }
                            people::EntryKind::Proposal { .. } => {
                                keep(&mut read, crate::InboxViewEntry::Waiting(entry));
                            }
                            people::EntryKind::Question { .. }
                            | people::EntryKind::Escalation { .. }
                            | people::EntryKind::PersonTask => unreachable!("person proposal entry kinds"),
                        }
                    }
                }
                Record::Tasks(tasks::Stored::Live(task)) => {
                    for entry in entries(domain, &task) {
                        if visible(domain, read.person, entry) {
                            match entry.kind {
                                people::EntryKind::Reply { message } => {
                                    if message == 0 || message > domain.core.counters.deployment().messages {
                                        return failed_owned(domain, waiter, read, people::Refusal::Limit, out);
                                    }
                                    if message > read.position && (!read.frozen_high || message <= read.high) {
                                        if !read.frozen_high {
                                            read.high = read.high.max(message);
                                        }
                                        keep(&mut read, crate::InboxViewEntry::Waiting(entry));
                                    }
                                }
                                people::EntryKind::Question { .. }
                                | people::EntryKind::Proposal { .. }
                                | people::EntryKind::Escalation { .. }
                                | people::EntryKind::PersonTask => {
                                    keep(&mut read, crate::InboxViewEntry::Waiting(entry));
                                }
                            }
                        }
                    }
                }
                Record::Tasks(
                    tasks::Stored::Ledger(_)
                    | tasks::Stored::Writer(_)
                    | tasks::Stored::Pool(_)
                    | tasks::Stored::Stub(_),
                ) => {}
                Record::Tasks(tasks::Stored::Ended(_) | tasks::Stored::History(_))
                | Record::People(_)
                | Record::Forge { .. }
                | Record::Deployment(_)
                | Record::Call(_)
                | Record::Turn(_)
                | Record::RunProof(_)
                | Record::Terminal(_)
                | Record::EscalationDecision(_)
                | Record::ProposalDecision(_) => {
                    unreachable!("live task range validates record family")
                }
            },
            Stage::Ended => match row {
                Record::Tasks(tasks::Stored::PersonProposal(_)) => unreachable!("ended result range has no proposal"),
                Record::Tasks(tasks::Stored::Ended(task)) => {
                    if task.requester == tasks::Party::Person(read.person)
                        && task.result_position > read.position
                        && (!read.frozen_high || task.result_position <= read.high)
                    {
                        if task.result_position > domain.core.counters.deployment().messages {
                            return failed_owned(domain, waiter, read, people::Refusal::Limit, out);
                        }
                        if !read.frozen_high {
                            read.high = read.high.max(task.result_position);
                        }
                        let ending = match task.phase {
                            tasks::Phase::Ended(ending) => ending,
                            tasks::Phase::Waiting
                            | tasks::Phase::Active(_)
                            | tasks::Phase::Closing(_)
                            | tasks::Phase::Held { .. } => {
                                unreachable!("ended range row")
                            }
                        };
                        let words = ending_words(ending);
                        if words.len() > usize::try_from(env.limits.journal.result_bytes).expect("u32 fits usize") {
                            return failed_owned(domain, waiter, read, people::Refusal::Limit, out);
                        }
                        let result = crate::ResultEntry { task: task.number, position: task.result_position, words };
                        keep(
                            &mut read,
                            crate::InboxViewEntry::Result { at: task.ended_at.unwrap_or(task.created_at), result },
                        );
                    }
                }
                Record::Tasks(
                    tasks::Stored::Live(_)
                    | tasks::Stored::Ledger(_)
                    | tasks::Stored::History(_)
                    | tasks::Stored::Stub(_)
                    | tasks::Stored::Writer(_)
                    | tasks::Stored::Pool(_),
                )
                | Record::People(_)
                | Record::Forge { .. }
                | Record::Deployment(_)
                | Record::Call(_)
                | Record::Turn(_)
                | Record::RunProof(_)
                | Record::Terminal(_)
                | Record::EscalationDecision(_)
                | Record::ProposalDecision(_) => {
                    unreachable!("ended result range validates record family")
                }
            },
        }
    }
    if let Some(after) = next {
        let range = match read.stage {
            Stage::Live => Range::Tasks,
            Stage::Ended => Range::EndedResults,
        };
        *domain.result_reads.get_mut(Id::from_token(waiter)).expect("inbox slot") = Some(RootRead::Inbox(read));
        request_load(domain, waiter, range, Some(after), out);
        return;
    }
    match read.stage {
        Stage::Live => {
            read.stage = Stage::Ended;
            *domain.result_reads.get_mut(Id::from_token(waiter)).expect("inbox slot") = Some(RootRead::Inbox(read));
            request_load(domain, waiter, Range::EndedResults, None, out);
        }
        Stage::Ended => {
            let removed = domain.core.reading_results.remove(&read.person);
            assert!(removed == Some(waiter), "inbox reader index names its waiter");
            domain.result_reads.retire(Id::from_token(waiter));
            let next = if read.more {
                match read.entries.last() {
                    Some(entry) => {
                        let mut cursor = entry.cursor();
                        cursor.read_high = read.high;
                        Some(cursor)
                    }
                    None => None,
                }
            } else {
                None
            };
            let mut decision = Decision::new(&env.limits.journal);
            if next.is_none() && read.high > read.position {
                let row = domain
                    .core
                    .people
                    .advance_read_position(read.person, read.high)
                    .expect("newest read position advances after the last page");
                save(&mut decision, &env.limits, Write::Save(Record::People(row)));
            }
            emit(
                &mut decision,
                &env.limits,
                Delivery::InboxView { to: read.to, person: read.person, entries: read.entries.into_boxed(), next },
            );
            close(domain, env, decision, out);
        }
    }
}

fn failed_owned(domain: &mut Domain, waiter: Token, read: Read, why: people::Refusal, out: &mut Queue<Request>) {
    let removed = domain.core.reading_results.remove(&read.person);
    assert!(removed == Some(waiter), "inbox reader index names its waiter");
    domain.result_reads.retire(Id::from_token(waiter));
    refused(read.to, why, out);
}

pub(super) fn entries(domain: &Domain, row: &tasks::TaskRecord) -> Box<[people::Entry]> {
    domain.core.waiting_entries(&super::core_limits(&domain.limits), row)
}
