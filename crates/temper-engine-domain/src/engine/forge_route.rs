//! Root translation for the forge connector. Its store rows and released API
//! calls cross one root decision; the connector never owns the store.
use super::{
    CallAnswer, CallKey, Decision, Delivery, Domain, Env, Family, ForgeRepository, ForgeStart, ForgeWorkspace, Id, Key,
    Limits, List, ProcedureAction, Queue, Record, ReplyTo, RoutedCall, Token, Work, Write, authority,
    authority_numbers, authority_value, decide_call, emit, escalation, forge, forge_change, forge_client, forge_issues,
    people, policy_translate, procedure_step, save, tasks,
};
use alloc::boxed::Box;
use jig_core_brief as brief;

fn append(out: &mut List<u8>, bytes: &[u8]) {
    for byte in bytes {
        if out.room() > 0 {
            out.push(*byte).expect("checked message room");
        }
    }
}

fn append_hex(out: &mut List<u8>, bytes: &[u8]) {
    for byte in bytes {
        let high = *b"0123456789abcdef".get(usize::from(byte >> 4_u8)).expect("high nibble is hexadecimal");
        let low = *b"0123456789abcdef".get(usize::from(byte & 15_u8)).expect("low nibble is hexadecimal");
        append(out, &[high, low]);
    }
}

fn news_words(news: &forge::News, limit: u32) -> Box<[u8]> {
    let mut out = List::with_capacity(limit);
    match news {
        forge::News::Landing { before, after, .. } => {
            append(&mut out, b"forge landing ");
            append_hex(&mut out, &before[..4]);
            append(&mut out, b" -> ");
            append_hex(&mut out, &after[..4]);
        }
        forge::News::Ci { head, status } => {
            append(&mut out, b"forge ci ");
            let label: &[u8] = match status {
                forge_client::api::Ci::None => b"none",
                forge_client::api::Ci::Pending => b"pending",
                forge_client::api::Ci::Passed => b"passed",
                forge_client::api::Ci::Failed => b"failed",
            };
            append(&mut out, label);
            append(&mut out, b" at ");
            append_hex(&mut out, &head[..4]);
        }
        forge::News::Changed { number } => {
            append(&mut out, b"forge item ");
            append_hex(&mut out, &number.to_be_bytes());
            append(&mut out, b" changed");
        }
    }
    out.into_boxed()
}

fn topic_name(topic: &forge::Topic, limits: &forge::Limits) -> Option<forge::Name> {
    let (repository, what) = match topic {
        forge::Topic::Landings { repository, branch } => {
            let mut parts = List::with_capacity(limits.name_bytes);
            let mut start = 0;
            for at in 0..=branch.len() {
                if at == branch.len() || branch.get(at) == Some(&b'/') {
                    let part = branch.get(start..at)?;
                    if part.is_empty() || parts.push(Box::from(part)).is_err() {
                        return None;
                    }
                    start = at.checked_add(1)?;
                }
            }
            (*repository, forge::What::Branch(parts.into_boxed()))
        }
        forge::Topic::Pull { repository, number } => (*repository, forge::What::Pull(*number)),
        forge::Topic::Participation { repository, number } => (*repository, forge::What::Issue(*number)),
        forge::Topic::Ci { repository, .. } => (*repository, forge::What::Repository),
    };
    Some(forge::Name { forge: repository.forge, repository: repository.repository, what })
}

pub(super) fn watch_names(
    domain: &Domain,
    limits: &Limits,
    subscriber: &forge::Subscriber,
) -> Option<Box<[forge::Name]>> {
    let name = topic_name(&subscriber.topic, &limits.forge)?;
    let mut names = List::with_capacity(limits.forge.resources_per_task);
    if let Some(existing) = domain.forge.names(subscriber.task) {
        for old in existing {
            names.push(old.clone()).ok()?;
            if *old == name {
                return Some(names.into_boxed());
            }
        }
    }
    names.push(name).ok()?;
    Some(names.into_boxed())
}

pub(super) fn claim_names(
    domain: &Domain,
    limits: &Limits,
    task: u64,
    writes: &[forge::Name],
) -> Option<Box<[forge::Name]>> {
    let mut names = List::with_capacity(limits.forge.resources_per_task);
    if let Some(existing) = domain.forge.names(task) {
        for name in existing {
            names.push(name.clone()).ok()?;
        }
    }
    for write in writes {
        if !names.as_slice().contains(write) {
            names.push(write.clone()).ok()?;
        }
    }
    Some(names.into_boxed())
}

pub(super) fn rows(limits: &Limits) -> Option<u32> {
    let forge = limits.forge;
    forge
        .repositories
        .checked_add(forge.holds)?
        .checked_add(forge.tasks)?
        .checked_add(forge.tasks)?
        .checked_add(forge.subscriptions)?
        .checked_add(forge.client.resources.checked_mul(3)?)?
        .checked_add(forge.subscriptions)?
        .checked_add(forge.landings)?
        .checked_add(forge.entries)?
        .checked_add(forge.client.repositories)?
        .checked_add(forge.changes)?
        .checked_add(forge.issues)
}

