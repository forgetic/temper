//! Durable answers use the same operation words before and after a restart.
use super::{Text, acts, document};
use crate::jig::{Limits, runner::Printed};
use alloc::boxed::Box;
use jig_client::{Answered, Ask, Outcome, contract as c};
use skein_json::writer::Encoder;
#[derive(Debug)]
pub(crate) struct Operation {
    pub exported: Option<(u32, u32)>,
    pub file: Option<Box<[u8]>>,
    pub word: &'static [u8],
    pub account: Option<Account>,
    pub task: Option<u64>,
    pub to: Option<c::Recipient>,
    pub party: Option<u64>,
    pub retry: Option<Box<[u8]>>,
    pub value_name: Option<crate::jig::key::Name>,
}
/// One in-flight source context; its name is bounded by the client's document text limit.
#[derive(Clone, Debug)]
pub(crate) struct Account {
    pub provider: Option<u32>,
    pub provider_name: Option<c::DocumentText>,
    pub program: bool,
    pub sign_in: bool,
}
impl Operation {
    pub(crate) fn of(ask: &Ask) -> Self {
        let (word, task): (&[u8], Option<u64>) = match ask {
            Ask::Family { .. } => (b"adopt", None),
            Ask::Jig(ask) => match ask.as_ref() {
                c::Ask::Propose { action: c::ProposeAction::Settings(_), .. } => (b"propose_settings", None),
                c::Ask::ChangeSettings(_) => unreachable!("the tools do not request settings"),
                c::Ask::Create { .. } | c::Ask::Propose { action: c::ProposeAction::Batch { .. }, .. } => {
                    (b"chat", None)
                }
                c::Ask::Message { task, .. } => (b"message", Some(*task)),
                c::Ask::Compound { operations, .. } => {
                    let mut task = None;
                    for operation in operations {
                        if let c::Operation::Cancel { task: number, .. } = operation {
                            task = Some(*number);
                        }
                    }
                    (b"close", task)
                }
                c::Ask::Answer { on, .. } => (
                    b"answer",
                    Some(match on {
                        c::AnswerOn::Question { task, .. } | c::AnswerOn::Person { task, .. } => *task,
                    }),
                ),
                c::Ask::Decide { on, .. } => (
                    b"decide",
                    match on {
                        c::DecideOn::Escalation { task, .. } => Some(*task),
                        c::DecideOn::Proposal { proposal, .. } => match proposal.proposer {
                            c::Proposer::Task(task) => Some(task),
                            c::Proposer::Party(_) => None,
                        },
                    },
                ),
                c::Ask::Release { task, .. } => (b"release", Some(*task)),
                c::Ask::Stop { task, .. } => (b"stop", Some(*task)),
                c::Ask::Cancel { task, .. } => (b"cancel", Some(*task)),
                c::Ask::Take { task, .. } => (b"take", Some(*task)),
                c::Ask::HandBack { task, .. } => (b"hand-back", Some(*task)),
                c::Ask::Amend { task, .. }
                | c::Ask::Propose { action: c::ProposeAction::Amendment { task, .. }, .. } => (b"amend", Some(*task)),
                c::Ask::Move { task, .. } => (b"move", Some(*task)),
                c::Ask::Seen { .. } => (b"seen", None),
                c::Ask::EditNote { .. } => (b"edit-note", None),
                c::Ask::DeleteNote { .. } => (b"delete-note", None),
                c::Ask::ChangePolicy { .. } => (b"change-policy", None),
                c::Ask::SetRoles { .. } => (b"set-roles", None),
                c::Ask::MakeProject { .. } => (b"make-project", None),
                c::Ask::Rename { .. } => (b"rename", None),
                c::Ask::MakeService { .. } => (b"make-service", None),
                c::Ask::SetPool { .. } => (b"set-pool", None),
                c::Ask::Withdraw { .. } => (b"withdraw", None),
                c::Ask::SignIn { .. } => (b"connect", None),
                c::Ask::SignOut { .. } => (b"disconnect", None),
                c::Ask::AccountChange { change, .. } => (
                    match change {
                        c::AccountChange::MoveOut { .. } | c::AccountChange::Written { .. } => b"export".as_slice(),
                        c::AccountChange::MoveIn { .. } => b"import",
                        c::AccountChange::DeclareProgram { .. } => b"declare".as_slice(),
                        c::AccountChange::CheckProgram { .. } => b"verify",
                        c::AccountChange::AddKey { .. } => b"add-key".as_slice(),
                        c::AccountChange::ReplaceKey { .. } => b"replace-key",
                        c::AccountChange::Resume { .. } => b"retry",
                        c::AccountChange::MoveAccount { .. } | c::AccountChange::MoveSource { .. } => b"rank",
                        c::AccountChange::Remove { .. } | c::AccountChange::RemoveKey { .. } => b"forget",
                        c::AccountChange::SetUses { .. } | c::AccountChange::FinishSignIn { .. } => b"account-change",
                    },
                    None,
                ),
            },
        };
        Self {
            exported: None,
            file: None,
            word,
            account: None,
            task,
            to: None,
            party: None,
            retry: None,
            value_name: None,
        }
    }
}
pub(crate) fn print(
    operation: &Operation,
    outcome: &Outcome,
    json: bool,
    me: Option<&c::Me>,
    limits: &Limits,
) -> Printed {
    let status = match outcome {
        Outcome::Answered(Answered::Jig(c::Answer::Done(_))) => 0,
        Outcome::Answered(Answered::Jig(c::Answer::Refused { .. })) => 2,
        Outcome::NotKept
        | Outcome::Answered(Answered::Family(_))
        | Outcome::Unknown
        | Outcome::Limit
        | Outcome::Unconfirmed
        | Outcome::Held
        | Outcome::Unsent
        | Outcome::Lost => 1,
    };
    let bytes = bytes(operation, outcome, json, me, limits, false);
    if status == 0 {
        Printed { file: crate::jig::FileEnd::Keep, status, stdout: bytes, stderr: Box::default() }
    } else {
        Printed { file: crate::jig::FileEnd::Keep, status, stdout: Box::default(), stderr: bytes }
    }
}
pub(crate) fn kept(
    operation: &Operation,
    outcome: &Outcome,
    json: bool,
    me: Option<&c::Me>,
    limits: &Limits,
) -> Box<[u8]> {
    bytes(operation, outcome, json, me, limits, true)
}
fn bytes(
    operation: &Operation,
    outcome: &Outcome,
    json: bool,
    me: Option<&c::Me>,
    limits: &Limits,
    kept: bool,
) -> Box<[u8]> {
    if json {
        let bounds = skein_json::writer::Limits {
            depth: 32,
            length: u32::try_from(limits.result_bytes).expect("output bytes fit"),
        };
        let mut measure = Encoder::measure(&bounds);
        encode(operation, outcome, &mut measure, kept, me, limits);
        let Ok(size) = measure.measured() else { return super::failure(b"result limit", true, 1).stderr };
        let mut write = Encoder::write(size, &bounds);
        encode(operation, outcome, &mut write, kept, me, limits);
        let bytes = write.finish();
        if kept { newline(&bytes) } else { bytes }
    } else {
        let mut measure = Text::measure();
        text(operation, outcome, me, limits, kept, &mut measure);
        let mut write = Text::writing_exact(measure);
        text(operation, outcome, me, limits, kept, &mut write);
        write.finish()
    }
}
fn client_reason<'a>(operation: &'a Operation, outcome: &Outcome) -> Option<[&'a [u8]; 4]> {
    let reason: Option<&[u8]> = match outcome {
        Outcome::NotKept => Some(b"request not kept"),
        Outcome::Unknown => Some(b"unknown past retention"),
        Outcome::Limit => Some(b"request limit"),
        Outcome::Held => Some(b"name held by another request"),
        Outcome::Unconfirmed => Some(b"tools' bug; unconfirmed request; report it as a finding"),
        Outcome::Answered(Answered::Family(_)) => Some(b"tools' bug; unexpected family answer; report it as a finding"),
        Outcome::Answered(Answered::Jig(_)) => None,
        Outcome::Unsent => Some(b"unsent"),
        Outcome::Lost => Some(b"lost"),
    };
    let reason = reason?;
    let name = if let Outcome::Unsent | Outcome::Lost = outcome { operation.value_name.as_ref() } else { None };
    Some(match name {
        Some(name) => [reason, b"; supply --key ", name.bytes(), b" and the same file again"],
        None => [reason, b"", b"", b""],
    })
}
fn text(operation: &Operation, outcome: &Outcome, me: Option<&c::Me>, limits: &Limits, kept: bool, out: &mut Text) {
    if kept {
        out.put(b"kept: ");
        out.put(operation.word);
        out.put(b", ");
    }
    if let Some(reason) = client_reason(operation, outcome) {
        for part in reason {
            out.put(part);
        }
        out.put(b"\n");
        return;
    }
    let Outcome::Answered(Answered::Jig(answer)) = outcome else { unreachable!("client answers handled") };
    match answer {
        c::Answer::Done(_) if operation.exported.is_some() => moved_out(operation, out),
        c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::AccountMovedIn { account, renewed })) => {
            moved_in(*account, *renewed, out);
        }
        c::Answer::Done(c::Outcome::Proposed { proposal }) => proposed(*proposal, operation, me, out),
        c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::Proposed { proposal })) => proposed(
            c::ProposalKey {
                proposer: c::Proposer::Party(operation.party.expect("request party retained")),
                number: *proposal,
            },
            operation,
            me,
            out,
        ),
        c::Answer::Done(c::Outcome::DecidedFirst { by, how, at }) => {
            out.put(b"decided first: by ");
            out.put(by.name.bytes());
            out.put(b", ");
            decision(how, out);
            out.put(b", ");
            document::utc(*at, out);
        }
        c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::AccountProgramDeclared { source, name })) => {
            out.put(b"Program source ");
            out.number(u64::from(*source));
            out.put(b": ");
            out.put(name);
            out.put(b"\nRun on each host: ");
            out.put(limits.program_login);
            out.put(b" ");
            out.put(name);
        }
        c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::AccountChanged)) if operation.word == b"verify" => {
            out.put(b"asked\nwait accounts --until settled");
        }
        c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::AccountSignInStarted { address, .. })) => {
            out.put(b"Open this address in a browser on the engine's machine:\n");
            out.put(address);
            out.put(b"\nwait accounts --until settled");
        }
        c::Answer::Done(c::Outcome::Owner(
            c::OwnerOutcome::AccountSignedOut { confirmed, failed_hosts }
            | c::OwnerOutcome::AccountRemoved { confirmed, failed_hosts },
        )) => {
            sign_out(operation, *confirmed, failed_hosts, me, limits, out);
        }
        c::Answer::Done(done) => {
            out.put(b"done: ");
            let task = match done {
                c::Outcome::SettingsChanged(_) => unreachable!("the tools do not request settings"),
                c::Outcome::Made { what: c::Made::Tasks { tasks } } => tasks.get(0).copied(),
                c::Outcome::Owner(c::OwnerOutcome::Created { first, .. }) => Some(*first),
                c::Outcome::Owner(_)
                | c::Outcome::Made { .. }
                | c::Outcome::Proposed { .. }
                | c::Outcome::Decided { .. }
                | c::Outcome::DecidedFirst { .. }
                | c::Outcome::Changed => operation.task,
            };
            if let Some(task) = task {
                out.put(b"task ");
                out.number(task);
                out.put(b", ");
            }
            out.put(operation.word);
        }
        c::Answer::Refused { refusal, .. } => {
            out.put(operation.word);
            out.put(b": ");
            acts::refusal(refusal, &document::phrasebook(limits.text), out);
            if let c::Refusal::Moved { .. } = refusal
                && let Some(task) = operation.task
            {
                out.put(b"\nread again: task ");
                out.number(task);
            }
            if let c::Refusal::Authority { proposable: Some(true), .. } = refusal {
                out.put(b"\npropose instead: ");
                match &operation.retry {
                    Some(call) => out.put(call),
                    None => out.put(operation.word),
                }
            }
        }
    }
    out.put(b"\n");
}
fn proposed(proposal: c::ProposalKey, operation: &Operation, me: Option<&c::Me>, out: &mut Text) {
    out.put(b"proposed: ");
    let name = crate::jig::argv::value::encode(
        &[],
        crate::jig::table::Kind::Proposal,
        &crate::jig::call::Value::Proposal(proposal),
    )
    .expect("proposal identity is printable");
    out.put(&name);
    if let Some(to) = &operation.to {
        out.put(b", to ");
        acts::recipient(to, me, out);
    }
}
fn decision(how: &c::Decision, out: &mut Text) {
    out.put(match how {
        c::Decision::Accepted => b"accepted",
        c::Decision::Rejected { .. } => b"rejected",
        c::Decision::PassedUp => b"passed up",
        c::Decision::Released { .. } => b"released",
        c::Decision::LeftHeld { .. } => b"left held",
    });
}
fn encode(
    operation: &Operation,
    outcome: &Outcome,
    out: &mut Encoder,
    kept: bool,
    me: Option<&c::Me>,
    limits: &Limits,
) {
    out.object_start();
    out.key(b"operation");
    out.string(operation.word);
    out.key(b"outcome");
    if let Some(reason) = client_reason(operation, outcome) {
        out.object_start();
        out.key(b"error");
        out.string_parts(&reason);
        out.object_end();
    } else {
        let Outcome::Answered(Answered::Jig(answer)) = outcome else { unreachable!("client answers handled") };
        if let (Some((account, number)), c::Answer::Done(_)) = (operation.exported, answer) {
            out.object_start();
            out.key(b"account");
            out.unsigned(u64::from(account));
            out.key(b"number");
            out.unsigned(u64::from(number));
            out.key(b"file");
            out.string(operation.file.as_deref().expect("export file"));
            out.object_end();
        } else {
            document::answer_json(answer, out);
        }
    }
    if let Outcome::Answered(Answered::Jig(c::Answer::Done(c::Outcome::Owner(
        c::OwnerOutcome::AccountProgramDeclared { name, .. },
    )))) = outcome
    {
        out.key(b"next");
        out.string_parts(&[b"Run on each host: ", limits.program_login, b" ", name]);
    }
    if let Some(next) = account_follow_up(operation, outcome) {
        out.key(b"next");
        out.string(next);
    }
    if let Outcome::Answered(Answered::Jig(c::Answer::Done(c::Outcome::Owner(
        c::OwnerOutcome::AccountSignedOut { confirmed, failed_hosts }
        | c::OwnerOutcome::AccountRemoved { confirmed, failed_hosts },
    )))) = outcome
    {
        let mut measure = Text::measure();
        if sign_out(operation, *confirmed, failed_hosts, me, limits, &mut measure) {
            let mut write = Text::writing_exact(measure);
            sign_out(operation, *confirmed, failed_hosts, me, limits, &mut write);
            out.key(b"next");
            out.string(&write.finish());
        }
    }
    if !kept {
        out.key(b"task");
        match operation.task {
            Some(task) => out.unsigned(task),
            None => out.null(),
        }
        out.key(b"to");
        match &operation.to {
            Some(to) => document::recipient_json(to, out),
            None => out.null(),
        }
    }
    if let Outcome::Answered(Answered::Jig(c::Answer::Refused { refusal, .. })) = outcome {
        if let c::Refusal::Moved { .. } = refusal
            && let Some(task) = operation.task
        {
            let mut measured = Text::measure();
            read_again(task, &mut measured);
            let mut write = Text::writing_exact(measured);
            read_again(task, &mut write);
            out.key(b"read_again");
            out.string(&write.finish());
        }
        if let c::Refusal::Authority { proposable: Some(true), .. } = refusal {
            out.key(b"propose_instead");
            out.string(operation.retry.as_deref().unwrap_or(operation.word));
        }
    }
    out.object_end();
}
fn read_again(task: u64, out: &mut Text) {
    out.put(b"task ");
    out.number(task);
}
pub(crate) fn newline(bytes: &[u8]) -> Box<[u8]> {
    let mut out = skein_lib::Writer::new(bytes.len().checked_add(1).expect("line size fits"));
    out.put(bytes).expect("measured line");
    out.put(b"\n").expect("measured newline");
    out.finish()
}
pub(crate) fn retry(call: &crate::jig::call::Call) -> Box<[u8]> {
    use crate::jig::{argv, rows};
    let tool = match call.tool {
        8 => &rows::CHAT_CREATE,
        9 => &rows::MESSAGE,
        10 => &rows::CLOSE,
        11 => &rows::ANSWER,
        12 => &rows::DECIDE,
        13 => &rows::RELEASE,
        14 => &rows::STOP,
        15 => &rows::CANCEL,
        24..=30 | 32 | 33 | rows::EXPORT_AT | rows::IMPORT_AT => account_tool(call.tool),
        31 => &rows::AMEND,
        _ => unreachable!("act tool"),
    };
    let mut call = call.clone();
    call.tool = 0;
    let mut words = skein_lib::List::with_capacity(128);
    argv::back(&[tool], &call, &mut words).expect("parsed act");
    let mut measure = Text::measure();
    retry_words(words.as_slice(), &mut measure);
    let mut out = Text::writing_exact(measure);
    retry_words(words.as_slice(), &mut out);
    out.finish()
}
fn retry_words(words: &[Box<[u8]>], out: &mut Text) {
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            out.put(b" ");
        }
        super::lists::argument(word, out);
    }
}

