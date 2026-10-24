//! Each act is the person's own request, as the client would send it,
//! admitted by the root under their sign-in and a key derived from the
//! start's (protocol/setup.md, 8.4).
use super::{Decision, Delivery, Domain, Env, Limits, Work, applications, core, emit, forge, internal, people};
use alloc::boxed::Box;
use skein_lib::{ReplyTo, Time, Token};

// Zero is never a task number. One start and one act at a time use this reply.
const REPLY: u64 = 0;
#[derive(Debug)]
pub(crate) struct Context {
    pub sign_in: u64,
    pub key: [u8; 16],
    pub carried: Option<core::Carried>,
}
#[derive(Debug)]
pub(crate) struct Start {
    pub request: Token,
    pub phase: Phase,
    pub until: Time,
}
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    Waiting,
    Account,
    Connecting,
    Adoption,
}

pub(crate) fn owns(to: Token) -> bool {
    to == Token::new(REPLY)
}

fn key(original: [u8; 16], act: &[u8]) -> [u8; 16] {
    let mut hash = skein_lib::sha256::Sha256::new();
    hash.update(b"temper demo start\0");
    hash.update(&original);
    hash.update(act);
    let digest = hash.finish();
    digest.get(..16).expect("digest prefix").try_into().expect("16-byte derived key")
}

pub(crate) fn start(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, request: Token) {
    assert!(domain.demo_start.is_none(), "one start at a time");
    let demo = domain.forge.demo().expect("offered demo");
    let until = env.now.saturating_add(demo.deadline);
    domain.demo_start = Some(Start { request, phase: Phase::Waiting, until });
    if domain.demo_reply.is_none() {
        begin(domain, env, decision);
    }
}

fn begin(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision) {
    let Some(start) = &mut domain.demo_start else { return };
    if start.phase != Phase::Waiting {
        return;
    }
    start.phase = Phase::Account;
    let request = start.request;
    let provider = domain.forge.demo().expect("offered demo").provider;
    let mut held = false;
    for fact in &domain.core.accounts.account_views() {
        if let Some(account) = domain.core.accounts.account(fact.account) {
            held |= account.provider() == provider && account.holder() == jig_domain_accounts::Holder::Deployment;
        }
    }
    let context = domain.demo_contexts.get_mut(&request).expect("admitted start envelope");
    let carried = context.carried.take();

    if held {
        if let Some(carried) = carried {
            emit(decision, &env.limits, Delivery::CarriedUnused { number: carried.number });
        }
        domain.demo_start.as_mut().expect("start").phase = Phase::Connecting;
        progress(domain, env, decision);
    } else if let Some(carried) = carried {
        domain.demo_reply = Some((request, Phase::Account));
        domain.work.push(Work::Core(core::Event::Key {
            reply_to: internal(REPLY),
            sign_in: context.sign_in,
            key: key(context.key, b"account"),
            ask: Box::new(people::Ask::AccountChange {
                holder: people::AccountHolder::Deployment,
                change: people::AccountChange::AddKey { provider, join: None, digest: carried.digest },
            }),
            carried,
        }));
    } else {
        finish(domain, forge::StartAnswer::Failed(forge::StartFailure::Account));
    }
}

pub(crate) fn progress(domain: &mut Domain, env: &Env<Limits>, _decision: &mut Decision) {
    if domain.forge.demo_state() == Some(forge::DemoState::Ended) {
        domain.demo_start = None;
        return;
    }
    let Some(start) = &domain.demo_start else { return };
    let demo = domain.forge.demo().expect("offered demo");
    let connection = domain.forge.connection(demo.adoption.forge);
    let connected = connected(&connection);
    if !connected && env.now >= start.until {
        finish(domain, forge::StartAnswer::Failed(forge::StartFailure::NotConnected));
        return;
    }
    if start.phase != Phase::Connecting || !connected {
        return;
    }
    if domain.forge.demo_adopted() {
        finish(domain, forge::StartAnswer::Running);
        return;
    }
    let adoption = demo.adoption.clone();
    let request = start.request;
    let context = domain.demo_contexts.get(&request).expect("active start");
    let admission = applications::Admission {
        reply_to: internal(REPLY),
        sign_in: context.sign_in,
        key: key(context.key, b"adoption"),
        digest: skein_lib::sha256::digest(b"temper demo adoption"),
        scope: people::ApplicationScope::Project(adoption.project),
        carried: None,
    };
    domain.demo_start.as_mut().expect("start").phase = Phase::Adoption;
    domain.demo_reply = Some((request, Phase::Adoption));
    applications::start(domain, admission, crate::Application::Forge(forge::Ask::Adopt(adoption)));
}