fn decimal(number: u64) -> Box<[u8]> {
    let mut digits = List::with_capacity(20);
    let mut value = number;
    for _ in 0_u32..20_u32 {
        digits
            .push(b'0'.checked_add(u8::try_from(value % 10).expect("decimal digit")).expect("decimal ascii digit"))
            .expect("u64 decimal width");
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut forward = List::with_capacity(20);
    for at in (0..digits.len()).rev() {
        forward.push(*digits.get(at).expect("measured decimal digit")).expect("same width");
    }
    forward.into_boxed()
}

fn read_name(repository: &forge::Repository, read: &forge_client::api::Read, limit: u32) -> Option<authority::Name> {
    let mut segments = List::with_capacity(limit);
    for segment in [b"forge".as_slice(), &repository.host, &repository.owner, &repository.name] {
        segments.push(Box::from(segment)).ok()?;
    }
    let what = match read {
        forge_client::api::Read::Pull { number }
        | forge_client::api::Read::Reviews { number, .. }
        | forge_client::api::Read::Remarks { number, .. }
        | forge_client::api::Read::PullFiles { number, .. } => Some((b"pull".as_slice(), Some(decimal(*number)))),
        forge_client::api::Read::Item { number, .. } => Some((b"issue".as_slice(), Some(decimal(*number)))),
        forge_client::api::Read::Branch { branch } | forge_client::api::Read::Protection { branch } => {
            Some((b"branch".as_slice(), Some(branch.clone())))
        }
        forge_client::api::Read::Items { .. }
        | forge_client::api::Read::PullFor { .. }
        | forge_client::api::Read::Statuses { .. }
        | forge_client::api::Read::Compare { .. }
        | forge_client::api::Read::Checks { .. }
        | forge_client::api::Read::Job { .. }
        | forge_client::api::Read::Branches
        | forge_client::api::Read::Settings
        | forge_client::api::Read::Collaborators { .. }
        | forge_client::api::Read::Permission { .. } => None,
    };
    if let Some((kind, part)) = what {
        segments.push(Box::from(kind)).ok()?;
        if let Some(part) = part {
            let mut start = 0;
            for at in 0..=part.len() {
                if at == part.len() || part.get(at) == Some(&b'/') {
                    let segment = part.get(start..at)?;
                    if segment.is_empty() {
                        return None;
                    }
                    segments.push(Box::from(segment)).ok()?;
                    start = at.checked_add(1)?;
                }
            }
        }
    }
    Some(authority::Name { segments: segments.into_boxed() })
}

fn resource_name(repository: &forge::Repository, what: &forge::What, limit: u32) -> Option<authority::Name> {
    let mut segments = List::with_capacity(limit);
    for segment in [b"forge".as_slice(), &repository.host, &repository.owner, &repository.name] {
        segments.push(Box::from(segment)).ok()?;
    }
    match what {
        forge::What::Repository => {}
        forge::What::Issue(number) => {
            segments.push(Box::from(&b"issue"[..])).ok()?;
            segments.push(decimal(*number)).ok()?;
        }
        forge::What::Pull(number) => {
            segments.push(Box::from(&b"pull"[..])).ok()?;
            segments.push(decimal(*number)).ok()?;
        }
        forge::What::Branch(parts) => {
            segments.push(Box::from(&b"branch"[..])).ok()?;
            for part in parts {
                if part.is_empty() || part.contains(&b'/') {
                    return None;
                }
                segments.push(part.clone()).ok()?;
            }
        }
    }
    Some(authority::Name { segments: segments.into_boxed() })
}

fn effect_access(repository: &forge::Repository, what: &forge::What, kind: u16) -> authority::EffectAccess {
    let permitted = match kind {
        1 => repository.kinds.read,
        2 => repository.kinds.push,
        3 => repository.kinds.open,
        4 => repository.kinds.land,
        5 => repository.kinds.review,
        6 => repository.kinds.status,
        7 => repository.kinds.comment,
        8 => repository.kinds.issue,
        9 => repository.kinds.branch,
        _ => false,
    };
    let shared = match what {
        forge::What::Issue(_) | forge::What::Pull(_) => true,
        forge::What::Repository | forge::What::Branch(_) => false,
    };
    match repository.role {
        forge::Role::Context => authority::EffectAccess::Context,
        _ if !permitted => authority::EffectAccess::Unavailable,
        _ if shared => authority::EffectAccess::Participant,
        forge::Role::Adopted => authority::EffectAccess::Participant,
        forge::Role::Fork if kind == 4 => authority::EffectAccess::Participant,
        forge::Role::Owned | forge::Role::Fork => authority::EffectAccess::Owned,
    }
}

fn effect_key(domain: &Domain, key: CallKey) -> Box<[u8]> {
    let mut result = List::with_capacity(80);
    append_hex(&mut result, &domain.counters.deployment().id);
    append_hex(&mut result, &key.task.to_be_bytes());
    append_hex(&mut result, &key.attempt.to_be_bytes());
    append_hex(&mut result, &key.completion.to_be_bytes());
    append_hex(&mut result, &key.position.to_be_bytes());
    result.into_boxed()
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one named call carries the root and its typed forge write"
)]
pub(super) fn effect_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    repository: forge_client::api::Repository,
    resource: forge::What,
    write: forge_client::api::Write,
) {
    let Some(adopted) = domain.forge.repository(repository) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeEffectRefused(forge_client::api::Error::Missing),
        );
        return;
    };
    let Some(context) = domain.tasks.delegation(key.task) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeEffectRefused(forge_client::api::Error::Forbidden),
        );
        return;
    };
    let (write, kind, state, permitted) = match write {
        forge_client::api::Write::CreateIssue { title, body, .. } => (
            forge_client::api::Write::CreateIssue { key: effect_key(domain, key), title, body },
            8_u16,
            [0; 32],
            resource == forge::What::Repository && adopted.kinds.issue,
        ),
        forge_client::api::Write::Post { number, body, .. } => (
            forge_client::api::Write::Post { number, key: effect_key(domain, key), body },
            7_u16,
            [0; 32],
            (resource == forge::What::Issue(number) || resource == forge::What::Pull(number)) && adopted.kinds.comment,
        ),
        forge_client::api::Write::Status { commit, context, check } => (
            forge_client::api::Write::Status { commit, context, check },
            6_u16,
            commit,
            resource == forge::What::Repository && adopted.kinds.status,
        ),
        forge_client::api::Write::OpenPull { .. }
        | forge_client::api::Write::Review { .. }
        | forge_client::api::Write::Edit { .. }
        | forge_client::api::Write::SetReviewers { .. }
        | forge_client::api::Write::Close { .. }
        | forge_client::api::Write::Reopen { .. }
        | forge_client::api::Write::Merge { .. }
        | forge_client::api::Write::Update { .. }
        | forge_client::api::Write::CreateBranch { .. }
        | forge_client::api::Write::DeleteBranch { .. } => {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::ForgeEffectRefused(forge_client::api::Error::Forbidden),
            );
            return;
        }
    };
    let Some(name) = resource_name(adopted, &resource, env.limits.authority.segments) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeEffectRefused(forge_client::api::Error::TooLarge),
        );
        return;
    };
    let effect = forge_client::Effect { write, condition: forge_client::Condition::None };
    let valid = match forge_client::effect_bytes(&effect) {
        Some(bytes) => bytes <= u64::from(env.limits.forge.client.op_bytes),
        None => false,
    };
    if !permitted || context.project != adopted.project || adopted.role == forge::Role::Context || !valid {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeEffectRefused(forge_client::api::Error::Forbidden),
        );
        return;
    }
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority findings"));
    let checked = authority::check_effect(
        &domain.config.authority,
        &authority::EffectAsk {
            project: context.project,
            authority: authority_value(&context.authority),
            numbers: authority_numbers(context.numbers),
            effect: authority::Effect {
                connector: domain.config.forge_connector,
                kind,
                name,
                state,
                price: None,
                access: effect_access(adopted, &resource, kind),
                additional: Box::new([]),
                guards: Box::new([]),
            },
            now: env.wall,
        },
        &[],
        &mut findings,
    );
    if checked != authority::Answer::Allow {
        let mut reasons =
            List::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority findings"));
        for _ in 0..findings.len() {
            reasons.push(findings.pop().expect("counted finding")).expect("reserved finding room");
        }
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeEffectDenied { answer: checked, findings: reasons.into_boxed() },
        );
        return;
    }
    let Some(entry) = crate::fresh(&mut domain.counters, Family::ForgeRow) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeEffectRefused(forge_client::api::Error::Busy),
        );
        return;
    };
    let token = to.into_token();
    if domain.forge_effecting.insert(entry, (ReplyTo::new(token), key)).is_err() {
        decide_call(
            domain,
            &env.limits,
            decision,
            ReplyTo::new(token),
            key,
            CallAnswer::ForgeEffectRefused(forge_client::api::Error::Busy),
        );
        return;
    }
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    domain.work.push(Work::Forge(forge::Event::Enqueue {
        entry: forge_client::Entry {
            number: entry,
            task: key.task,
            repository,
            effect,
            start: None,
            attempt: None,
            failures: 0,
        },
    }));
}

pub(super) fn read_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    repository: forge_client::api::Repository,
    read: forge_client::api::Read,
) {
    let Some(adopted) = domain.forge.repository(repository) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeRead(Box::new(Err(forge_client::api::Error::Missing))),
        );
    };
    let Some(context) = domain.tasks.delegation(key.task) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeRead(Box::new(Err(forge_client::api::Error::Forbidden))),
        );
    };
    let name = read_name(adopted, &read, env.limits.authority.segments);
    let allowed = match name {
        Some(name) if context.project == adopted.project && adopted.kinds.read => {
            let mut findings =
                Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority findings"));
            authority::check_call(
                &domain.config.authority,
                &authority::CallAsk {
                    project: context.project,
                    authority: authority_value(&context.authority),
                    family: authority::Tools(1),
                    call: authority::Call::Read(authority::Effect {
                        connector: domain.config.forge_connector,
                        kind: 1,
                        name,
                        state: [0; 32],
                        price: None,
                        access: effect_access(adopted, &forge::What::Repository, 1),
                        additional: Box::new([]),
                        guards: Box::new([]),
                    }),
                },
                &mut findings,
            ) == authority::Answer::Allow
        }
        Some(_) | None => false,
    };
    if !allowed {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeRead(Box::new(Err(forge_client::api::Error::Forbidden))),
        );
    }
    let Some(serial) = crate::fresh(&mut domain.counters, Family::Call) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeRead(Box::new(Err(forge_client::api::Error::Busy))),
        );
    };
    if serial >= 1_u64 << 61_u32 {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ForgeRead(Box::new(Err(forge_client::api::Error::Busy))),
        );
    }
    let owner = Token::new(serial | (1_u64 << 61_u32));
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.forge_reading.insert(owner, (to, key)) == Ok(None), "one fresh read correlation");
    domain.work.push(Work::Forge(forge::Event::Client(forge_client::Event::Read { owner, repository, read })));
}

#[expect(clippy::too_many_arguments, reason = "one typed task and connector subscription is routed together")]
pub(super) fn subscribe_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    topic: forge::Topic,
    own_change: Option<u64>,
    paths: Box<[Box<[u8]>]>,
) {
    let Some(number) = crate::fresh(&mut domain.counters, Family::Message) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::SubscriptionRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Busy }),
        );
        return;
    };
    let subscriber = forge::Subscriber { task: key.task, number, topic, own_change, paths };
    if !domain.forge.can_subscribe(&env.limits.forge, &subscriber)
        || watch_names(domain, &env.limits, &subscriber).is_none()
    {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::SubscriptionRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Subscription }),
        );
        return;
    }
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call room reserved");
    assert!(
        domain.routing_calls.insert(token, RoutedCall::Subscribe { key, subscription: number }) == Ok(None),
        "one routed call"
    );
    assert!(domain.forge_subscribing.insert(token, subscriber) == Ok(None), "one connector interest");
    domain.work.push(Work::Tasks(tasks::Event::SubscribeTopic {
        reply_to: ReplyTo::new(token),
        task: key.task,
        subscription: tasks::Subscription {
            number,
            kind: tasks::SubscriptionKind::Topic { connector: domain.config.forge_connector, topic: number },
        },
    }));
}

fn branch_parts(branch: &[u8], limit: u32) -> Option<Box<[Box<[u8]>]>> {
    let mut parts = List::with_capacity(limit);
    let mut start = 0;
    for at in 0..=branch.len() {
        if at == branch.len() || branch.get(at) == Some(&b'/') {
            let part = branch.get(start..at)?;
            if part.is_empty() {
                return None;
            }
            parts.push(Box::from(part)).ok()?;
            start = at.checked_add(1)?;
        }
    }
    Some(parts.into_boxed())
}

