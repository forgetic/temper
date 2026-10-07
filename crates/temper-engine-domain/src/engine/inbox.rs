//! Volatile person inbox projections from authoritative task rows.
//! Tasks retain the waiting state; people caches bounded references and the
//! root rebuilds the cache from restored rows (domain/people.md, section 6).

use super::{
    Decision, Delivery, Domain, Env, Id, Key, Range, Read as RootRead, Record, ReplyTo, Request, Token, admits,
    authority, close, emit, ending_words, people, request_load, save, take_read, tasks,
};
use crate::Write;
use alloc::boxed::Box;
use skein_lib::{List, Queue};

/// Project one durable person-origin goal proposal into policy or proposer inboxes.
pub(super) fn person_proposal_entries(domain: &Domain, row: &tasks::PersonProposal) -> Box<[people::Entry]> {
    let mut entries = List::with_capacity(4);
    match &row.state {
        tasks::PersonProposalState::Pending { since } => {
            for role in 0..4 {
                if let Some(policy) = domain.config.authority.role(row.project, role)
                    && policy.decides.allows(authority::ProposalKind::Batch)
                {
                    entries
                        .push(people::Entry {
                            task: row.goal.number,
                            project: row.project,
                            whom: people::Whom::Role { project: row.project, role },
                            kind: people::EntryKind::Proposal { number: row.number },
                            at: *since,
                        })
                        .expect("one entry per policy role");
                }
            }
        }
        tasks::PersonProposalState::Rejected { message, at, .. } => {
            entries
                .push(people::Entry {
                    task: row.goal.number,
                    project: row.project,
                    whom: people::Whom::Person(row.proposer),
                    kind: people::EntryKind::Reply { message: *message },
                    at: *at,
                })
                .expect("one rejection reply");
        }
        tasks::PersonProposalState::Accepted { .. } => {}
    }
    entries.into_boxed()
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
            Some(cursor) => cursor.read_high > domain.journal.deployment().messages,
            None => false,
        }
    {
        return refused(to, people::Refusal::Limit, out);
    }
    let Some(person) = domain.people.person(sign_in, env.now, env.wall) else {
        return refused(to, people::Refusal::SignIn, out);
    };
    if !domain.ready() || !admits(domain, &env.limits) || domain.reading_results.contains_key(&person) {
        return refused(to, people::Refusal::Busy, out);
    }
    let read = Read {
        to,
        person,
        before,
        position: domain.people.read_position(person).expect("authenticated person restored"),
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
    let indexed = domain.reading_results.insert(person, id.token());
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
    let removed = domain.reading_results.remove(&read.person);
    assert!(removed == Some(waiter), "inbox reader index names its waiter");
    domain.result_reads.retire(Id::from_token(waiter));
    refused(read.to, why, out);
}

