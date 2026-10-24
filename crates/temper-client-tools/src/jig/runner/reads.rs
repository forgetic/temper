//! One call's bounded read set, with watches closed at their first delivery.
use super::{Called, Out, Runner, finish};
use crate::jig::{
    Limits,
    call::{Call, Project, Value, cursor, number, value},
    render,
};
use alloc::collections::BTreeMap;
use jig_client::{
    Client, Event, Handle, Latest, Readable, Readout, Watchable, Why, contract as c,
    thread::{self, Thread},
};
use skein_lib::{Env, Queue};
#[derive(Debug)]
pub(crate) struct Reading {
    pub called: Called,
    pub pending: u8,
    pub pages: BTreeMap<u32, Readout>,
    pub documents: BTreeMap<u32, c::Delivery>,
    pub text: Option<c::RunText>,
    pub thread: Option<Thread>,
}
fn handle(runner: &Runner, prefix: u32, local: u32) -> Handle {
    Handle { asker: runner.asker, number: (u64::from(prefix) << 32u32) | u64::from(local) }
}
fn read(runner: &Runner, reading: &mut Reading, local: u32, what: c::Read, out: &mut Queue<Out>) {
    reading.pending |= 1u8.checked_shl(local).expect("fixed read slot");
    out.push(Out::Client {
        event: Event::Read { handle: handle(runner, reading.called.call_number, local), read: Readable::Jig(what) },
    });
}
fn watch(runner: &Runner, reading: &mut Reading, local: u32, subject: c::Subject, out: &mut Queue<Out>) {
    reading.pending |= 1u8.checked_shl(local).expect("fixed watch slot");
    out.push(Out::Client {
        event: Event::Watch {
            handle: handle(runner, reading.called.call_number, local),
            subject: Watchable::Jig(subject),
        },
    });
}
pub(super) fn start(runner: &mut Runner, client: &Client, env: &Env<Limits>, called: Called, out: &mut Queue<Out>) {
    let mut reading =
        Reading { called, pending: 0, pages: BTreeMap::new(), documents: BTreeMap::new(), text: None, thread: None };
    let task = number(&reading.called.call, 0).unwrap_or(0);
    match reading.called.call.tool {
        1 => {
            if !inbox(runner, client, &mut reading, out) {
                let printed = render::failure(b"unknown project", reading.called.json, 1);
                finish(reading.called, printed, out);
                return;
            }
        }
        2 => {
            let Some(document) = client.document(runner.me()) else { return };
            let Some(Latest::Jig(c::Delivery::Me(me))) = document.latest else { return };
            let Some(_) = project(&reading.called.call, me) else {
                let printed = render::failure(b"unknown project", reading.called.json, 1);
                finish(reading.called, printed, out);
                return;
            };
            let call = &reading.called.call;
            let ended = value(call, 4).is_some();
            let query = task_query(call, me).expect("resolved project");
            let after = cursor(call, 6);
            let (live, ended) = if ended { (None, after) } else { (after, None) };
            read(runner, &mut reading, 1, c::Read::Tasks { query, live, ended }, out);
        }
        3 => {
            read(runner, &mut reading, 1, c::Read::Task { task }, out);
            if value(&reading.called.call, 1).is_some() {
                watch(runner, &mut reading, 3, c::Subject::Tree { task, window: env.limits.rows }, out);
            }
            if number(&reading.called.call, 2).is_some() {
                read(runner, &mut reading, 2, c::Read::Activity { task, before: None }, out);
            }
            if value(&reading.called.call, 3).is_some() {
                watch(runner, &mut reading, 4, c::Subject::Run { task, turn: None, offset: 0 }, out);
            }
        }
        4 => {
            let mut thread = Thread::new(
                task,
                &thread::Limits { lines: env.limits.rows, bytes: env.limits.ask_bytes, text: env.limits.text },
            );
            let next = match cursor(&reading.called.call, 1) {
                Some(cursor) => {
                    thread.lines.edges.after = thread::window::Edge::More(cursor);
                    thread.older()
                }
                None => thread.moved(1),
            };
            if let Some(next) = next {
                read(runner, &mut reading, 1, next, out);
            }
            reading.thread = Some(thread);
            watch(runner, &mut reading, 4, c::Subject::Run { task, turn: None, offset: 0 }, out);
        }
        5 => {
            let before = cursor(&reading.called.call, 1);
            read(runner, &mut reading, 1, c::Read::Activity { task, before }, out);
        }
        6 => {
            let Some(Value::Turn(name)) = value(&reading.called.call, 1) else { return };
            let what = c::Read::Turn { task, attempt: name.attempt, turn: name.number };
            read(runner, &mut reading, 1, what, out);
        }
        22 | 23 | 25 | 27..=30 | 33 | crate::jig::rows::EXPORT_AT => {
            let Some(document) = client.document(runner.me()) else { return };
            let Some(Latest::Jig(c::Delivery::Me(me))) = document.latest else { return };
            if reading.called.call.tool == 23 && model_project(&reading.called.call, me).is_err() {
                let printed = render::failure(b"unknown project", reading.called.json, 1);
                finish(reading.called, printed, out);
                return;
            }
            watch(runner, &mut reading, 3, c::Subject::Accounts, out);
            if me.deployment_role == Some(c::HeldRole::Owner) {
                watch(runner, &mut reading, 4, c::Subject::Deployment, out);
            }
        }
        7 => watch(runner, &mut reading, 4, c::Subject::Run { task, turn: None, offset: 0 }, out),
        _ => unreachable!("read tool"),
    }
    runner.active.insert(reading.called.call_number, reading);
}
pub(crate) fn project(call: &Call, me: &c::Me) -> Option<u32> {
    match value(call, 0) {
        Some(Value::Project(Project::Number(number))) => Some(*number),
        Some(Value::Project(Project::Name(name))) => {
            for project in &me.projects {
                if project.name.bytes() == name.as_ref() {
                    return Some(project.project);
                }
            }
            None
        }
        _ => None,
    }
}
pub(super) fn page(
    runner: &mut Runner,
    client: &Client,
    env: &Env<Limits>,
    handle: Handle,
    page: Readout,
    out: &mut Queue<Out>,
) {
    let prefix = u32::try_from(handle.number >> 32u32).expect("high half");
    let local = u32::try_from(handle.number & u64::from(u32::MAX)).expect("low half");
    let Some(mut reading) = runner.active.remove(&prefix) else { return };
    reading.pending &= !1u8.checked_shl(local).expect("fixed read slot");
    if let Some(thread) = &mut reading.thread
        && let Readout::Jig(c::ReadResult::Value(c::ReadValue::Activity(page))) = &page
        && let Some(next) = thread.accept(*page.clone())
    {
        read(runner, &mut reading, local, next, out);
    }
    let more_inbox = match &page {
        Readout::Jig(c::ReadResult::Value(c::ReadValue::Inbox(page))) => page.after.is_some(),
        Readout::Jig(c::ReadResult::Value(c::ReadValue::OwnerInbox(page))) => page.after.is_some(),
        Readout::Limit | Readout::NotServed { .. } | Readout::Jig(_) | Readout::Family { .. } => true,
    };
    reading.pages.insert(local, page);
    if reading.called.call.tool == 1 && local == 1 && !more_inbox && value(&reading.called.call, 1).is_none() {
        let call = &reading.called.call;
        let Some(document) = client.document(runner.me()) else { return };
        let Some(Latest::Jig(c::Delivery::Me(me))) = document.latest else { return };
        let what = c::Read::Inbox {
            group: c::InboxReadGroup::Updates,
            project: project(call, me),
            kind: None,
            goal: number(call, 2),
            after: None,
        };
        read(runner, &mut reading, 5, what, out);
    }
    complete(runner, client, env, reading, out);
}
pub(super) fn changed(runner: &mut Runner, client: &Client, env: &Env<Limits>, handle: Handle, out: &mut Queue<Out>) {
    let prefix = u32::try_from(handle.number >> 32u32).expect("high half");
    let local = u32::try_from(handle.number & u64::from(u32::MAX)).expect("low half");
    let Some(mut reading) = runner.active.remove(&prefix) else { return };
    let bit = 1u8.checked_shl(local).expect("fixed watch slot");
    if reading.pending & bit != 0
        && let Some(document) = client.document(handle)
        && (document.state == jig_client::State::Live || document.state == jig_client::State::Ended)
        && let Some(Latest::Jig(delivery)) = document.latest
    {
        reading.pending &= !bit;
        reading.documents.insert(local, delivery.clone());
        if local == 4 {
            reading.text = client.text(handle).cloned();
            if let Some(c::Delivery::RunState(run)) = reading.documents.get_mut(&local)
                && let Some(text) = &reading.text
            {
                run.writing = Some(text.clone());
            }
        }
        out.push(Out::Client { event: Event::Unwatch { handle } });
    }
    complete(runner, client, env, reading, out);
}
pub(super) fn ended(runner: &mut Runner, handle: Handle, why: Why, out: &mut Queue<Out>) {
    let prefix = u32::try_from(handle.number >> 32u32).expect("high half");
    let local = u32::try_from(handle.number & u64::from(u32::MAX)).expect("low half");
    let Some(reading) = runner.active.remove(&prefix) else { return };
    // A first delivery already filled this slot; its closure cannot cancel
    // the other documents still needed by the call.
    if reading.documents.contains_key(&local) {
        runner.active.insert(prefix, reading);
        return;
    }
    let status = match why {
        Why::Unknown | Why::NotAllowed | Why::SubjectEnded => 2,
        Why::NotServed | Why::SignedOut | Why::Limit => 1,
    };
    close(runner, &reading, out);
    let printed = render::failure(b"watch ended", reading.called.json, status);
    finish(reading.called, printed, out);
}
fn close(runner: &Runner, reading: &Reading, out: &mut Queue<Out>) {
    for local in [3, 4] {
        if reading.pending & 1u8.checked_shl(local).expect("fixed watch slot") != 0 {
            out.push(Out::Client {
                event: Event::Unwatch { handle: handle(runner, reading.called.call_number, local) },
            });
        }
    }
}
fn complete(runner: &mut Runner, client: &Client, env: &Env<Limits>, mut reading: Reading, out: &mut Queue<Out>) {
    if reading.pending != 0 {
        runner.active.insert(reading.called.call_number, reading);
        return;
    }
    if let Some(thread) = &mut reading.thread
        && let Some(c::Delivery::RunState(run)) = reading.documents.get(&4)
    {
        thread.run(run, reading.text.as_ref());
    }
    if reading.called.call.tool >= 24 {
        super::accounts::resolved(runner, client, env, reading, out);
        return;
    }
    let printed = render::reads(&reading, client, runner.me(), &env.limits, runner.parts.expect("host vocabulary"));
    finish(reading.called, printed, out);
}