fn branch_what(branch: &[u8], limit: u32) -> Option<forge::What> {
    Some(forge::What::Branch(branch_parts(branch, limit)?))
}

#[expect(clippy::manual_map, reason = "the strict subset does not use closures")]
fn tree_branch(
    repository: &forge::Repository,
    root: u64,
    marker: u8,
    task: u64,
    attempt: Option<u64>,
    limit: u32,
) -> Option<Box<[u8]>> {
    let root_bytes = decimal(root);
    let task_bytes = decimal(task);
    let attempt_bytes = match attempt {
        Some(number) => Some(decimal(number)),
        None => None,
    };
    let mut needed =
        repository.prefix.len().checked_add(root_bytes.len())?.checked_add(2)?.checked_add(task_bytes.len())?;
    if let Some(number) = &attempt_bytes {
        needed = needed.checked_add(1)?.checked_add(number.len())?;
    }
    if needed > usize::try_from(limit).ok()? {
        return None;
    }
    let mut result = List::with_capacity(limit);
    append(&mut result, &repository.prefix);
    append(&mut result, &root_bytes);
    append(&mut result, b"/");
    append(&mut result, &[marker]);
    append(&mut result, &task_bytes);
    if let Some(attempt) = attempt_bytes {
        append(&mut result, b"-");
        append(&mut result, &attempt);
    }
    let branch = result.into_boxed();
    branch_what(&branch, limit)?;
    Some(branch)
}

pub(super) struct RunWorkspace {
    pub workspace: ForgeWorkspace,
    pub writes: Box<[authority::Write]>,
    pub names: Box<[forge::Name]>,
    pub own_holds: Box<[forge::Name]>,
    pub holders: Box<[u64]>,
}

/// Translate committed tracked-task facts into one bounded issue projection.
#[expect(
    clippy::disallowed_methods,
    clippy::wildcard_enum_match_arm,
    clippy::too_many_lines,
    reason = "projection converts already validated task text and selects current milestone kinds"
)]
pub(super) fn project_goal(domain: &mut Domain, env: &Env<Limits>, goal: &tasks::TaskRecord) {
    if goal.tracked.is_none() {
        return;
    }
    let Some(repository) = domain.forge.home(goal.project) else { return };
    let provider = repository.provider;
    let Some(name) = resource_name(repository, &forge::What::Repository, env.limits.authority.segments) else {
        domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
        return;
    };
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority findings"));
    let allowed = authority::check_effect(
        &domain.config.authority,
        &authority::EffectAsk {
            project: goal.project,
            authority: authority_value(&goal.authority),
            numbers: authority_numbers(goal.numbers),
            effect: authority::Effect {
                connector: domain.config.forge_connector,
                kind: 8,
                name,
                state: [0; 32],
                price: None,
                access: effect_access(repository, &forge::What::Repository, 8),
                additional: Box::new([]),
                guards: Box::new([]),
            },
            now: env.wall,
        },
        &[],
        &mut findings,
    );
    if allowed != authority::Answer::Allow {
        domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
        return;
    }
    let Some(text) = core::str::from_utf8(&goal.spec.words).ok() else {
        domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
        return;
    };
    let title = text.lines().next().unwrap_or(text);
    let mut plan = List::with_capacity(env.limits.forge.issue_policy.plan_items);
    for view in domain.tasks.view_tasks() {
        if view.requester != tasks::Party::Task(goal.number) {
            continue;
        }
        let Some(child) = domain.tasks.task(view.number) else { continue };
        let Some(words) = core::str::from_utf8(&child.spec.words).ok() else { continue };
        if plan
            .push(forge_issues::PlanItem {
                text: Box::from(words),
                done: match child.phase {
                    tasks::Phase::Ended(_) => true,
                    tasks::Phase::Waiting
                    | tasks::Phase::Active(_)
                    | tasks::Phase::Closing(_)
                    | tasks::Phase::Held { .. } => false,
                },
            })
            .is_err()
        {
            domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
            return;
        }
    }
    let mut milestones = List::with_capacity(env.limits.forge.issue_policy.milestones);
    for word in &goal.inbox {
        let key = match word.kind {
            tasks::MessageKind::Result(tasks::ResultKind::Change { .. }) => {
                Some(forge_issues::MilestoneKey::ChangeLanded(word.number))
            }
            tasks::MessageKind::Result(tasks::ResultKind::Failed) => {
                Some(forge_issues::MilestoneKey::ChangeHeld(word.number))
            }
            tasks::MessageKind::Result(tasks::ResultKind::Report) => {
                Some(forge_issues::MilestoneKey::Report(word.number))
            }
            _ => None,
        };
        if let Some(key) = key {
            let words = core::str::from_utf8(&word.words).unwrap_or("Task milestone");
            if milestones.push(forge_issues::Milestone { key, text: Box::from(words) }).is_err() {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
                return;
            }
        }
    }
    let Some(entry) = crate::fresh(&mut domain.counters, Family::ForgeRow) else { return };
    domain.work.push(Work::Forge(forge::Event::Project {
        entry,
        repository: provider,
        view: forge_issues::GoalView {
            goal: goal.number,
            repository: u64::from(provider.repository),
            title: Box::from(title),
            goal_text: Box::from(text),
            plan: plan.into_boxed(),
            milestones: milestones.into_boxed(),
            finished: match goal.phase {
                tasks::Phase::Ended(_) => Some(Box::from("Goal finished")),
                _ => None,
            },
        },
    }));
}

pub(super) fn subscribe_goal(domain: &mut Domain, env: &Env<Limits>, goal: u64, topic: forge::Topic) {
    if domain.tasks.task(goal).is_none() {
        return;
    }
    let Some(number) = crate::fresh(&mut domain.counters, Family::Message) else { return };
    let subscriber = forge::Subscriber { task: goal, number, topic, own_change: None, paths: Box::new([]) };
    if !domain.forge.can_subscribe(&env.limits.forge, &subscriber)
        || watch_names(domain, &env.limits, &subscriber).is_none()
    {
        return;
    }
    domain.work.push(Work::Tasks(tasks::Event::SubscribeTopic {
        reply_to: super::internal(u64::MAX),
        task: goal,
        subscription: tasks::Subscription {
            number,
            kind: tasks::SubscriptionKind::Topic { connector: domain.config.forge_connector, topic: number },
        },
    }));
    domain.work.push(Work::GoalSubscribe(subscriber));
}

pub(super) fn goal_subscribed(domain: &mut Domain, env: &Env<Limits>, subscriber: forge::Subscriber) {
    let Some(task) = domain.tasks.task(subscriber.task) else { return };
    let mut present = false;
    for subscription in &task.subscriptions {
        if subscription.number == subscriber.number {
            present = true;
            break;
        }
    }
    if !present {
        return;
    }
    let Some(names) = watch_names(domain, &env.limits, &subscriber) else { return };
    domain.work.push(Work::Forge(forge::Event::Subscribe { subscription: subscriber }));
    domain.work.push(Work::Forge(forge::Event::Names { task: task.number, resources: names }));
}

fn may_push(
    domain: &Domain,
    env: &Env<Limits>,
    context: &tasks::RunContext,
    repository: &forge::Repository,
    branch: &[u8],
) -> bool {
    let Some(what) = branch_what(branch, env.limits.forge.name_bytes) else { return false };
    let Some(name) = resource_name(repository, &what, env.limits.authority.segments) else { return false };
    let Some(bound) = authority::max_out(domain.config.authority.limits()) else { return false };
    let mut findings = Queue::with_capacity(bound);
    authority::check_run(
        &domain.config.authority,
        &authority::RunAsk {
            project: context.project,
            authority: authority_value(&context.authority),
            numbers: authority_numbers(context.numbers),
            budget: authority::left(authority_numbers(context.numbers))
                .min(domain.config.authority.rules().maximum_run_spend),
            wall: env.wall,
            accounts: Box::new([domain.accounts.usable(domain.config.account)]),
            writes: Box::new([authority::Write {
                effect: authority::Effect {
                    connector: domain.config.forge_connector,
                    kind: 2,
                    name,
                    state: [0; 32],
                    price: None,
                    access: effect_access(repository, &what, 2),
                    additional: Box::new([]),
                    guards: Box::new([]),
                },
                held: authority::Writer::Task,
            }]),
        },
        &mut findings,
    ) == authority::Answer::Allow
}

fn tracked_ancestor(domain: &Domain, requester: tasks::Party) -> Option<u64> {
    let mut next = requester;
    for _ in 0..=domain.limits.tasks.depth {
        let task = match next {
            tasks::Party::Task(task) => task,
            tasks::Party::Person(_) | tasks::Party::Deployment { .. } => return None,
        };
        let row = domain.tasks.task(task)?;
        if row.tracked.is_some() {
            return Some(task);
        }
        next = row.requester;
    }
    None
}