pub(crate) fn account_tool(tool: u8) -> &'static crate::jig::table::Tool {
    use crate::jig::rows;
    match tool {
        24 => &rows::CONNECT,
        25 => &rows::DISCONNECT,
        26 => &rows::ADD_KEY,
        27 => &rows::REPLACE_KEY,
        28 => &rows::FORGET,
        29 => &rows::RANK,
        30 => &rows::RETRY,
        32 => &rows::DECLARE,
        33 => &rows::VERIFY,
        rows::EXPORT_AT => &rows::EXPORT,
        rows::IMPORT_AT => &rows::IMPORT,
        _ => unreachable!("account tool"),
    }
}

fn account_follow_up(operation: &Operation, outcome: &Outcome) -> Option<&'static [u8]> {
    match outcome {
        Outcome::Answered(Answered::Jig(c::Answer::Done(c::Outcome::Owner(
            c::OwnerOutcome::AccountSignInStarted { .. },
        )))) => Some(b"wait accounts --until settled"),
        Outcome::Answered(Answered::Jig(c::Answer::Done(c::Outcome::Owner(c::OwnerOutcome::AccountChanged))))
            if operation.word == b"verify" =>
        {
            Some(b"wait accounts --until settled")
        }
        Outcome::NotKept
        | Outcome::Answered(_)
        | Outcome::Unknown
        | Outcome::Limit
        | Outcome::Unconfirmed
        | Outcome::Held
        | Outcome::Unsent
        | Outcome::Lost => None,
    }
}