fn finish(domain: &mut Domain, answer: forge::StartAnswer) {
    domain.demo_start = None;
    domain.work.push(Work::Forge(forge::Event::DemoAnswered { answer }));
}

pub(crate) fn reply(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    _to: ReplyTo,
    reply: people::Reply,
) {
    let Some((request, phase)) = domain.demo_reply.take() else { return };
    if !active(domain, request) {
        begin(domain, env, decision);
        return;
    }
    let kept = match reply {
        people::Reply::Outcome(people::Outcome::AccountKeyKept { .. }) => true,
        people::Reply::Outcome(_)
        | people::Reply::SignedIn { .. }
        | people::Reply::SignedOut
        | people::Reply::Again(_)
        | people::Reply::Full { .. }
        | people::Reply::Refused(_)
        | people::Reply::Signer(_) => false,
    };
    if phase == Phase::Account && kept {
        domain.demo_start.as_mut().expect("active start").phase = Phase::Connecting;
        progress(domain, env, decision);
    } else {
        let why = if phase == Phase::Account { forge::StartFailure::Account } else { forge::StartFailure::Adoption };
        finish(domain, forge::StartAnswer::Failed(why));
    }
}

pub(crate) fn adopted(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    outcome: Option<forge::Answered>,
) {
    let Some((request, _)) = domain.demo_reply.take() else { return };
    if !active(domain, request) {
        begin(domain, env, decision);
        return;
    }
    let answer = match outcome {
        Some(forge::Answered::Adopt(forge::ApplicationOutcome::Adopted(_))) => forge::StartAnswer::Running,
        Some(
            forge::Answered::Adopt(
                forge::ApplicationOutcome::CoreRefused(_) | forge::ApplicationOutcome::ForgeRefused(_),
            )
            | forge::Answered::SetUp(_)
            | forge::Answered::StartDemo(_)
            | forge::Answered::Connect(_)
            | forge::Answered::Install(_)
            | forge::Answered::Reachable(_),
        )
        | None => forge::StartAnswer::Failed(forge::StartFailure::Adoption),
    };
    finish(domain, answer);
}

pub(crate) fn connection(domain: &mut Domain, forge: u16, why: &[u8]) {
    if domain.demo_start.is_some()
        && let Some(demo) = domain.forge.demo()
        && demo.adoption.forge == forge
    {
        finish(
            domain,
            forge::StartAnswer::Failed(forge::StartFailure::Group {
                why: why.get(..why.len().min(256)).expect("bounded reason").into(),
            }),
        );
    }
}

fn active(domain: &Domain, request: Token) -> bool {
    match &domain.demo_start {
        Some(start) => start.request == request,
        None => false,
    }
}

pub(crate) fn connected(connection: &forge::Connection) -> bool {
    match connection {
        forge::Connection::Connected { .. } => true,
        forge::Connection::Connecting
        | forge::Connection::NotConnected
        | forge::Connection::NotSetUp
        | forge::Connection::SettingUp { .. }
        | forge::Connection::SetUpFailed { .. }
        | forge::Connection::Stopped { .. } => false,
    }
}

pub(crate) fn done(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, request: Token, success: bool) {
    if let Some(context) = domain.demo_contexts.remove(&request)
        && success
        && let Some(carried) = context.carried
    {
        emit(decision, &env.limits, Delivery::CarriedUnused { number: carried.number });
    }
    if let Some(start) = &domain.demo_start
        && start.request == request
    {
        domain.demo_start = None;
    }
}

fn end_key(number: u64, purpose: &[u8]) -> [u8; 16] {
    let mut hash = skein_lib::sha256::Sha256::new();
    hash.update(b"temper demo end\0");
    hash.update(&number.to_be_bytes());
    hash.update(purpose);
    hash.finish().get(..16).expect("digest prefix").try_into().expect("16-byte key")
}