fn include_repository<'a>(selected: &mut List<&'a forge::Repository>, repository: &'a forge::Repository) -> Option<()> {
    for prior in selected.as_slice() {
        if prior.provider == repository.provider {
            return Some(());
        }
    }
    selected.push(repository).ok()
}

/// Resolve a run's repository tags and change ancestry before asking policy
/// or taking the connector writer slot.
#[expect(clippy::too_many_lines, reason = "one claim translates and checks all repository writes and holds")]
pub(super) fn run_workspace(
    domain: &Domain,
    env: &Env<Limits>,
    context: &tasks::RunContext,
    attempt: u64,
) -> Option<RunWorkspace> {
    let root = domain.tasks.root(context.task)?;
    let parent = match context.requester {
        tasks::Party::Task(task) => Some(task),
        tasks::Party::Person(_) | tasks::Party::Deployment { .. } => None,
    };
    let change = match parent {
        Some(task) => domain.forge.change(task),
        None => None,
    };
    let inherited = match change {
        Some(row) => match row.delegate {
            Some((child, kind)) if child == context.task => Some((row, kind)),
            Some(_) | None => None,
        },
        None => None,
    };
    let adopted = match inherited {
        Some((row, _)) => domain.forge.repository(row.repository),
        None => domain.forge.home(context.project),
    };
    let mut selected = List::with_capacity(env.limits.forge.resources_per_task);
    if let Some(repository) = adopted {
        include_repository(&mut selected, repository)?;
    }
    if inherited.is_none() {
        for parameter in &context.spec.parameters {
            match parameter {
                tasks::Parameter::Resource { connector, resource, .. }
                    if *connector == domain.config.forge_connector =>
                {
                    let repository = domain.forge.repository_tag(context.project, u32::try_from(*resource).ok()?)?;
                    if repository.project != context.project {
                        return None;
                    }
                    include_repository(&mut selected, repository)?;
                }
                tasks::Parameter::Resource { .. }
                | tasks::Parameter::Number { .. }
                | tasks::Parameter::Bytes { .. } => {}
            }
        }
        for tag in &context.saved {
            let Some(repository) = domain.forge.repository_tag(context.project, *tag) else {
                if adopted.is_some() {
                    return None;
                }
                continue;
            };
            include_repository(&mut selected, repository)?;
        }
    }
    let mut repositories = List::with_capacity(env.limits.forge.resources_per_task);
    let mut writes = List::with_capacity(env.limits.authority.writes);
    let mut names = List::with_capacity(env.limits.forge.resources_per_task);
    let mut own_holds = List::with_capacity(env.limits.forge.resources_per_task);
    let mut holders = List::with_capacity(env.limits.forge.resources_per_task);
    let mut key = context.task;
    for repository in &selected {
        let saved = context.saved.contains(&repository.provider.repository);
        let (start, mut push, holder) = if let Some((row, kind)) = inherited {
            key = row.task;
            match kind {
                forge_change::Delegate::Produce => {
                    let name = forge::Name {
                        forge: repository.provider.forge,
                        repository: repository.provider.repository,
                        what: branch_what(&row.branch, env.limits.forge.name_bytes)?,
                    };
                    let start = if domain.forge.branch_head(&name).is_some() {
                        ForgeStart::Branch(row.branch.clone())
                    } else {
                        ForgeStart::Base(row.base.clone())
                    };
                    (start, Some(row.branch.clone()), Some(row.task))
                }
                forge_change::Delegate::Repair(_) => {
                    (ForgeStart::Branch(row.branch.clone()), Some(row.branch.clone()), Some(row.task))
                }
                forge_change::Delegate::Resolve { base } => {
                    (ForgeStart::Merge { branch: row.branch.clone(), base }, Some(row.branch.clone()), Some(row.task))
                }
                forge_change::Delegate::Gate { .. } => (ForgeStart::Branch(row.branch.clone()), None, None),
            }
        } else {
            let branch = tree_branch(repository, root, b'r', context.task, Some(attempt), env.limits.forge.name_bytes)?;
            let start = if saved {
                ForgeStart::Saved(tree_branch(repository, root, b's', context.task, None, env.limits.forge.name_bytes)?)
            } else {
                ForgeStart::Base(repository.settings.default_branch.clone())
            };
            (start, Some(branch), None)
        };
        if inherited.is_none() {
            let permitted = match &push {
                Some(branch) => may_push(domain, env, context, repository, branch),
                None => false,
            };
            if !permitted {
                push = None;
            }
        }
        if let Some(branch) = &push {
            let name = forge::Name {
                forge: repository.provider.forge,
                repository: repository.provider.repository,
                what: branch_what(branch, env.limits.forge.name_bytes)?,
            };
            let held = match holder {
                Some(owner) => {
                    if domain.forge.hold(&name)?.task != owner {
                        return None;
                    }
                    holders.push(owner).ok()?;
                    authority::Writer::Ancestor
                }
                None => {
                    if let Some(existing) = domain.forge.hold(&name) {
                        if existing.task != context.task {
                            return None;
                        }
                    } else {
                        own_holds.push(name.clone()).ok()?;
                    }
                    authority::Writer::Task
                }
            };
            writes
                .push(authority::Write {
                    effect: authority::Effect {
                        connector: domain.config.forge_connector,
                        kind: 2,
                        name: resource_name(repository, &name.what, env.limits.authority.segments)?,
                        state: [0; 32],
                        price: None,
                        access: effect_access(repository, &name.what, 2),
                        additional: Box::new([]),
                        guards: Box::new([]),
                    },
                    held,
                })
                .ok()?;
            names.push(name).ok()?;
        }
        let provider = repository.provider;
        repositories
            .push(ForgeRepository {
                tag: provider.repository,
                provider,
                name: repository.name.clone(),
                host: repository.host.clone(),
                owner: repository.owner.clone(),
                start,
                push,
                identity: u32::from(provider.forge),
            })
            .ok()?;
    }
    Some(RunWorkspace {
        workspace: ForgeWorkspace { key: Box::from(key.to_be_bytes()), repositories: repositories.into_boxed() },
        writes: writes.into_boxed(),
        names: names.into_boxed(),
        own_holds: own_holds.into_boxed(),
        holders: holders.into_boxed(),
    })
}

#[expect(clippy::manual_map, reason = "the strict subset uses an explicit option match")]
fn pull_what(number: Option<u64>) -> Option<forge::What> {
    match number {
        Some(number) => Some(forge::What::Pull(number)),
        None => None,
    }
}

/// Project landing gates are procedure gates before they become effect requirements.
fn configured_gates(
    domain: &Domain,
    env: &Env<Limits>,
    repository: &forge::Repository,
    base: &[u8],
) -> Option<Box<[forge_change::Gate]>> {
    let what = branch_what(base, env.limits.forge.name_bytes)?;
    let name = resource_name(repository, &what, env.limits.authority.segments)?;
    let mut gates: List<forge_change::Gate> = List::with_capacity(env.limits.forge.change_policy.gates);
    let project = domain.config.landing.projects.get(&repository.project);
    let empty: &[people::LandingRule] = &[];
    let project_rules = match project {
        Some(rules) => rules.as_ref(),
        None => empty,
    };
    for rules in [&domain.config.landing.deployment[..], project_rules] {
        for rule in rules {
            if rule.connector != domain.config.forge_connector
                || rule.kind != 4
                || !authority::pattern_covers(&policy_translate::pattern_to_authority(rule.pattern.clone()), &name)
            {
                continue;
            }
            for gate in &rule.gates {
                let mut present = false;
                for prior in gates.as_slice() {
                    if prior.number == u64::from(gate.number) {
                        present = true;
                    }
                }
                if present {
                    continue;
                }
                let is_check = !repository.ci && repository.checks.contains(&gate.number);
                gates
                    .push(forge_change::Gate {
                        number: u64::from(gate.number),
                        kind: if is_check { forge_change::GateKind::Check } else { forge_change::GateKind::Agent },
                        blocking: is_check || gate.blocking,
                        freshness: if is_check {
                            forge_change::Freshness::Exact
                        } else {
                            match gate.freshness {
                                people::Freshness::Exact => forge_change::Freshness::Exact,
                                people::Freshness::Clean => forge_change::Freshness::Clean,
                            }
                        },
                        eager: false,
                    })
                    .ok()?;
            }
        }
    }
    if !repository.ci {
        for number in &repository.checks {
            let mut present = false;
            for prior in gates.as_slice() {
                if prior.number == u64::from(*number) {
                    present = true;
                }
            }
            if !present {
                gates
                    .push(forge_change::Gate {
                        number: u64::from(*number),
                        kind: forge_change::GateKind::Check,
                        blocking: true,
                        freshness: forge_change::Freshness::Exact,
                        eager: false,
                    })
                    .ok()?;
            }
        }
    }
    Some(gates.into_boxed())
}