fn inbox(runner: &Runner, client: &Client, reading: &mut Reading, out: &mut Queue<Out>) -> bool {
    let call = &reading.called.call;
    let kind = match value(call, 1) {
        Some(Value::Group(kind)) => Some(*kind),
        _ => None,
    };
    let project = match client.document(runner.me()) {
        Some(document) => match document.latest {
            Some(Latest::Jig(c::Delivery::Me(me))) => project(call, me),
            _ => None,
        },
        None => None,
    };
    if value(call, 0).is_some() && project.is_none() {
        return false;
    }
    let goal = number(call, 2);
    let after = match value(call, 3) {
        Some(Value::Cursor(bytes)) => Some(bytes.as_ref()),
        _ => None,
    };
    let tag = match after {
        Some(bytes) => bytes.first().copied(),
        None => None,
    };
    let group = match tag {
        Some(1) => c::InboxReadGroup::Updates,
        Some(0) => c::InboxReadGroup::Needs,
        None => match kind {
            Some(kind) => c::InboxReadGroup::of(kind),
            None => c::InboxReadGroup::Needs,
        },
        Some(_) => unreachable!("parsed inbox group"),
    };
    let cursor = match after {
        Some(bytes) => match bytes.get(1..) {
            Some(bytes) if !bytes.is_empty() => Some(c::Cursor::new(bytes.into(), u32::MAX).expect("parsed cursor")),
            Some(_) | None => None,
        },
        None => None,
    };
    let what = c::Read::Inbox { group, project, kind, goal, after: cursor };
    read(runner, reading, if group == c::InboxReadGroup::Needs { 1 } else { 5 }, what, out);
    true
}