fn sign_out(
    operation: &Operation,
    confirmed: bool,
    failed_hosts: &[c::AccountHostFailure],
    me: Option<&c::Me>,
    limits: &Limits,
    out: &mut Text,
) -> bool {
    let mut advice = false;
    let program = match &operation.account {
        Some(account) => account.program,
        None => !failed_hosts.is_empty(),
    };
    if program {
        if failed_hosts.is_empty() {
            out.put(b"Signed out on every machine.");
        } else {
            advice = true;
            out.put(b"Not signed out on:");
            for host in failed_hosts {
                out.put(b"\n");
                out.put(&host.name);
                out.put(match host.why {
                    c::AccountHostWhy::NotInContact => b": not in contact",
                    c::AccountHostWhy::Lost => b": contact lost",
                    c::AccountHostWhy::Failed => b": failed",
                });
            }
        }
    }
    let sign_in = match &operation.account {
        Some(account) => account.sign_in,
        None => false,
    };
    // With failed hosts, a mixed removal cannot tell us whether its sign-in was revoked.
    if !confirmed && (sign_in || failed_hosts.is_empty()) {
        advice = true;
        if program {
            out.put(b"\n");
        }
        out.put(limits.application);
        out.put(b" removed its tokens. Disconnect ");
        out.put(limits.application);
        out.put(b" in ");
        let mut named = false;
        if let Some(account) = &operation.account {
            if let Some(name) = &account.provider_name
                && !name.cut
            {
                out.put(name.bytes());
                out.put(b"'s settings, under Apps.");
                named = true;
            }
            if !named && let Some(me) = me {
                for offer in &me.providers {
                    if Some(offer.provider) == account.provider && !offer.name.cut {
                        out.put(offer.name.bytes());
                        out.put(b"'s settings, under Apps.");
                        named = true;
                        break;
                    }
                }
            }
        }
        if !named {
            out.put(b"the provider's settings, under Apps.");
        }
    } else if !program {
        out.put(b"done: ");
        out.put(operation.word);
    }
    advice
}

fn moved_out(operation: &Operation, out: &mut Text) {
    out.put(b"moved out: account ");
    out.number(u64::from(operation.exported.expect("exported").0));
    out.put(b" to ");
    out.put(operation.file.as_deref().expect("export file"));
}
fn moved_in(account: u32, renewed: bool, out: &mut Text) {
    out.put(b"moved in: account ");
    out.number(u64::from(account));
    if renewed {
        out.put(b", renewed");
    }
}