fn landing_reviewers(
    domain: &Domain,
    env: &Env<Limits>,
    row: &forge::ChangeRow,
    evidence: &forge::ChangeEvidence,
) -> Option<Box<[forge::Reviewer]>> {
    let mut reviews = List::with_capacity(env.limits.forge.client.inbox);
    for review in &evidence.reviews {
        let mut superseded = false;
        for other in &evidence.reviews {
            if other.author == review.author
                && (other.at > review.at || (other.at == review.at && other.id > review.id))
            {
                superseded = true;
            }
        }
        if superseded {
            continue;
        }
        let Some((person, role)) = domain.people.role_for_identity(
            people::IdentityKey { provider: 0, subject: review.author.to_be_bytes().into() },
            domain.forge.repository(row.repository)?.project,
        ) else {
            continue;
        };
        reviews
            .push(forge::Reviewer {
                person,
                role: escalation::role_number(role),
                head: review.commit,
                verdict: review.verdict,
            })
            .ok()?;
    }
    Some(reviews.into_boxed())
}

fn check_change_effect(
    domain: &Domain,
    env: &Env<Limits>,
    task: u64,
    effect: forge_change::Effect,
    evidence: &forge::ChangeEvidence,
) -> authority::Answer {
    let Some(row) = domain.forge.change(task) else { return authority::Answer::Refuse };
    let Some(repository) = domain.forge.repository(row.repository) else { return authority::Answer::Refuse };
    let Some(context) = domain.tasks.delegation(task) else { return authority::Answer::Refuse };
    if repository.project != context.project || repository.role == forge::Role::Context {
        return authority::Answer::Refuse;
    }
    let pull_what = pull_what(row.pull);
    let (kind, what, state) = match effect {
        forge_change::Effect::Open => {
            (3, branch_what(&row.branch, env.limits.forge.name_bytes), evidence.head.unwrap_or([0; 32]))
        }
        forge_change::Effect::CreateBranch { head } => (9, branch_what(&row.branch, env.limits.forge.name_bytes), head),
        forge_change::Effect::Reopen | forge_change::Effect::Retarget => {
            (3, pull_what, evidence.head.unwrap_or([0; 32]))
        }
        forge_change::Effect::Update { head, .. } => (2, branch_what(&row.branch, env.limits.forge.name_bytes), head),
        forge_change::Effect::Merge { head, .. } => (4, branch_what(&row.base, env.limits.forge.name_bytes), head),
    };
    let Some(what) = what else { return authority::Answer::Refuse };
    let Some(name) = resource_name(repository, &what, env.limits.authority.segments) else {
        return authority::Answer::Refuse;
    };
    let mut effect = authority::Effect {
        connector: domain.config.forge_connector,
        kind,
        name,
        state,
        price: None,
        access: effect_access(repository, &what, kind),
        additional: Box::new([]),
        guards: Box::new([]),
    };
    let Some(judges) = authority::needed_judges(&domain.config.authority, context.project, &effect) else {
        return authority::Answer::Refuse;
    };
    let mut guards = List::with_capacity(env.limits.authority.facts);
    if kind == 4 {
        for judge in &judges {
            if judge.connector == domain.config.forge_connector
                && domain.forge.guards_landing(context.project, judge.requirement, judge.parameters)
                && guards.push(*judge).is_err()
            {
                return authority::Answer::Refuse;
            }
        }
    }
    effect.guards = guards.into_boxed();
    let Some(reviewers) = landing_reviewers(domain, env, row, evidence) else {
        return authority::Answer::Refuse;
    };
    let mut given = List::with_capacity(env.limits.authority.facts);
    for judge in &judges {
        if judge.connector != domain.config.forge_connector {
            continue;
        }
        let Some(verdict) = domain.forge.judge_landing(
            context.project,
            judge.requirement,
            judge.parameters,
            task,
            state,
            evidence,
            &reviewers,
        ) else {
            continue;
        };
        let verdict = match verdict {
            forge::JudgeVerdict::Met => authority::Verdict::Met,
            forge::JudgeVerdict::Wait => authority::Verdict::Wait,
            forge::JudgeVerdict::Refuse => authority::Verdict::Refuse,
        };
        if given.push(authority::Given { judge: *judge, verdict, at: env.wall, state }).is_err() {
            return authority::Answer::Refuse;
        }
    }
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority findings"));
    authority::check_effect(
        &domain.config.authority,
        &authority::EffectAsk {
            project: context.project,
            authority: authority_value(&context.authority),
            numbers: authority_numbers(context.numbers),
            effect,
            now: env.wall,
        },
        given.as_slice(),
        &mut findings,
    )
}

/// Register the connector's procedure beside the task's first due step.
/// Parameter 1 names a project-unique repository tag, 2 the base,
/// 3 an optional already-pushed branch, 4 the pull body, and 5 priority.
#[expect(clippy::too_many_lines, reason = "one procedure registration and fresh-step route")]
pub(super) fn start_change(domain: &mut Domain, env: &Env<Limits>, context: &tasks::RunContext) -> bool {
    let task = context.task;
    let mut provider = None;
    let mut base = None;
    let mut branch = None;
    let mut body = None;
    let mut priority = 0_i32;
    let mut base_repair = false;
    for parameter in &context.spec.parameters {
        match parameter {
            tasks::Parameter::Resource { name: 1, connector, resource } => {
                if *connector != domain.config.forge_connector {
                    return false;
                }
                let Ok(tag) = u32::try_from(*resource) else { return false };
                let Some(repository) = domain.forge.repository_tag(context.project, tag) else { return false };
                provider = Some(repository.provider);
            }
            tasks::Parameter::Bytes { name: 2, value } => base = Some(value.clone()),
            tasks::Parameter::Bytes { name: 3, value } => branch = Some(value.clone()),
            tasks::Parameter::Bytes { name: 4, value } => body = Some(value.clone()),
            tasks::Parameter::Number { name: 5, value } => {
                let Ok(number) = i32::try_from(*value) else { return false };
                priority = number;
            }
            tasks::Parameter::Number { name: 9, value: 1 } => base_repair = true,
            tasks::Parameter::Number { .. } | tasks::Parameter::Bytes { .. } | tasks::Parameter::Resource { .. } => {
                return false;
            }
        }
    }
    let Some(provider) = provider else { return false };
    if base_repair {
        match context.requester {
            tasks::Party::Deployment { .. } => {}
            tasks::Party::Task(_) | tasks::Party::Person(_) => return false,
        }
    }
    let Some(adopted) = domain.forge.repository(provider) else { return false };
    if adopted.project != context.project || adopted.role == forge::Role::Context || !adopted.kinds.open {
        return false;
    }
    let base = match base {
        Some(base) => base,
        None => adopted.settings.default_branch.clone(),
    };
    let branch = match branch {
        Some(branch) => branch,
        None => {
            let Some(root) = domain.tasks.root(task) else { return false };
            let root = decimal(root);
            let task_bytes = decimal(task);
            let Some(length) = adopted.prefix.len().checked_add(root.len()) else { return false };
            let Some(length) = length.checked_add(2) else { return false };
            let Some(length) = length.checked_add(task_bytes.len()) else { return false };
            if length > usize::try_from(env.limits.forge.name_bytes).expect("u32 fits usize") {
                return false;
            }
            let mut out = List::with_capacity(env.limits.forge.name_bytes);
            append(&mut out, &adopted.prefix);
            append(&mut out, &root);
            append(&mut out, b"/c");
            append(&mut out, &task_bytes);
            out.into_boxed()
        }
    };
    let Some(parts) = branch_parts(&branch, env.limits.forge.name_bytes) else { return false };
    if base.is_empty() || branch.is_empty() || context.spec.words.is_empty() {
        return false;
    }
    let Some(base_parts) = branch_parts(&base, env.limits.forge.name_bytes) else { return false };
    let Some(gates) = configured_gates(domain, env, adopted, &base) else { return false };
    if domain.forge.change(task).is_none() {
        domain.work.push(Work::Forge(forge::Event::Change {
            row: forge::ChangeRow {
                task,
                repository: provider,
                branch: branch.clone(),
                base: base.clone(),
                title: context.spec.words.clone(),
                body: match body {
                    Some(body) => body,
                    None => Box::new([]),
                },
                priority,
                change: forge_change::Change {
                    task,
                    state: forge_change::State::Producing { requested: false },
                    gates,
                    clean: Box::new([]),
                    repairs: 0,
                    resolutions: 0,
                    updates: 0,
                    since: env.wall,
                    ready_since: None,
                    owns_turn: false,
                    last_head: None,
                },
                pull: None,
                pending: None,
                effect: forge_change::EffectResult::None,
                delegate: None,
                delegate_status: forge_change::Status::Unknown,
                verdicts: Box::new([]),
                gate_remarks: Box::new([]),
                drift: None,
                base_repair,
                queue_repair: None,
                queue_repairs: 0,
            },
        }));
        let branch_name =
            forge::Name { forge: provider.forge, repository: provider.repository, what: forge::What::Branch(parts) };
        let base_name = forge::Name {
            forge: provider.forge,
            repository: provider.repository,
            what: forge::What::Branch(base_parts),
        };
        domain
            .work
            .push(Work::Forge(forge::Event::Names { task, resources: Box::new([branch_name.clone(), base_name]) }));
        domain.work.push(Work::Forge(forge::Event::Hold { task, resource: branch_name, from: None }));
        if let Some(goal) = tracked_ancestor(domain, context.requester) {
            subscribe_goal(domain, env, goal, forge::Topic::Landings { repository: provider, branch: base.clone() });
        }
    } else if let Some((child, _)) = domain.forge.change(task).expect("registered change").delegate {
        for word in &context.inbox {
            if word.from != tasks::Party::Task(child) {
                continue;
            }
            let status = match word.kind {
                tasks::MessageKind::Result(
                    tasks::ResultKind::Report
                    | tasks::ResultKind::Change { .. }
                    | tasks::ResultKind::Verdict { code: 1 },
                ) => Some(forge_change::Status::Passed),
                tasks::MessageKind::Result(
                    tasks::ResultKind::Failed | tasks::ResultKind::Cancelled | tasks::ResultKind::Verdict { .. },
                ) => Some(forge_change::Status::Failed),
                tasks::MessageKind::Escalation { .. }
                | tasks::MessageKind::Proposal { .. }
                | tasks::MessageKind::ProposalDecision { .. }
                | tasks::MessageKind::Words
                | tasks::MessageKind::Amendment { .. }
                | tasks::MessageKind::Question
                | tasks::MessageKind::Answer { .. }
                | tasks::MessageKind::Notice { .. }
                | tasks::MessageKind::Timer { .. }
                | tasks::MessageKind::News { .. } => None,
            };
            if let Some(status) = status {
                domain.work.push(Work::Forge(forge::Event::DelegateResult {
                    task,
                    child,
                    status,
                    words: word.words.clone(),
                }));
                break;
            }
        }
    }
    let Some(entry) = crate::fresh(&mut domain.counters, Family::ForgeRow) else { return false };
    domain.work.push(Work::Forge(forge::Event::StepChange {
        task,
        entry,
        heard: forge_change::Heard {
            delegate: forge_change::Status::Unknown,
            effect: forge_change::EffectResult::None,
            released: false,
            cancelled: false,
        },
        gates: Box::new([]),
        queue_repair_active: match domain.forge.queue_repair(provider, &base) {
            Some((_, repair)) => domain.tasks.task(repair).is_some(),
            None => false,
        },
    }));
    true
}