fn visible(domain: &Domain, person: u64, entry: people::Entry) -> bool {
    match entry.whom {
        people::Whom::Person(named) => named == person,
        people::Whom::Role { project, role } => match domain.people.role(person, project) {
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
                                if message == 0 || message > domain.journal.deployment().messages {
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
                                    if message == 0 || message > domain.journal.deployment().messages {
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
                Record::Tasks(tasks::Stored::Ledger(_)) => {}
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
                        if task.result_position > domain.journal.deployment().messages {
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
                Record::Tasks(tasks::Stored::Live(_) | tasks::Stored::Ledger(_) | tasks::Stored::History(_))
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
            let removed = domain.reading_results.remove(&read.person);
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
    let removed = domain.reading_results.remove(&read.person);
    assert!(removed == Some(waiter), "inbox reader index names its waiter");
    domain.result_reads.retire(Id::from_token(waiter));
    refused(read.to, why, out);
}

fn add(
    entries: &mut List<people::Entry>,
    row: &tasks::TaskRecord,
    whom: people::Whom,
    kind: people::EntryKind,
    at: skein_lib::Wall,
) {
    entries
        .push(people::Entry { task: row.number, project: row.project, whom, kind, at })
        .expect("one bounded projection per task state");
}

fn proposal_kind(kind: tasks::ProposalKind) -> authority::ProposalKind {
    match kind {
        tasks::ProposalKind::Batch => authority::ProposalKind::Batch,
        tasks::ProposalKind::Amend => authority::ProposalKind::Amend,
        tasks::ProposalKind::Widen => authority::ProposalKind::Widen,
        tasks::ProposalKind::Release => authority::ProposalKind::Escalation,
    }
}

fn question_answered(row: &tasks::TaskRecord, question: u64) -> bool {
    for word in &row.inbox {
        match word.kind {
            tasks::MessageKind::Answer { question: answered } if answered == question => return true,
            tasks::MessageKind::Answer { .. }
            | tasks::MessageKind::Question
            | tasks::MessageKind::Words
            | tasks::MessageKind::Amendment { .. }
            | tasks::MessageKind::Proposal { .. }
            | tasks::MessageKind::ProposalDecision { .. }
            | tasks::MessageKind::Escalation { .. }
            | tasks::MessageKind::Notice { .. }
            | tasks::MessageKind::Timer { .. }
            | tasks::MessageKind::News { .. }
            | tasks::MessageKind::Result(_) => {}
        }
    }
    false
}

/// Project the currently waiting person-facing references of one task.
#[expect(clippy::too_many_lines, reason = "one exhaustive task projection covers all person-facing waiting kinds")]
pub(super) fn entries(domain: &Domain, row: &tasks::TaskRecord) -> Box<[people::Entry]> {
    let capacity = domain.limits.tasks.inbox_messages.checked_add(7).expect("validated task inbox projection bound");
    let mut entries = List::with_capacity(capacity);
    match &row.phase {
        tasks::Phase::Ended(_) | tasks::Phase::Closing(_) => return entries.into_boxed(),
        tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Held { .. } => {}
    }
    match row.requester {
        tasks::Party::Person(person) => {
            for word in &row.inbox {
                match word.kind {
                    tasks::MessageKind::Question => {
                        if !question_answered(row, word.number) {
                            add(
                                &mut entries,
                                row,
                                people::Whom::Person(person),
                                people::EntryKind::Question { message: word.number },
                                word.at,
                            );
                        }
                    }
                    tasks::MessageKind::Answer { .. } => add(
                        &mut entries,
                        row,
                        people::Whom::Person(person),
                        people::EntryKind::Reply { message: word.number },
                        word.at,
                    ),
                    tasks::MessageKind::ProposalDecision { .. }
                    | tasks::MessageKind::Proposal { .. }
                    | tasks::MessageKind::Words
                    | tasks::MessageKind::Amendment { .. }
                    | tasks::MessageKind::Notice { .. }
                    | tasks::MessageKind::Timer { .. }
                    | tasks::MessageKind::News { .. }
                    | tasks::MessageKind::Result(_)
                    | tasks::MessageKind::Escalation { .. } => {}
                }
            }
        }
        tasks::Party::Task(_) | tasks::Party::Deployment { .. } => {}
    }
    if let Some(proposal) = &row.proposal {
        match proposal.state {
            tasks::ProposalState::Pending { holder, since } => match holder {
                tasks::ProposalHolder::Person(person) => add(
                    &mut entries,
                    row,
                    people::Whom::Person(person),
                    people::EntryKind::Proposal { number: proposal.number },
                    since,
                ),
                tasks::ProposalHolder::Policy { project, kind } => {
                    for role in 0..4 {
                        if let Some(policy) = domain.config.authority.role(project, role)
                            && policy.decides.allows(proposal_kind(kind))
                        {
                            add(
                                &mut entries,
                                row,
                                people::Whom::Role { project, role },
                                people::EntryKind::Proposal { number: proposal.number },
                                since,
                            );
                        }
                    }
                }
                tasks::ProposalHolder::Task(_) => {}
            },
            tasks::ProposalState::Accepted { .. }
            | tasks::ProposalState::Rejected { .. }
            | tasks::ProposalState::Withdrawn => {}
        }
    }
    match row.escalation {
        tasks::Escalation::Waiting { holder, revision, since, .. } => match holder {
            tasks::EscalationHolder::Person(person) => {
                add(&mut entries, row, people::Whom::Person(person), people::EntryKind::Escalation { revision }, since);
            }
            tasks::EscalationHolder::Role { project, role } => {
                add(
                    &mut entries,
                    row,
                    people::Whom::Role { project, role },
                    people::EntryKind::Escalation { revision },
                    since,
                );
            }
            tasks::EscalationHolder::Task(_) => {}
        },
        tasks::Escalation::Unheld { .. } | tasks::Escalation::Routing { .. } | tasks::Escalation::Rejected { .. } => {}
    }
    match row.executor {
        tasks::Executor::Person(tasks::PersonAddress::Person(person)) => {
            if row.taken_by.is_none() {
                add(&mut entries, row, people::Whom::Person(person), people::EntryKind::PersonTask, row.created_at);
            }
        }
        tasks::Executor::Person(tasks::PersonAddress::Role(role)) => {
            let whom = match row.taken_by {
                Some(person) => people::Whom::Person(person),
                None => people::Whom::Role { project: row.project, role },
            };
            add(&mut entries, row, whom, people::EntryKind::PersonTask, row.created_at);
        }
        tasks::Executor::Agent { .. } | tasks::Executor::Procedure { .. } => {}
    }
    entries.into_boxed()
}