/// Selection is read once, before any act changes the core, in the end's decision.
pub(crate) fn list_end_acts(domain: &mut Domain) {
    let demo = domain.forge.demo().expect("running configured demo");
    let mut acts = skein_lib::List::with_capacity(forge::demo_act_bound(&domain.limits.forge).expect("act bound"));
    for task in domain.core.tasks.view_tasks() {
        let row = domain.core.tasks.task(task.number).expect("live task row");
        let live = match row.phase {
            jig_domain_tasks::Phase::Waiting
            | jig_domain_tasks::Phase::Active(_)
            | jig_domain_tasks::Phase::Held { .. } => true,
            jig_domain_tasks::Phase::Closing(_) | jig_domain_tasks::Phase::Ended(_) => false,
        };
        if row.project == demo.adoption.project && live && domain.core.conversation_requester(row).is_some() {
            acts.push(forge::DemoAct {
                key: end_key(row.number, b"close"),
                act: forge::DemoActKind::Close { project: row.project, task: row.number },
            })
            .expect("one close per task");
        }
    }
    for fact in &domain.core.accounts.account_views() {
        if let Some(account) = domain.core.accounts.account(fact.account)
            && account.provider() == demo.provider
            && account.holder() == jig_domain_accounts::Holder::Deployment
        {
            acts.push(forge::DemoAct {
                key: end_key(u64::from(fact.account), b"remove"),
                act: forge::DemoActKind::RemoveAccount { account: fact.account },
            })
            .expect("one deployment account per provider");
            break;
        }
    }
    domain.work.push(Work::Forge(forge::Event::DemoActs { acts: acts.into_boxed() }));
}

pub(crate) fn send_end_act(domain: &mut Domain, row: forge::DemoAct) {
    let token = Token::new(domain.next_demo_act_reply);
    domain.next_demo_act_reply = domain.next_demo_act_reply.checked_add(1).expect("demo act reply sequence");
    domain.demo_act_replies.insert(token, row).expect("one reply per kept demo act");
    let act = match row.act {
        forge::DemoActKind::Close { project, task } => {
            let mut grace = None;
            for setting in &domain.core.current.offered() {
                let jig_domain_core::settings::Setting::Template(number) = setting else { continue };
                let template = domain.core.current.template(*number).expect("offered template");
                if template.flags.conversation {
                    grace = template.grace;
                    break;
                }
            }
            people::Act::Close { project, task, grace, reason: b"Ended with the playground".as_slice().into() }
        }
        forge::DemoActKind::RemoveAccount { account } => people::Act::RemoveAccount { account },
    };
    domain.work.push(Work::Core(core::Event::Act {
        reply_to: ReplyTo::new(token),
        connector: domain.config.forge_connector,
        key: row.key,
        act: Box::new(act),
        carried: None,
    }));
}