fn child_authority(context: &tasks::DelegationContext) -> Option<tasks::Authority> {
    let mut authority = context.authority.clone();
    let available = context
        .numbers
        .budget
        .checked_sub(context.numbers.spent)?
        .checked_sub(context.numbers.spent_below)?
        .checked_sub(context.numbers.reserved)?;
    authority.budget.spend = authority.budget.spend.min(available);
    if authority.budget.spend == 0 {
        return None;
    }
    authority.delegation.kinds = Box::new([]);
    authority.delegation.tasks = 0;
    authority.delegation.depth = 0;
    Some(authority)
}

fn change_delegate(
    domain: &Domain,
    env: &Env<Limits>,
    task: u64,
    kind: forge_change::Delegate,
) -> Option<super::Delegate> {
    let row = domain.forge.change(task)?;
    let context = domain.tasks.delegation(task)?;
    let authority = child_authority(&context)?;
    let mut parameters = List::with_capacity(env.limits.tasks.parameters);
    parameters
        .push(tasks::Parameter::Resource {
            name: 1,
            connector: domain.config.forge_connector,
            resource: u64::from(row.repository.repository),
        })
        .ok()?;
    parameters.push(tasks::Parameter::Bytes { name: 2, value: row.base.clone() }).ok()?;
    parameters.push(tasks::Parameter::Bytes { name: 3, value: row.branch.clone() }).ok()?;
    let (executor, words, contract) = match kind {
        forge_change::Delegate::Produce => (
            tasks::Executor::Agent { charter: domain.config.charter },
            row.title.clone(),
            tasks::Contract::Change {
                connector: domain.config.forge_connector,
                kind: 2,
                words: env.limits.tasks.result_bytes,
            },
        ),
        forge_change::Delegate::Repair(why) => {
            let words: &[u8] = match why {
                forge_change::Repair::Ci => b"Repair the failed CI on the change branch",
                forge_change::Repair::Semantic => b"Repair the base update's semantic conflict",
                forge_change::Repair::Gate(_) => b"Repair the requested gate changes",
            };
            (
                tasks::Executor::Agent { charter: domain.config.charter },
                Box::from(words),
                tasks::Contract::Change {
                    connector: domain.config.forge_connector,
                    kind: 2,
                    words: env.limits.tasks.result_bytes,
                },
            )
        }
        forge_change::Delegate::Resolve { base } => {
            parameters.push(tasks::Parameter::Bytes { name: 6, value: Box::from(&base[..]) }).ok()?;
            (
                tasks::Executor::Agent { charter: domain.config.charter },
                Box::from(&b"Resolve the merge conflict and push the merge commit"[..]),
                tasks::Contract::Change {
                    connector: domain.config.forge_connector,
                    kind: 2,
                    words: env.limits.tasks.result_bytes,
                },
            )
        }
        forge_change::Delegate::Gate { number, head } => {
            parameters.push(tasks::Parameter::Number { name: 7, value: number }).ok()?;
            parameters.push(tasks::Parameter::Bytes { name: 8, value: Box::from(&head[..]) }).ok()?;
            let mut found = None;
            for gate in &row.change.gates {
                if gate.number == number {
                    found = Some(*gate);
                }
            }
            let gate = found?;
            let executor = match gate.kind {
                forge_change::GateKind::Agent | forge_change::GateKind::Check => {
                    tasks::Executor::Agent { charter: domain.config.charter }
                }
                forge_change::GateKind::Person => tasks::Executor::Person(tasks::PersonAddress::Role(1)),
            };
            (
                executor,
                Box::from(&b"Review this change at the named head"[..]),
                tasks::Contract::Verdict {
                    choices: Box::new([
                        tasks::Verdict { code: 1, words: env.limits.tasks.result_bytes },
                        tasks::Verdict { code: 2, words: env.limits.tasks.result_bytes },
                    ]),
                },
            )
        }
    };
    Some(super::Delegate {
        executor,
        spec: tasks::Spec { words, parameters: parameters.into_boxed(), inputs: Box::new([]) },
        contract,
        authority,
        symbolic_grants: Box::new([]),
        dependencies: Box::new([]),
        wake: tasks::WakePolicy::DEFAULT,
    })
}

/// Admit one project-funded repair of a broken landing branch. The owner row
/// records its identity in the same root decision as the new task.
fn start_queue_repair(domain: &mut Domain, env: &Env<Limits>, task: u64) -> bool {
    let Some(row) = domain.forge.change(task) else { return false };
    if let Some((_, repair)) = domain.forge.queue_repair(row.repository, &row.base)
        && domain.tasks.task(repair).is_some()
    {
        return true;
    }
    if domain.forge.queue_repairs(row.repository, &row.base) >= env.limits.forge.change_policy.repairs {
        return false;
    }
    let Some(repository) = domain.forge.repository(row.repository) else { return false };
    let project = repository.project;
    let Some(policy) = domain.config.authority.policy(project) else { return false };
    let mut authority = policy.ceiling.clone();
    let period = domain.config.period;
    let available = match domain.tasks.funding(tasks::Funder::Period { project, period }) {
        Some(funding) => funding
            .numbers
            .budget
            .saturating_sub(funding.numbers.spent)
            .saturating_sub(funding.numbers.spent_below)
            .saturating_sub(funding.numbers.reserved),
        None => domain.config.period_budget,
    };
    authority.budget.spend =
        authority.budget.spend.min(available).min(domain.config.authority.rules().maximum_run_spend);
    if authority.budget.spend == 0 {
        return false;
    }
    let provider = row.repository;
    let base = row.base.clone();
    let Some(repair) = crate::fresh(&mut domain.counters, Family::Task) else { return false };
    if domain.tasks.funding(tasks::Funder::Period { project, period }).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
            reply_to: super::internal(u64::MAX - 2),
            project,
            period,
            budget: domain.config.period_budget,
        }));
    }
    domain.work.push(Work::Forge(forge::Event::QueueRepairStarted { owner: task, repair }));
    domain.work.push(Work::Tasks(tasks::Event::Make {
        reply_to: super::internal(u64::MAX - 3),
        creator: tasks::Party::Deployment { project },
        batch: Box::new([tasks::New {
            number: repair,
            project,
            executor: tasks::Executor::Procedure { connector: domain.config.forge_connector, code: 2 },
            spec: tasks::Spec {
                words: Box::from(&b"Repair landing branch CI"[..]),
                parameters: Box::new([
                    tasks::Parameter::Resource {
                        name: 1,
                        connector: domain.config.forge_connector,
                        resource: u64::from(provider.repository),
                    },
                    tasks::Parameter::Bytes { name: 2, value: base },
                    tasks::Parameter::Number { name: 5, value: u64::try_from(i32::MAX).expect("positive priority") },
                    tasks::Parameter::Number { name: 9, value: 1 },
                ]),
                inputs: Box::new([]),
            },
            contract: tasks::Contract::Change {
                connector: domain.config.forge_connector,
                kind: 1,
                words: env.limits.tasks.result_bytes,
            },
            numbers: tasks::Numbers { budget: authority.budget.spend, spent: 0, spent_below: 0, reserved: 0 },
            authority: super::task_authority(&authority),
            funder: tasks::Funder::Period { project, period },
            dependencies: Box::new([]),
            wake: tasks::WakePolicy::DEFAULT,
            recurring: None,
            tracked: None,
        }]),
    }));
    true
}