pub(crate) fn task_query(call: &Call, me: &c::Me) -> Option<c::TaskQuery> {
    let project = project(call, me)?;
    let ended = value(call, 4).is_some();
    Some(c::TaskQuery {
        project: Some(project),
        requester: if value(call, 3).is_some() { Some(c::OwnerRequester::Person(me.party.number)) } else { None },
        root: None,
        parent: number(call, 7),
        label: match value(call, 1) {
            Some(Value::Word(0)) => Some(jig_client::shapes::CHAT),
            Some(Value::Word(1)) => Some(jig_client::shapes::GOAL),
            _ => None,
        },
        phase: if ended { Some(c::PhaseClass::Ended) } else { None },
        executor: None,
        tracked: if value(call, 2).is_some() { Some(true) } else { None },
        order: match value(call, 5) {
            Some(Value::Order(order)) => *order,
            _ => c::QueryOrder::Number,
        },
    })
}

// Numeric project arguments require membership too; omission includes every
// project in the supplied me document, never an engine-wide catalogue.
pub(crate) fn model_project(call: &Call, me: &c::Me) -> Result<Option<u32>, ()> {
    if value(call, 0).is_none() {
        return Ok(None);
    }
    let selected = project(call, me).ok_or(())?;
    for held in &me.projects {
        if held.project == selected {
            return Ok(Some(selected));
        }
    }
    Err(())
}