pub(crate) fn end_acted(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    outcome: people::Outcome,
) {
    let row = domain.demo_act_replies.remove(&to.into_token()).expect("demo end act reply");
    let (keep, why) = match &outcome {
        people::Outcome::Again(_) => (true, None),
        people::Outcome::Refused(people::Refusal::NotOffered) => (
            true,
            Some(match row.act {
                forge::DemoActKind::Close { .. } => "the forge connector is not granted close (engine.acts)",
                forge::DemoActKind::RemoveAccount { .. } => {
                    "the forge connector is not granted remove_account (engine.acts)"
                }
            }),
        ),
        people::Outcome::Refused(people::Refusal::Role { held: None }) => (true, Some("no first owner configured")),
        people::Outcome::Refused(people::Refusal::Unknown | people::Refusal::Ended { .. })
        | people::Outcome::Compound { .. }
        | people::Outcome::AccountRemoved { .. } => (false, None),
        people::Outcome::Refused(why) | people::Outcome::CompoundRefused { why, .. } => {
            (false, Some(refusal_name(why)))
        }
        people::Outcome::SettingsChanged { .. }
        | people::Outcome::Renamed { .. }
        | people::Outcome::ProjectMade { .. }
        | people::Outcome::Created { .. }
        | people::Outcome::MarkedSeen { .. }
        | people::Outcome::Applied { .. }
        | people::Outcome::SignedOut
        | people::Outcome::NoteEdited { .. }
        | people::Outcome::ServiceMade { .. }
        | people::Outcome::Adopted { .. }
        | people::Outcome::Proposed { .. }
        | people::Outcome::Withdrawn { .. }
        | people::Outcome::Stopped { .. }
        | people::Outcome::Cancelled { .. }
        | people::Outcome::Released { .. }
        | people::Outcome::PersonTaken { .. }
        | people::Outcome::PersonHandedBack { .. }
        | people::Outcome::PersonAnswered { .. }
        | people::Outcome::Moved { .. }
        | people::Outcome::ProposalDecided { .. }
        | people::Outcome::Said { .. }
        | people::Outcome::QuestionAnswered { .. }
        | people::Outcome::Amended { .. }
        | people::Outcome::RolesSet { .. }
        | people::Outcome::PolicyChanged { .. }
        | people::Outcome::PoolSet { .. }
        | people::Outcome::EscalationDecided { .. }
        | people::Outcome::AccountSignInStarted { .. }
        | people::Outcome::AccountFinishing { .. }
        | people::Outcome::AccountSignedOut { .. }
        | people::Outcome::AccountKeyKept { .. }
        | people::Outcome::AccountProgramDeclared { .. }
        | people::Outcome::AccountChanged
        | people::Outcome::AccountMovingOut { .. }
        | people::Outcome::AccountMovedIn { .. } => unreachable!("close or remove account answer"),
    };
    if let Some(why) = why {
        let line = end_note(row.act, why.as_bytes());
        emit(decision, &env.limits, Delivery::DemoNote { line });
    }
    domain.work.push(Work::Forge(forge::Event::DemoActAnswered { key: row.key, keep }));
}

fn refusal_name(why: &people::Refusal) -> &'static str {
    use people::Refusal;
    match why {
        Refusal::Stale { .. } => "Stale",
        Refusal::Full { .. } => "Full",
        Refusal::NameTaken => "NameTaken",
        Refusal::NotWellFormed { why: people::Malformed::Name(_), .. } => "Name",
        Refusal::Circle { .. } => "Circle",
        Refusal::Dependency { .. } => "Dependency",
        Refusal::Kept { .. } => "Kept",
        Refusal::Account { .. } => "Account",
        Refusal::NotWellFormed { .. } => "NotWellFormed",
        Refusal::Role { .. } => "Role",
        Refusal::Authority { .. } => "Authority",
        Refusal::Limit { .. } => "Limit",
        Refusal::Moved { .. } => "Moved",
        Refusal::Ended { .. } => "Ended",
        Refusal::Taken { .. } => "Taken",
        Refusal::NotWaiting => "NotWaiting",
        Refusal::NoFurther => "NoFurther",
        Refusal::Unknown => "Unknown",
        Refusal::NotOffered => "NotOffered",
        Refusal::Provider => "Provider",
        Refusal::SignInRedirect { .. } => "SignInRedirect",
        Refusal::KeyConflict => "KeyConflict",
        Refusal::NoEscalationHolder => "NoEscalationHolder",
        Refusal::Llm { .. } => "Llm",
        Refusal::AccountMove { .. } => "AccountMove",
    }
}

fn end_note(act: forge::DemoActKind, why: &[u8]) -> Box<[u8]> {
    let (subject, number, action) = match act {
        forge::DemoActKind::Close { task, .. } => (b"chat ".as_slice(), task, b" not closed: ".as_slice()),
        forge::DemoActKind::RemoveAccount { account } => {
            (b"account ".as_slice(), u64::from(account), b" not removed: ".as_slice())
        }
    };
    let number = super::forge_route::decimal(number);
    let parts = [b"demo end: ".as_slice(), subject, number.as_ref(), action, why];
    let mut length = 0_usize;
    for part in parts {
        length = length.checked_add(part.len()).expect("bounded demo diagnostic");
    }
    assert!(length <= 256, "bounded secret-free diagnostic");
    let mut writer = skein_lib::Writer::new(length);
    for part in parts {
        writer.put(part).expect("measured diagnostic");
    }
    writer.finish()
}