#[expect(clippy::too_many_lines, reason = "one procedure decision translates the complete change vocabulary")]
fn change_decision(domain: &mut Domain, env: &Env<Limits>, task: u64, choice: forge_change::Decision) {
    let Some((connector, code, step)) = domain.tasks.procedure_due(task) else { return };
    if connector != domain.config.forge_connector || code != 2 {
        return;
    }
    domain.forge_change_due.remove(&task);
    let mut again = false;
    let action = match choice {
        forge_change::Decision::Finish { merge } => {
            let pull = match domain.forge.change(task) {
                Some(row) => row.pull,
                None => None,
            };
            let Some(pull) = pull else {
                drop(procedure_step(
                    domain,
                    env,
                    task,
                    step,
                    connector,
                    code,
                    ProcedureAction::Hold(tasks::Hold::Effects),
                ));
                return;
            };
            ProcedureAction::Result(tasks::TaskResult::Change {
                connector: domain.config.forge_connector,
                kind: 1,
                resource: pull,
                words: Box::from(&merge[..]),
            })
        }
        forge_change::Decision::Hold(why) => {
            let why = match why {
                forge_change::Hold::BranchMoved
                | forge_change::Hold::BranchMissing
                | forge_change::Hold::Retargeted
                | forge_change::Hold::PullClosed => tasks::Hold::Drift,
                forge_change::Hold::Stalled
                | forge_change::Hold::Repairs
                | forge_change::Hold::Resolutions
                | forge_change::Hold::Updates
                | forge_change::Hold::Failed
                | forge_change::Hold::Rejected => tasks::Hold::Effects,
            };
            ProcedureAction::Hold(why)
        }
        forge_change::Decision::Ready => {
            again = true;
            ProcedureAction::Wait
        }
        forge_change::Decision::QueueRepair => {
            if start_queue_repair(domain, env, task) {
                let until = skein_lib::Wall::from_nanos(
                    env.wall.as_nanos().saturating_add(env.limits.forge.queue_window.as_nanos()),
                );
                domain.forge_change_due.insert(task, until).expect("one timer per change");
                ProcedureAction::Wait
            } else {
                ProcedureAction::Hold(tasks::Hold::Effects)
            }
        }
        forge_change::Decision::Wait { until } => {
            let until = match until {
                Some(until) => until,
                None => skein_lib::Wall::from_nanos(
                    env.wall.as_nanos().saturating_add(env.limits.forge.queue_window.as_nanos()),
                ),
            };
            domain.forge_change_due.insert(task, until).expect("one timer per change");
            ProcedureAction::Wait
        }
        forge_change::Decision::None | forge_change::Decision::Effect(_) | forge_change::Decision::Cancel => {
            ProcedureAction::Wait
        }
        forge_change::Decision::Delegate(kind) => {
            let Some(delegate) = change_delegate(domain, env, task, kind) else {
                drop(procedure_step(
                    domain,
                    env,
                    task,
                    step,
                    connector,
                    code,
                    ProcedureAction::Hold(tasks::Hold::Effects),
                ));
                return;
            };
            let delegated = procedure_step(
                domain,
                env,
                task,
                step,
                connector,
                code,
                ProcedureAction::Delegate(Box::new([delegate])),
            );
            if let Some(numbers) = delegated
                && let Some(child) = numbers.first()
            {
                domain.work.push(Work::Forge(forge::Event::Delegated { task, child: *child, kind }));
            }
            return;
        }
    };
    drop(procedure_step(domain, env, task, step, connector, code, action));
    if again {
        domain.work.push(Work::Tasks(tasks::Event::WakeProcedure { task }));
    }
}

