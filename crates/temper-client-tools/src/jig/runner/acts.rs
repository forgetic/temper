//! Offered acts are read once, then kept by the shared client's request table.
pub(crate) mod choice;
use super::{Called, Out, Runner, finish};
use crate::jig::{
    Limits,
    call::{Call, Value, Values, number, value},
    render::{self, outcome::Operation},
};
use alloc::boxed::Box;
use jig_client::{Client, Event, Handle, Key, Latest, Readable, Readout, contract as c, shapes};
use skein_lib::{Env, List, Queue};
#[derive(Debug)]
pub(super) struct Reading {
    called: Called,
    document: Option<c::ReadValue>,
}
#[derive(Debug)]
pub(super) struct Sent {
    pub(super) called: Called,
    pub(super) operation: Operation,
    pub(super) holder: Option<c::AccountHolder>,
    pub(super) dispatched: bool,
}
pub(crate) fn me<'a>(runner: &Runner, client: &'a Client) -> Option<&'a c::Me> {
    match client.document(runner.me())?.latest? {
        Latest::Jig(c::Delivery::Me(me)) => Some(me),
        Latest::Jig(_) | Latest::Family(_) => None,
    }
}
pub(super) fn start(runner: &mut Runner, client: &Client, env: &Env<Limits>, called: Called, out: &mut Queue<Out>) {
    if called.call.tool == 8 {
        let Some(me) = me(runner, client) else { return };
        let ask = chat(&called.call, me, &env.limits);
        prepared(runner, client, env, called, ask, out);
        return;
    }
    let call = &called.call;
    let task = number(call, 0).unwrap_or(0);
    let read = match call.tool {
        12 if call.form == 0 => {
            let Some(Value::Proposal(proposal)) = value(call, 0) else { unreachable!("proposal form") };
            c::Read::Proposal { proposal: *proposal }
        }
        12 => c::Read::Escalation { task },
        11 => c::Read::Question { task, message: number(call, 1).expect("question") },
        _ => c::Read::Task { task },
    };
    out.push(Out::Client {
        event: Event::Read {
            handle: Handle { asker: runner.asker, number: (u64::from(called.call_number) << 32u32) | 1 },
            read: Readable::Jig(read),
        },
    });
    runner.acts.insert(called.call_number, Reading { called, document: None });
}
pub(crate) enum Failed {
    Fixed(&'static [u8]),
    Unknown { what: &'static [u8], name: Box<[u8]> },
    EffortRequired { name: Box<[u8]> },
    Detail(Box<[u8]>),
}
pub(crate) type Prepared = Result<(Request, Option<c::Recipient>), Failed>;
fn chat(call: &Call, me: &c::Me, limits: &Limits) -> Prepared {
    let project = super::reads::project(call, me).ok_or(Failed::Fixed(b"unknown project"))?;
    let Ok(mut ask) = shapes::chat(me, project, words(call, 1, limits)?) else {
        return Err(Failed::Fixed(b"chat not offered"));
    };
    let c::Ask::Create { tasks, .. } = &mut ask else { unreachable!("chat creates") };
    let task = tasks.get_mut(0).expect("chat creates one task");
    task.llm = choice::llm(call, 2, 3, me, project, task.llm.as_ref())?;
    let mut creating = None;
    for membership in &me.projects {
        if membership.project == project {
            creating = membership.creating.as_ref();
        }
    }
    let creating = creating.ok_or(Failed::Fixed(b"chat not offered"))?;
    let draft = shapes::GoalDraft {
        words: task.spec.words.clone(),
        executor: task.executor,
        budget: task.authority.spend,
        deadline: task.authority.deadline,
        priority: task.tracked,
        llm: task.llm.clone(),
    };
    match shapes::fall(creating, &draft, task.deadline) {
        shapes::Fall::Within => Ok((Request::Ask(ask), None)),
        shapes::Fall::Proposable(to) => {
            let c::Ask::Create { tasks, .. } = ask else { unreachable!("chat creates") };
            Ok((
                Request::Ask(c::Ask::Propose {
                    project,
                    action: c::ProposeAction::Batch { tasks },
                    reason: empty(limits),
                }),
                Some(to),
            ))
        }
        shapes::Fall::Refused(_) | shapes::Fall::NotOffered(_) => Err(Failed::Fixed(b"chat exceeds offered bounds")),
    }
}
pub(crate) fn words(call: &Call, index: usize, limits: &Limits) -> Result<c::Text, Failed> {
    let bytes = match value(call, index) {
        Some(Value::Text(bytes)) => bytes.as_ref(),
        None => b"",
        _ => return Err(Failed::Fixed(b"text not loaded")),
    };
    match c::Text::new(Box::from(bytes), limits.text) {
        Ok(text) => Ok(text),
        Err(_) => Err(Failed::Fixed(b"text limit")),
    }
}
fn empty(limits: &Limits) -> c::Text {
    c::Text::new(Box::default(), limits.text).expect("empty words fit")
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
    let Some(mut reading) = runner.acts.remove(&prefix) else { return };
    if let Some(document) = reading.document.take() {
        let called = reading.called;
        let ask = match page {
            Readout::Jig(c::ReadResult::Value(c::ReadValue::Task(task))) => {
                let Some(me) = me(runner, client) else { return };
                build(&called.call, &document, me, &env.limits, Some(task.project))
            }
            Readout::Limit | Readout::NotServed { .. } | Readout::Jig(_) | Readout::Family { .. } => {
                Err(Failed::Fixed(b"task project unavailable"))
            }
        };
        prepared(runner, client, env, called, ask, out);
        return;
    }
    if let Readout::Jig(c::ReadResult::Value(document)) = &page {
        let chip = match document {
            c::ReadValue::Question(q) => Some(&q.task),
            c::ReadValue::Escalation(e) => Some(&e.task),
            c::ReadValue::Settings(_)
            | c::ReadValue::Setting(_)
            | c::ReadValue::Revision(_)
            | c::ReadValue::Revisions(_)
            | c::ReadValue::Tasks(_)
            | c::ReadValue::OwnerProject(_)
            | c::ReadValue::Historical(_)
            | c::ReadValue::Me(_)
            | c::ReadValue::Turn(_)
            | c::ReadValue::Task(_)
            | c::ReadValue::Proposal(_)
            | c::ReadValue::OwnerProposal(_)
            | c::ReadValue::OwnerEscalation(_)
            | c::ReadValue::PersonTask(_)
            | c::ReadValue::OwnerPersonTask(_)
            | c::ReadValue::Result(_)
            | c::ReadValue::Ended(_)
            | c::ReadValue::OwnerEnded(_)
            | c::ReadValue::History(_)
            | c::ReadValue::Activity(_)
            | c::ReadValue::Inbox(_)
            | c::ReadValue::OwnerInbox(_)
            | c::ReadValue::Spend(_)
            | c::ReadValue::OwnerSpend(_)
            | c::ReadValue::Project(_)
            | c::ReadValue::Notes(_)
            | c::ReadValue::Deployment(_)
            | c::ReadValue::OwnerDeployment(_) => None,
        };
        if let Some(chip) = chip
            && chip_project(chip).is_none()
        {
            out.push(Out::Client {
                event: Event::Read { handle, read: Readable::Jig(c::Read::Task { task: chip.task }) },
            });
            reading.document = Some(document.clone());
            runner.acts.insert(prefix, reading);
            return;
        }
    }
    let called = reading.called;
    let Some(me) = me(runner, client) else {
        let printed = render::failure(b"signed out", called.json, 1);
        finish(called, printed, out);
        return;
    };
    if called.call.tool == 31
        && let Readout::Jig(c::ReadResult::Value(c::ReadValue::Task(task))) = &page
    {
        out.push(Out::DraftAmendment { called: Box::new(called), task: task.clone() });
        return;
    }
    let ask = match &page {
        Readout::Jig(c::ReadResult::Value(document)) => build(&called.call, document, me, &env.limits, None),
        Readout::Limit | Readout::NotServed { .. } | Readout::Jig(_) | Readout::Family { .. } => {
            let printed = render::read(
                &page,
                client,
                runner.me(),
                called.json,
                &env.limits,
                runner.parts.expect("host vocabulary"),
            );
            finish(called, printed, out);
            return;
        }
    };
    prepared(runner, client, env, called, ask, out);
}
fn build(call: &Call, document: &c::ReadValue, me: &c::Me, limits: &Limits, project: Option<u32>) -> Prepared {
    if call.tool == 31 {
        return Err(Failed::Fixed(b"unexpected amendment document"));
    }
    let (project, offers) = match document {
        c::ReadValue::Task(task) => (task.project, &task.acts),
        c::ReadValue::Question(question) => {
            (chip_project(&question.task).or(project).ok_or(Failed::Fixed(b"project unavailable"))?, &question.acts)
        }
        c::ReadValue::Proposal(proposal) => (proposal.project, &proposal.acts),
        c::ReadValue::OwnerProposal(proposal) => (proposal.project, &proposal.acts),
        c::ReadValue::Escalation(escalation) => {
            (chip_project(&escalation.task).or(project).ok_or(Failed::Fixed(b"project unavailable"))?, &escalation.acts)
        }
        c::ReadValue::OwnerEscalation(escalation) => (escalation.task.project, &escalation.acts),
        c::ReadValue::Settings(_)
        | c::ReadValue::Setting(_)
        | c::ReadValue::Revision(_)
        | c::ReadValue::Revisions(_)
        | c::ReadValue::Tasks(_)
        | c::ReadValue::OwnerProject(_)
        | c::ReadValue::Historical(_)
        | c::ReadValue::Me(_)
        | c::ReadValue::Turn(_)
        | c::ReadValue::PersonTask(_)
        | c::ReadValue::OwnerPersonTask(_)
        | c::ReadValue::Result(_)
        | c::ReadValue::Ended(_)
        | c::ReadValue::OwnerEnded(_)
        | c::ReadValue::History(_)
        | c::ReadValue::Activity(_)
        | c::ReadValue::Inbox(_)
        | c::ReadValue::OwnerInbox(_)
        | c::ReadValue::Spend(_)
        | c::ReadValue::OwnerSpend(_)
        | c::ReadValue::Project(_)
        | c::ReadValue::Notes(_)
        | c::ReadValue::Deployment(_)
        | c::ReadValue::OwnerDeployment(_) => {
            return Err(Failed::Fixed(b"tools' bug; unexpected act document; report it as a finding"));
        }
    };
    let act = kind(call);
    let mut admission = None;
    for offer in offers {
        if offer.act == act {
            admission = Some(&offer.admission);
        }
    }
    let to = match admission {
        Some(c::Admission::Allowed) => None,
        Some(c::Admission::Proposed { to }) => Some(to.clone()),
        Some(c::Admission::Bounded { .. }) => return Err(Failed::Fixed(b"act has no bounded draft")),
        Some(c::Admission::Refused(_)) | None => return Err(Failed::Fixed(b"act not offered")),
    };
    let task = number(call, 0).unwrap_or(0);
    let ask = match call.tool {
        9 => c::Ask::Message { project, task, words: words(call, 1, limits)? },
        10 => close(call, document, me, project, limits)?,
        11 => c::Ask::Answer {
            project,
            on: c::AnswerOn::Question {
                task,
                message: number(call, 1).expect("question"),
                words: words(call, 2, limits)?,
            },
        },
        12 => {
            let on = if call.form == 0 {
                let Some(Value::Proposal(proposal)) = value(call, 0) else { unreachable!("proposal form") };
                c::DecideOn::Proposal { proposal: *proposal, decision: decision(call, limits)?, running: None }
            } else {
                c::DecideOn::Escalation {
                    task,
                    revision: c::Revision { value: number(call, 1).expect("hold") },
                    decision: decision(call, limits)?,
                }
            };
            c::Ask::Decide { project, on }
        }
        13 => c::Ask::Release {
            project,
            task,
            revision: c::Revision { value: number(call, 1).expect("hold") },
            again: if value(call, 2).is_some() { Some(true) } else { None },
        },
        14 => c::Ask::Stop { project, task },
        15 => c::Ask::Cancel {
            project,
            task,
            reason: words(call, 2, limits)?,
            grace: match value(call, 1) {
                Some(Value::Duration(duration)) => Some(*duration),
                _ => None,
            },
        },
        _ => unreachable!("act tool"),
    };
    Ok((Request::Ask(ask), to))
}
fn chip_project(chip: &c::Chip) -> Option<u32> {
    match chip.details.as_ref() {
        c::ChipPhase::Owner { details, .. } => Some(details.project),
        c::ChipPhase::Active(_)
        | c::ChipPhase::Waiting { .. }
        | c::ChipPhase::Idle { .. }
        | c::ChipPhase::Starting
        | c::ChipPhase::Running { .. }
        | c::ChipPhase::WaitingForWords { .. }
        | c::ChipPhase::BackingOff { .. }
        | c::ChipPhase::Procedure { .. }
        | c::ChipPhase::Person { .. }
        | c::ChipPhase::Closing(_)
        | c::ChipPhase::Held { .. }
        | c::ChipPhase::Ended { .. } => None,
    }
}
fn kind(call: &Call) -> c::ActKind {
    match call.tool {
        9 => c::ActKind::Message,
        10 | 15 => c::ActKind::Cancel,
        11 => c::ActKind::Answer,
        12 => match value(call, if call.form == 0 { 1 } else { 2 }) {
            Some(Value::Word(0)) => {
                if call.form == 0 {
                    c::ActKind::Accept
                } else {
                    c::ActKind::Release
                }
            }
            Some(Value::Word(1)) => {
                if call.form == 0 {
                    c::ActKind::Reject
                } else {
                    c::ActKind::LeaveHeld
                }
            }
            _ => c::ActKind::PassUp,
        },
        13 => c::ActKind::Release,
        14 => c::ActKind::Stop,
        _ => unreachable!("act tool"),
    }
}
fn decision(call: &Call, limits: &Limits) -> Result<c::Decision, Failed> {
    let index = if call.form == 0 { 1 } else { 2 };
    let reason = words(call, if call.form == 0 { 2 } else { 3 }, limits)?;
    Ok(match value(call, index) {
        Some(Value::Word(0)) => {
            if call.form == 0 {
                c::Decision::Accepted
            } else {
                c::Decision::Released { again: None }
            }
        }
        Some(Value::Word(1)) => {
            if call.form == 0 {
                c::Decision::Rejected { reason }
            } else {
                c::Decision::LeftHeld { reason }
            }
        }
        _ => c::Decision::PassedUp,
    })
}
pub(crate) enum Request {
    MoveOut { holder: c::AccountHolder, account: u32 },
    Written { holder: c::AccountHolder, account: u32, number: u32 },
    MoveIn,
    Ask(c::Ask),
    Account { ask: c::Ask, context: render::outcome::Account },
    AddKey { holder: c::AccountHolder, provider: u32, join: Option<u32> },
    ReplaceKey { holder: c::AccountHolder, source: u32 },
    DeclareProgram { holder: c::AccountHolder, provider: u32, join: Option<u32>, name: Box<[u8]> },
    CheckProgram { holder: c::AccountHolder, source: u32 },
}
pub(crate) fn prepared(
    runner: &mut Runner,
    client: &Client,
    _env: &Env<Limits>,
    called: Called,
    request: Prepared,
    out: &mut Queue<Out>,
) {
    let (request, to) = match request {
        Ok(request) => request,
        Err(why) => {
            let printed = match why {
                Failed::Fixed(why) => render::failure(why, called.json, 1),
                Failed::Detail(why) => render::failure(&why, called.json, 1),
                Failed::Unknown { what, name } => choice::named(b"unknown ", what, &name, called.json),
                Failed::EffortRequired { name } => choice::named(b"effort required", b"", &name, called.json),
            };
            finish(called, printed, out);
            return;
        }
    };
    send(runner, client, called, request, to, out);
}
fn send(
    runner: &mut Runner,
    client: &Client,
    mut called: Called,
    request: Request,
    to: Option<c::Recipient>,
    out: &mut Queue<Out>,
) {
    let key = Key { asker: runner.asker, number: (u64::from(called.call_number) << 32u32) | 1 };
    if client.pending(key).is_some() {
        let printed = render::failure(b"tools' bug; client key collision; report it as a finding", called.json, 1);
        finish(called, printed, out);
        return;
    }
    draw_name(runner, &mut called);
    let name = match &called.call.key {
        Some(name) => Some(crate::jig::key::named(runner.salt, name)),
        None => None,
    };
    let (request, account) = match request {
        Request::Account { ask, context } => (Request::Ask(ask), Some(context)),
        request @ (Request::MoveOut { .. }
        | Request::Written { .. }
        | Request::MoveIn
        | Request::Ask(_)
        | Request::AddKey { .. }
        | Request::ReplaceKey { .. }
        | Request::DeclareProgram { .. }
        | Request::CheckProgram { .. }) => (request, None),
    };
    let mut holder = None;
    let mut exported = None;
    let event = match request {
        Request::MoveOut { holder: h, account } => {
            holder = Some(h);
            jig_client::accounts::move_out(key, name.expect("move name"), h, account)
        }
        Request::Written { holder, account, number } => {
            exported = Some((account, number));
            jig_client::accounts::written(key, name.expect("written name"), holder, number)
        }
        Request::MoveIn => jig_client::accounts::move_in(
            key,
            name.expect("import name"),
            c::AccountHolder::Own,
            called.value.take().expect("record supplied"),
        ),
        Request::Account { .. } => unreachable!("account context extracted"),
        Request::DeclareProgram { holder, provider, join, name: program } => {
            jig_client::accounts::declare_program(key, name.expect("declaration name"), holder, provider, join, program)
        }
        Request::CheckProgram { holder, source } => {
            jig_client::accounts::check_program(key, name.expect("check name"), holder, source)
        }
        Request::Ask(ask) => Event::Send { key, name, request: jig_client::Ask::Jig(Box::new(ask)) },
        Request::AddKey { holder, provider, join } => jig_client::accounts::add_key(
            key,
            name.expect("key request name"),
            holder,
            provider,
            join,
            called.value.take().expect("admitted value"),
        ),
        Request::ReplaceKey { holder, source } => jig_client::accounts::replace_key(
            key,
            name.expect("key request name"),
            holder,
            source,
            called.value.take().expect("admitted value"),
        ),
    };
    let (Event::Send { request, .. } | Event::Carry { request, .. }) = &event else { unreachable!("act event") };
    let mut operation = Operation::of(request);
    operation.to = to;
    operation.exported = exported;
    if called.call.tool == crate::jig::rows::EXPORT_AT {
        let Some(Value::File(file)) = value(&called.call, 1) else { unreachable!("export file") };
        operation.file = Some(file.clone());
    }
    operation.account = account;
    if called.call.tool != crate::jig::rows::EXPORT_AT {
        operation.retry = Some(render::outcome::retry(&called.call));
    }
    if called.call.tool == 26 || called.call.tool == 27 || called.call.tool == crate::jig::rows::IMPORT_AT {
        operation.value_name.clone_from(&called.call.key);
    }
    if let Some(me) = me(runner, client) {
        operation.party = Some(me.party.number);
    }
    // The compound's public operation is close, rather than its final cancellation.
    if called.call.tool == 10 {
        operation.word = b"close";
    }
    if (24..=30).contains(&called.call.tool)
        || called.call.tool == 32
        || called.call.tool == 33
        || called.call.tool == crate::jig::rows::EXPORT_AT
        || called.call.tool == crate::jig::rows::IMPORT_AT
    {
        operation.word = render::outcome::account_tool(called.call.tool).name;
    }
    runner.sent.insert(key, Sent { called, operation, holder, dispatched: false });
    out.push(Out::Client { event });
}
pub(super) fn answered(
    runner: &mut Runner,
    client: &Client,
    env: &Env<Limits>,
    key: Key,
    outcome: &jig_client::Outcome,
    out: &mut Queue<Out>,
) {
    if let Some(sent) = runner.sent.remove(&key) {
        if sent.called.call.tool == crate::jig::rows::EXPORT_AT {
            super::moves::answered(runner, client, env, sent, outcome, out);
            return;
        }
        let mut printed =
            render::outcome::print(&sent.operation, outcome, sent.called.json, me(runner, client), &env.limits);
        if sent.called.call.tool == crate::jig::rows::IMPORT_AT
            && let jig_client::Outcome::Answered(jig_client::Answered::Jig(c::Answer::Done(c::Outcome::Owner(
                c::OwnerOutcome::AccountMovedIn { .. },
            )))) = outcome
        {
            printed.file = super::FileEnd::Remove;
        }
        finish(sent.called, printed, out);
    } else if let Some(operation) = runner.kept.remove(&key) {
        let line = render::outcome::kept(&operation, outcome, runner.kept_json, me(runner, client), &env.limits);
        out.push(Out::Kept { line });
    }
}

fn close(call: &Call, document: &c::ReadValue, me: &c::Me, project: u32, limits: &Limits) -> Result<c::Ask, Failed> {
    let c::ReadValue::Task(task) = document else { return Err(Failed::Fixed(b"unexpected close document")) };
    if let Some(Some(Values::Many(moves))) = call.values.get(1) {
        for moved in moves {
            let mut found = false;
            for delegate in &task.delegates {
                if *moved == Value::Number(delegate.chip.task) {
                    found = true;
                }
            }
            if !found {
                return Err(Failed::Fixed(b"move is not a live delegate"));
            }
        }
    }
    let mut delegates = List::with_capacity(task.delegates.len());
    for delegate in &task.delegates {
        let mut fate = shapes::Fate::Cancel;
        if let Some(Some(Values::Many(moves))) = call.values.get(1) {
            for moved in moves {
                if *moved == Value::Number(delegate.chip.task) {
                    fate = shapes::Fate::Move;
                }
            }
        }
        delegates.push(shapes::Delegate { task: delegate.chip.task, fate }).expect("one fate per delegate");
    }
    match shapes::close_chat(me, project, task.chip.task, delegates.as_slice(), words(call, 2, limits)?) {
        Ok(ask) => Ok(ask),
        Err(_) => Err(Failed::Fixed(b"close not offered")),
    }
}

fn draw_name(runner: &mut Runner, called: &mut Called) {
    // Tool 33 draws a key because check_program requires one even when verify omits --key.
    if (called.value.is_some() || called.call.tool == 33 || called.call.tool == crate::jig::rows::EXPORT_AT)
        && called.call.key.is_none()
    {
        let mut bytes = [0; 32];
        for chunk in bytes.chunks_mut(16) {
            let hex = crate::jig::digest::Digest::from_bytes(runner.names.next_u64().to_le_bytes()).hex();
            for (slot, byte) in chunk.iter_mut().zip(hex) {
                *slot = byte;
            }
        }
        called.call.key = Some(crate::jig::key::Name::new(&bytes).expect("drawn hexadecimal name"));
    }
}