#[expect(clippy::too_many_lines, reason = "one exhaustive connector output translation")]
pub(super) fn outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    out: &mut Queue<forge::Request>,
) {
    for _ in 0..out.len() {
        let request = out.pop().expect("connector output count");
        match request {
            forge::Request::BriefClient { event } => domain.work.push(Work::Forge(forge::Event::Client(event))),
            forge::Request::BriefReady { .. } => unreachable!("the root uses held forge sections"),
            forge::Request::BriefSized { section, size } => {
                let Some(row) = domain.brief_connectors.get(Id::from_token(section)) else { continue };
                let brief = Token::new(row.task);
                let event = match size {
                    Some(size) if row.cutting => brief::GatherEvent::Cut { brief, section, size },
                    Some(size) => brief::GatherEvent::Ready { brief, section, size },
                    None => brief::GatherEvent::Missing { brief, section },
                };
                domain.work.push(Work::Brief(event));
            }
            forge::Request::BriefTaken { .. } => unreachable!("the root takes completed sections directly"),
            forge::Request::Save { record } => {
                let key = forge::stored_key(&record);
                let first = !domain.forge_keys.contains_key(&key);
                let number = match domain.forge_keys.get(&key) {
                    Some(number) => *number,
                    None => {
                        let number = crate::fresh(&mut domain.counters, Family::ForgeRow).expect("admitted forge row");
                        domain.forge_keys.insert(key, number).expect("bounded connector rows");
                        number
                    }
                };
                let entry = match &record {
                    forge::Stored::Entry(row) => Some((row.number, row.task)),
                    forge::Stored::Repository(_)
                    | forge::Stored::Hold(_)
                    | forge::Stored::Names { .. }
                    | forge::Stored::Subscription(_)
                    | forge::Stored::BranchHead(_)
                    | forge::Stored::PullState(_)
                    | forge::Stored::Ci(_)
                    | forge::Stored::Landed { .. }
                    | forge::Stored::Client(_)
                    | forge::Stored::Change(_)
                    | forge::Stored::Issue(_)
                    | forge::Stored::Release(_) => None,
                };
                save(decision, &env.limits, Write::Save(Record::Forge { id: number, row: Box::new(record) }));
                if let Some((entry, task)) = entry {
                    if let Some((to, key)) = domain.forge_effecting.remove(&entry) {
                        decide_call(
                            domain,
                            &env.limits,
                            decision,
                            to,
                            key,
                            CallAnswer::ForgeEffect { entry, outcome: None },
                        );
                    }
                    let change_pending = match domain.forge.change(task) {
                        Some(row) => row.pending == Some(entry),
                        None => false,
                    };
                    if first && !change_pending {
                        emit(decision, &env.limits, Delivery::ForgeCommitted { entry });
                    }
                }
            }
            forge::Request::Erase { key } => {
                if let Some(number) = domain.forge_keys.remove(&key) {
                    save(decision, &env.limits, Write::Erase(Key::Forge(number)));
                }
            }
            forge::Request::Call { call, repository, op } => {
                emit(decision, &env.limits, Delivery::ForgeCall { call, repository, op });
            }
            forge::Request::Read { owner, result } => {
                if let Some((to, key)) = domain.forge_reading.remove(&owner) {
                    decide_call(domain, &env.limits, decision, to, key, CallAnswer::ForgeRead(Box::new(result)));
                }
            }
            forge::Request::Adopted { reply_to, result } => {
                let previous = domain.adoption_restore.remove(&reply_to).expect("one admitted adoption result");
                match result {
                    Ok(adopted) => {
                        let mut seeds = List::with_capacity(env.limits.forge.collaborators);
                        let mut exhausted = false;
                        for collaborator in &adopted.collaborators {
                            let role = match collaborator.permission {
                                forge_client::api::Permission::Admin => Some(people::Role::Maintainer),
                                forge_client::api::Permission::Write => Some(people::Role::Member),
                                forge_client::api::Permission::Read => Some(people::Role::Observer),
                                forge_client::api::Permission::None => None,
                            };
                            if let Some(role) = role {
                                match crate::fresh(&mut domain.counters, Family::Person) {
                                    Some(candidate) => {
                                        seeds
                                            .push(people::Seed {
                                                identity: people::IdentityKey {
                                                    provider: 0,
                                                    subject: collaborator.user.to_be_bytes().into(),
                                                },
                                                candidate,
                                                role,
                                            })
                                            .expect("bounded collaborators");
                                    }
                                    None => exhausted = true,
                                }
                            }
                        }
                        if exhausted
                            || !domain.people.can_seed(&env.limits.people, adopted.repository.project, seeds.as_slice())
                        {
                            domain.work.push(Work::Forge(forge::Event::ForgetAdoption {
                                repository: adopted.repository.provider,
                                restore: previous,
                            }));
                            domain.work.push(Work::People(people::Event::Decided {
                                request: reply_to,
                                outcome: people::Outcome::Refused(people::Refusal::Busy),
                            }));
                        } else {
                            domain.work.push(Work::People(people::Event::Seed {
                                project: adopted.repository.project,
                                collaborators: seeds.into_boxed(),
                            }));
                            domain.work.push(Work::People(people::Event::Decided {
                                request: reply_to,
                                outcome: people::Outcome::RepositoryAdopted {
                                    project: adopted.repository.project,
                                    forge: adopted.repository.provider.forge,
                                    repository: adopted.repository.provider.repository,
                                },
                            }));
                        }
                    }
                    Err(error) => {
                        let refusal = match error {
                            forge_client::api::Error::Busy
                            | forge_client::api::Error::Unavailable
                            | forge_client::api::Error::Timeout
                            | forge_client::api::Error::RateLimited { .. } => people::Refusal::Busy,
                            forge_client::api::Error::TooLarge | forge_client::api::Error::Full => {
                                people::Refusal::Limit
                            }
                            forge_client::api::Error::Forbidden
                            | forge_client::api::Error::Protected
                            | forge_client::api::Error::Refused => people::Refusal::Authority,
                            forge_client::api::Error::Missing
                            | forge_client::api::Error::MissingJob
                            | forge_client::api::Error::Empty
                            | forge_client::api::Error::Exists
                            | forge_client::api::Error::NothingToMerge
                            | forge_client::api::Error::Closed
                            | forge_client::api::Error::Stale
                            | forge_client::api::Error::Conflict
                            | forge_client::api::Error::InvalidAnswer => people::Refusal::Unknown,
                        };
                        domain.work.push(Work::People(people::Event::Decided {
                            request: reply_to,
                            outcome: people::Outcome::Refused(refusal),
                        }));
                    }
                }
            }
            forge::Request::News { task, subscription, news, class, .. } => {
                let class = match class {
                    forge::Class::Wakes => tasks::NewsClass::Wakes,
                    forge::Class::Kept => tasks::NewsClass::Kept,
                    forge::Class::Dropped => continue,
                };
                let number = crate::fresh(&mut domain.counters, Family::Message).expect("news number admitted");
                domain.work.push(Work::Tasks(tasks::Event::Notice {
                    task,
                    word: tasks::Word {
                        number,
                        from: tasks::Party::Task(task),
                        kind: tasks::MessageKind::News { subscription, class },
                        words: news_words(&news, env.limits.tasks.message_bytes),
                        at: env.wall,
                        hits: 1,
                        eligible: false,
                    },
                }));
            }
            forge::Request::Taken { task, .. } | forge::Request::Refused { task } => {
                if domain.claiming.remove(&task).is_some() {
                    drop(domain.assignments.remove(&task));
                    drop(domain.proofs.remove(&task));
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                }
                let mut failed = None;
                for (&entry, (_, key)) in &domain.forge_effecting {
                    if key.task == task {
                        failed = Some(entry);
                        break;
                    }
                }
                if let Some(entry) = failed {
                    let (to, key) = domain.forge_effecting.remove(&entry).expect("effect awaiting connector admission");
                    decide_call(
                        domain,
                        &env.limits,
                        decision,
                        to,
                        key,
                        CallAnswer::ForgeEffectRefused(forge_client::api::Error::Busy),
                    );
                }
                domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
            }
            forge::Request::Outcome { entry, task, outcome } => {
                if outcome != forge_client::Outcome::Uncertain {
                    let mut named = None;
                    for (&key, answer) in &domain.calls {
                        match answer {
                            CallAnswer::ForgeEffect { entry: number, .. } if *number == entry => {
                                named = Some(key);
                                break;
                            }
                            CallAnswer::ForgeEffect { .. }
                            | CallAnswer::ForgeEffectRefused(_)
                            | CallAnswer::ForgeEffectDenied { .. }
                            | CallAnswer::ForgeRead(_)
                            | CallAnswer::EscalationDecided { .. }
                            | CallAnswer::EscalationRefused(_)
                            | CallAnswer::Proposed { .. }
                            | CallAnswer::ProposalDecided { .. }
                            | CallAnswer::ProposalRefused(_)
                            | CallAnswer::Controlled
                            | CallAnswer::ControlRefused(_)
                            | CallAnswer::ControlDenied { .. }
                            | CallAnswer::Sent { .. }
                            | CallAnswer::Introduced
                            | CallAnswer::MessageRefused(_)
                            | CallAnswer::Subscribed { .. }
                            | CallAnswer::Unsubscribed
                            | CallAnswer::SubscriptionRefused(_)
                            | CallAnswer::Delegated(_)
                            | CallAnswer::DelegationDenied { .. }
                            | CallAnswer::DelegationRefused(_)
                            | CallAnswer::Unavailable => {}
                        }
                    }
                    if let Some(key) = named {
                        let answer = CallAnswer::ForgeEffect { entry, outcome: Some(outcome) };
                        domain.calls.insert(key, answer.clone()).expect("replaces retained named effect");
                        save(decision, &env.limits, Write::Save(Record::Call(crate::CallRecord { key, answer })));
                    }
                }
                match outcome {
                    forge_client::Outcome::Failed(_)
                    | forge_client::Outcome::Raced { .. }
                    | forge_client::Outcome::Withdrawn
                        if domain.forge.change(task).is_none() =>
                    {
                        domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                    }
                    forge_client::Outcome::Failed(_)
                    | forge_client::Outcome::Raced { .. }
                    | forge_client::Outcome::Withdrawn
                    | forge_client::Outcome::Made { .. }
                    | forge_client::Outcome::Uncertain => {}
                }
                if outcome != forge_client::Outcome::Uncertain && domain.forge.change(task).is_some() {
                    domain.work.push(Work::Tasks(tasks::Event::WakeProcedure { task }));
                }
            }
            forge::Request::ContinueRelease { task } => {
                if let Some(entry) = crate::fresh(&mut domain.counters, Family::ForgeRow) {
                    domain.work.push(Work::Forge(forge::Event::ContinueRelease { task, entry }));
                } else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                }
            }
            forge::Request::Released { task } => {
                domain.work.push(Work::Tasks(tasks::Event::Settled { task }));
            }
            forge::Request::ReleaseFailed { task } => {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
            }
            forge::Request::ProjectionFailed { goal } => {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal, why: tasks::Hold::Effects }));
            }
            forge::Request::Drift { task, .. } => {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Drift }));
            }
            forge::Request::ChangeDecision { task, decision: choice, entry, evidence } => {
                change_decision(domain, env, task, choice);
                match choice {
                    forge_change::Decision::Effect(effect) => {
                        if let Some(entry) = entry
                            && let Some(evidence) = evidence
                        {
                            let answer = check_change_effect(domain, env, task, effect, &evidence);
                            match answer {
                                authority::Answer::Allow => {
                                    emit(decision, &env.limits, Delivery::ForgeCommitted { entry });
                                }
                                authority::Answer::Wait | authority::Answer::Propose | authority::Answer::Refuse => {
                                    domain.work.push(Work::Forge(forge::Event::VetoChange {
                                        task,
                                        entry,
                                        prior: evidence.prior,
                                    }));
                                    if answer == authority::Answer::Wait {
                                        let when = skein_lib::Wall::from_nanos(
                                            env.wall
                                                .as_nanos()
                                                .saturating_add(env.limits.forge.queue_window.as_nanos()),
                                        );
                                        domain.forge_change_due.insert(task, when).expect("one timer per change");
                                    } else {
                                        domain
                                            .work
                                            .push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                                    }
                                }
                            }
                        }
                    }
                    forge_change::Decision::None
                    | forge_change::Decision::Wait { .. }
                    | forge_change::Decision::Delegate(_)
                    | forge_change::Decision::Ready
                    | forge_change::Decision::QueueRepair
                    | forge_change::Decision::Finish { .. }
                    | forge_change::Decision::Cancel
                    | forge_change::Decision::Hold(_) => {}
                }
            }
            forge::Request::ProjectAfter { goal, when } => {
                if let Some(issue) = domain.forge.issue(goal)
                    && let Some(number) = issue.number
                {
                    subscribe_goal(
                        domain,
                        env,
                        goal,
                        forge::Topic::Participation { repository: issue.repository, number },
                    );
                }
                if let Some(when) = when {
                    domain.forge_projection_due.insert(goal, when).expect("projection count bounded by issue rows");
                } else if match domain.forge.issue(goal) {
                    Some(row) => row.pending.is_none(),
                    None => false,
                } && let Some(entry) = crate::fresh(&mut domain.counters, Family::ForgeRow)
                {
                    domain.work.push(Work::Forge(forge::Event::ProjectDesired { entry, goal }));
                }
            }
        }
    }
}
