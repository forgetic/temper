//! Root translation for the forge connector. Its store rows and released API
//! calls cross one root decision; the connector never owns the store.
use super::{
    CallAnswer, CallKey, Decision, Delivery, Domain, Env, Family, ForgeRepository, ForgeStart, ForgeWorkspace,
    Freshness, Id, Key, LandingRule, Limits, List, ProcedureAction, Queue, Record, ReplyTo, Token, Work, Write,
    authority, decide_call, emit, escalation, forge, forge_change, forge_client, forge_issues, people, save, tasks,
};
use alloc::boxed::Box;
use jig_core::connector::{EffectDescription, EffectForm, Recovery};
use jig_core_brief as brief;

/// Translate the worker's connector-specific saved tags at the root boundary.
pub(super) fn saved_resources(connector: u16, tags: &[u32]) -> Option<Box<[tasks::SavedResource]>> {
    let mut resources = List::with_capacity(u32::try_from(tags.len()).ok()?);
    for tag in tags {
        resources.push(tasks::SavedResource { connector, path: Box::new([Box::from(tag.to_be_bytes())]) }).ok()?;
    }
    Some(resources.into_boxed())
}

/// Return the forge's resource tags from task-owned generic saved names.
pub(super) fn saved_tags(resources: &[tasks::SavedResource], connector: u16) -> Option<Box<[u32]>> {
    let mut tags = List::with_capacity(u32::try_from(resources.len()).ok()?);
    for resource in resources {
        if resource.connector != connector {
            continue;
        }
        let [repository] = resource.path.as_ref() else { return None };
        let [one, two, three, four] = repository.as_ref() else { return None };
        tags.push(u32::from_be_bytes([*one, *two, *three, *four])).ok()?;
    }
    Some(tags.into_boxed())
}

/// Build a generic people request from temper's forge repository options.
/// The people child retains only the resource name, role and opaque options.
#[must_use]
pub fn adopt_repository_ask(adoption: forge::Adoption, connector: u16) -> Option<people::Ask> {
    let role = match adoption.role {
        forge::Role::Owned => people::ResourceRole::Owned,
        forge::Role::Fork => people::ResourceRole::Fork,
        forge::Role::Context => people::ResourceRole::Context,
        forge::Role::Adopted => return None,
    };
    let count = u32::try_from(adoption.checks.len()).ok()?.checked_add(7)?;
    let mut options = List::with_capacity(count);
    options.push(Box::from([u8::from(adoption.home)])).ok()?;
    options.push(adoption.host).ok()?;
    options.push(adoption.owner).ok()?;
    options.push(adoption.name).ok()?;
    options.push(adoption.prefix).ok()?;
    options.push(adoption.landing).ok()?;
    options.push(Box::from([u8::from(adoption.ci)])).ok()?;
    for check in adoption.checks {
        options.push(Box::from(check.to_be_bytes())).ok()?;
    }
    Some(people::Ask::Adopt {
        project: adoption.project,
        adoption: people::Adoption {
            resource: people::ResourceName {
                connector,
                path: Box::new([
                    Box::from(adoption.provider.forge.to_be_bytes()),
                    Box::from(adoption.provider.repository.to_be_bytes()),
                ]),
            },
            role,
            options: options.into_boxed(),
        },
    })
}

fn adoption_flag(value: &[u8]) -> Option<bool> {
    match value {
        [0] => Some(false),
        [1] => Some(true),
        _ => None,
    }
}

fn permission_number(permission: forge_client::api::Permission) -> u16 {
    match permission {
        forge_client::api::Permission::None => 0,
        forge_client::api::Permission::Read => 1,
        forge_client::api::Permission::Write => 2,
        forge_client::api::Permission::Admin => 3,
    }
}

fn seeded_role(domain: &Domain, project: u32, permission: forge_client::api::Permission) -> Option<people::Role> {
    let number = permission_number(permission);
    let mappings = domain.core.permission_roles.get(&project)?;
    for mapping in mappings.as_ref() {
        if mapping.connector == domain.config.forge_connector && mapping.permission == number {
            return Some(people::Role::from_number(mapping.role));
        }
    }
    None
}

/// Decode the forge's own options after the people child has authorized the
/// resource adoption. Malformed connector data refuses without a forge call.
pub(super) fn parse_adoption(project: u32, adoption: people::Adoption, connector: u16) -> Option<forge::Adoption> {
    if adoption.resource.connector != connector || adoption.resource.path.len() != 2 || adoption.options.len() < 7 {
        return None;
    }
    let forge_id = adoption.resource.path.first()?;
    let repo_id = adoption.resource.path.get(1)?;
    if forge_id.len() != 2 || repo_id.len() != 4 {
        return None;
    }
    let forge = u16::from_be_bytes([*forge_id.first()?, *forge_id.get(1)?]);
    let repository = u32::from_be_bytes([*repo_id.first()?, *repo_id.get(1)?, *repo_id.get(2)?, *repo_id.get(3)?]);
    if forge == 0 || repository == 0 {
        return None;
    }
    let home = adoption_flag(adoption.options.first()?)?;
    let ci = adoption_flag(adoption.options.get(6)?)?;
    let mut checks = List::with_capacity(u32::try_from(adoption.options.len().checked_sub(7)?).ok()?);
    for option in adoption.options.get(7..)? {
        if option.len() != 4 {
            return None;
        }
        checks.push(u32::from_be_bytes([*option.first()?, *option.get(1)?, *option.get(2)?, *option.get(3)?])).ok()?;
    }
    let role = match adoption.role {
        people::ResourceRole::Owned => forge::Role::Owned,
        people::ResourceRole::Fork => forge::Role::Fork,
        people::ResourceRole::Context => forge::Role::Context,
    };
    Some(forge::Adoption {
        project,
        home,
        provider: forge_client::api::Repository { forge, repository },
        host: adoption.options.get(1)?.clone(),
        owner: adoption.options.get(2)?.clone(),
        name: adoption.options.get(3)?.clone(),
        prefix: adoption.options.get(4)?.clone(),
        role,
        landing: adoption.options.get(5)?.clone(),
        ci,
        checks: checks.into_boxed(),
    })
}

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
    match forge::effect_access(repository, what, kind) {
        forge::Access::Owned => authority::EffectAccess::Owned,
        forge::Access::Participant => authority::EffectAccess::Participant,
        forge::Access::Context => authority::EffectAccess::Context,
        forge::Access::Unavailable => authority::EffectAccess::Unavailable,
    }
}

fn effect_key(domain: &Domain, key: CallKey) -> Box<[u8]> {
    forge_client::effect_key(
        &domain.core.counters.deployment().id,
        &forge_client::EffectPurpose::Call {
            task: key.task,
            attempt: key.attempt,
            completion: key.completion,
            position: key.position,
        },
        44,
    )
    .expect("fixed call key fits its frame")
}

#[expect(clippy::too_many_arguments, reason = "one named call carries the root and its typed forge write")]
pub(super) fn effect_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    repository: forge_client::api::Repository,
    resource: forge::What,
    write: forge_client::api::Write,
    proposal: Option<(Box<[u8]>, bool)>,
) {
    let owner = effect_owner(domain);
    let flight = match describe_agent(domain, env, key, repository, resource, write) {
        Ok(flight) => flight,
        Err(forge_client::api::Error::Busy) => {
            super::relay_call(domain, &env.limits, decision, to, CallAnswer::Unavailable);
            return;
        }
        Err(why) => {
            decide_call(domain, &env.limits, decision, to, key, CallAnswer::ForgeEffectRefused(why));
            return;
        }
    };
    assert!(domain.forge_effects.insert(owner, flight).is_ok(), "admitted forge effect handoff");
    let deadline = skein_lib::Wall::from_nanos(
        env.wall.as_nanos().saturating_add(domain.core.settings.run.call_timeout.as_nanos()),
    );
    let origin = match proposal {
        Some((reason, as_holder)) => jig_core::EffectOrigin::Propose { to, key, reason, as_holder },
        None => jig_core::EffectOrigin::Call { to, key, deadline },
    };
    domain.work.push(Work::Core(jig_core::Event::EffectStart {
        owner,
        connector: domain.config.forge_connector,
        origin,
    }));
}

fn describe_agent(
    domain: &Domain,
    env: &Env<Limits>,
    key: CallKey,
    repository: forge_client::api::Repository,
    resource: forge::What,
    write: forge_client::api::Write,
) -> Result<EffectFlight, forge_client::api::Error> {
    let mut staged = 0_u32;
    for (_, flight) in &domain.forge_effects {
        match flight {
            EffectFlight::Call { .. } => staged = staged.saturating_add(1),
            EffectFlight::Procedure { .. } | EffectFlight::Projection { .. } => {}
        }
    }
    if !domain.forge.effect_room(&env.limits.forge, staged) {
        return Err(forge_client::api::Error::Busy);
    }
    let adopted = domain.forge.repository(repository).ok_or(forge_client::api::Error::Missing)?;
    if !domain.core.connector_project(key.task, adopted.project) {
        return Err(forge_client::api::Error::Forbidden);
    }
    let described = match forge::describe_agent_effect(
        adopted,
        &resource,
        write,
        effect_key(domain, key),
        env.limits.forge.client.op_bytes,
    ) {
        Ok(described) => described,
        Err(why) => {
            return Err(why);
        }
    };
    let name =
        resource_name(adopted, &resource, env.limits.authority.segments).ok_or(forge_client::api::Error::TooLarge)?;
    let form = match described.form {
        forge::EffectForm::Creation => EffectForm::Creation,
        forge::EffectForm::Transition => EffectForm::Transition,
        forge::EffectForm::Set => EffectForm::Set,
    };
    let recovery = match described.recovery {
        forge::Recovery::Keyed => Recovery::Keyed,
        forge::Recovery::Conditional => Recovery::Conditional,
        forge::Recovery::Idempotent => Recovery::Idempotent,
        forge::Recovery::Unrecoverable => Recovery::Unrecoverable,
    };
    let description = EffectDescription {
        connector: domain.config.forge_connector,
        purpose: key.completion.into(),
        effect: authority::Effect {
            connector: domain.config.forge_connector,
            kind: described.kind,
            name,
            state: described.state,
            price: None,
            access: effect_access(adopted, &resource, described.kind),
            additional: Box::new([]),
            guards: Box::new([]),
        },
        form,
        recovery,
    };
    Ok(EffectFlight::Call { description: Box::new(description), repository, effect: described.effect, key, resource })
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
    let Some(context) = domain.core.tasks.delegation(key.task) else {
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
                Queue::with_capacity(authority::max_out(domain.core.authority.limits()).expect("authority findings"));
            domain.core.connector_read_admit(
                key.task,
                authority::Effect {
                    connector: domain.config.forge_connector,
                    kind: 1,
                    name,
                    state: [0; 32],
                    price: None,
                    access: effect_access(adopted, &forge::What::Repository, 1),
                    additional: Box::new([]),
                    guards: Box::new([]),
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
    let Some(serial) = crate::fresh(&mut domain.core.counters, Family::Call) else {
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
    assert!(domain.core.reserve_connector_call(key), "call record room reserved");
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
    let Some(number) = crate::fresh(&mut domain.core.counters, Family::Message) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::SubscriptionRefused(tasks::Problem {
                task: Some(key.task),
                why: tasks::Refusal::Busy,
                blocked_by: None,
            }),
        );
        return;
    };
    let subscriber = forge::Subscriber { task: key.task, number, topic, own_change, goal_tasks: None, paths };
    if !domain.forge.can_subscribe(&env.limits.forge, &subscriber)
        || watch_names(domain, &env.limits, &subscriber).is_none()
    {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::SubscriptionRefused(tasks::Problem {
                task: Some(key.task),
                why: tasks::Refusal::Subscription,
                blocked_by: None,
            }),
        );
        return;
    }
    let token = to.into_token();
    assert!(domain.core.reserve_connector_subscription(token, key, number), "one routed call");
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

/// The hub sees an opaque, bounded name for a forge branch. Forge path syntax
/// stays here; the task child only compares these literal bytes.
pub(super) fn hub_name(connector: u16, name: &forge::Name, limits: &tasks::Limits) -> Option<tasks::Name> {
    let forge::What::Branch(parts) = &name.what else { return None };
    if limits.hold_segments < 3 {
        return None;
    }
    let mut branch = List::with_capacity(limits.hold_bytes);
    for (at, part) in parts.iter().enumerate() {
        if at != 0 {
            branch.push(b'/').ok()?;
        }
        for byte in part.iter().copied() {
            branch.push(byte).ok()?;
        }
    }
    if branch.len().checked_add(6)? > limits.hold_bytes {
        return None;
    }
    Some(tasks::Name {
        connector,
        path: Box::new([
            Box::from(name.forge.to_be_bytes()),
            Box::from(name.repository.to_be_bytes()),
            branch.into_boxed(),
        ]),
    })
}

/// Decode only this connector's branch names when projecting a hub hold.
pub(super) fn forge_name(connector: u16, resource: &tasks::Name, limit: u32) -> Option<forge::Name> {
    if resource.connector != connector {
        return None;
    }
    let [forge, repository, branch] = resource.path.as_ref() else { return None };
    let [one, two] = forge.as_ref() else { return None };
    let [three, four, five, six] = repository.as_ref() else { return None };
    Some(forge::Name {
        forge: u16::from_be_bytes([*one, *two]),
        repository: u32::from_be_bytes([*three, *four, *five, *six]),
        what: branch_what(branch, limit)?,
    })
}

fn write_holding(connector: u16, name: &forge::Name, limits: &tasks::Limits) -> Option<tasks::Holding> {
    Some(tasks::Holding::Write { resource: hub_name(connector, name, limits)?, kind: 1 })
}

/// Name the forge branches a new task will own before the hub admits its batch.
#[expect(clippy::too_many_arguments, reason = "task identity and specification arrive from separate admitted carriers")]
pub(super) fn task_holdings(
    domain: &Domain,
    env: &Env<Limits>,
    project: u32,
    root: u64,
    number: u64,
    executor: tasks::Executor,
    spec: &tasks::Spec,
    parent: Option<u64>,
) -> Option<Box<[tasks::Holding]>> {
    let child_of_change = match parent {
        Some(task) => domain.forge.change(task).is_some(),
        None => false,
    };
    let eligible = match executor {
        tasks::Executor::Person(_) => false,
        tasks::Executor::Procedure { connector, code } => {
            connector == domain.config.forge_connector && (code == 1 || code == 2)
        }
        tasks::Executor::Agent { .. } => !child_of_change,
    };
    if !eligible {
        return Some(Box::new([]));
    }
    let mut selected = List::with_capacity(env.limits.forge.resources_per_task);
    if let tasks::Executor::Procedure { connector, code: 1 | 2 } = executor
        && connector == domain.config.forge_connector
    {
        let mut repository = None;
        let mut chosen = None;
        for parameter in &spec.parameters {
            match parameter {
                tasks::Parameter::Resource { name: 1, connector, resource }
                    if *connector == domain.config.forge_connector =>
                {
                    repository = domain.forge.repository_tag(project, u32::try_from(*resource).ok()?);
                }
                tasks::Parameter::Bytes { name: 3, value } => chosen = Some(value.clone()),
                tasks::Parameter::Number { .. }
                | tasks::Parameter::Bytes { .. }
                | tasks::Parameter::Resource { .. } => {}
            }
        }
        let repository = repository?;
        let branch = match chosen {
            Some(branch) => branch,
            None => tree_branch(repository, root, b'c', number, None, env.limits.forge.name_bytes)?,
        };
        let name = forge::Name {
            forge: repository.provider.forge,
            repository: repository.provider.repository,
            what: branch_what(&branch, env.limits.forge.name_bytes)?,
        };
        return Some(Box::new([write_holding(domain.config.forge_connector, &name, &env.limits.tasks)?]));
    }
    if let Some(home) = domain.forge.home(project) {
        include_repository(&mut selected, home)?;
    }
    for parameter in &spec.parameters {
        if let tasks::Parameter::Resource { connector, resource, .. } = parameter
            && *connector == domain.config.forge_connector
        {
            let repository = domain.forge.repository_tag(project, u32::try_from(*resource).ok()?)?;
            include_repository(&mut selected, repository)?;
        }
    }
    let mut holdings = List::with_capacity(env.limits.tasks.holdings);
    for repository in &selected {
        if repository.role == forge::Role::Context || !repository.kinds.push {
            continue;
        }
        let branch = tree_branch(repository, root, b'r', number, None, env.limits.forge.name_bytes)?;
        let name = forge::Name {
            forge: repository.provider.forge,
            repository: repository.provider.repository,
            what: branch_what(&branch, env.limits.forge.name_bytes)?,
        };
        holdings.push(write_holding(domain.config.forge_connector, &name, &env.limits.tasks)?).ok()?;
    }
    Some(holdings.into_boxed())
}

#[derive(Debug)]
pub(super) struct RunWorkspace {
    pub workspace: ForgeWorkspace,
    pub writes: Box<[authority::Write]>,
    pub names: Box<[forge::Name]>,
    pub own_holds: Box<[forge::Name]>,
}

/// Translate committed tracked-task facts into one bounded issue projection.
#[expect(
    clippy::disallowed_methods,
    clippy::wildcard_enum_match_arm,
    reason = "projection converts already validated task text and selects current milestone kinds"
)]
pub(super) fn project_goal(domain: &mut Domain, env: &Env<Limits>, feed: &jig_core::ProjectionFeed) {
    goal_topics(domain, env, feed);
    let goal = &feed.goal;
    let Some(repository) = domain.forge.home(goal.project) else {
        if feed.closing {
            domain.work.push(Work::Core(jig_core::Event::ProjectionSettled {
                goal: goal.number,
                connector: domain.config.forge_connector,
            }));
        }
        return;
    };
    let provider = repository.provider;
    let Some(text) = core::str::from_utf8(&goal.words).ok() else {
        domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
        return;
    };
    let title = text.lines().next().unwrap_or(text);
    let mut plan = List::with_capacity(env.limits.forge.issue_policy.plan_items);
    for child in &feed.plan {
        if child.number == goal.number {
            continue;
        }
        let Some(words) = core::str::from_utf8(&child.words).ok() else { continue };
        if plan.push(forge_issues::PlanItem { text: Box::from(words), phase: projection_phase(&child.phase) }).is_err()
        {
            domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
            return;
        }
    }
    let mut milestones = List::with_capacity(env.limits.forge.issue_policy.milestones);
    for milestone in &feed.milestones {
        let kept = match &milestone.phase {
            Some(tasks::Phase::Ended(_) | tasks::Phase::Held { .. }) => true,
            Some(tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Closing(_)) | None => false,
        };
        if !kept && milestone.change.is_none() {
            continue;
        }
        let key = match milestone.identity {
            jig_core::MilestoneId::Lifecycle { task, position } => {
                forge_issues::MilestoneKey::Lifecycle { task, position }
            }
            jig_core::MilestoneId::Revision { task, revision } => {
                forge_issues::MilestoneKey::TaskRevision { task, revision }
            }
        };
        let words = core::str::from_utf8(&milestone.words).unwrap_or("Task milestone");
        let words = if words.is_empty() { "Task milestone" } else { words };
        if milestones.push(forge_issues::Milestone { key, text: Box::from(words) }).is_err() {
            domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal.number, why: tasks::Hold::Effects }));
            return;
        }
    }
    let Some(entry) = crate::fresh(&mut domain.core.counters, Family::ConnectorRow) else { return };
    domain.work.push(Work::Forge(forge::Event::Project {
        entry,
        repository: provider,
        view: forge_issues::GoalView {
            phase: goal_phase(&goal.phase),
            goal: goal.number,
            repository: u64::from(provider.repository),
            title: Box::from(title),
            goal_text: Box::from(text),
            plan: plan.into_boxed(),
            milestones: milestones.into_boxed(),
            finished: match goal.phase {
                tasks::Phase::Closing(_)
                | tasks::Phase::Ended(_)
                | tasks::Phase::Held { was: tasks::Was::Closing(_), .. } => Some(Box::from("Goal finished")),
                _ => None,
            },
        },
    }));
}

fn projection_phase(phase: &tasks::Phase) -> forge_issues::Phase {
    match phase {
        tasks::Phase::Waiting => forge_issues::Phase::Waiting,
        tasks::Phase::Active(_) => forge_issues::Phase::Active,
        tasks::Phase::Held { .. } => forge_issues::Phase::Held,
        tasks::Phase::Closing(_) => forge_issues::Phase::Settling,
        tasks::Phase::Ended(ending) => ending_phase(ending),
    }
}
fn ending_phase(ending: &tasks::Ending) -> forge_issues::Phase {
    match ending {
        tasks::Ending::Done(_) => forge_issues::Phase::Done,
        tasks::Ending::Failed { .. } => forge_issues::Phase::Failed,
        tasks::Ending::Cancelled { .. } => forge_issues::Phase::Cancelled,
    }
}
fn goal_phase(phase: &tasks::Phase) -> forge_issues::Phase {
    match phase {
        tasks::Phase::Closing(closing) | tasks::Phase::Held { was: tasks::Was::Closing(closing), .. } => {
            ending_phase(&closing.ending)
        }
        tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Held { .. } | tasks::Phase::Ended(_) => {
            projection_phase(phase)
        }
    }
}

// Translate the plan's forge procedure parameters before those tasks activate.
fn goal_topics(domain: &mut Domain, env: &Env<Limits>, feed: &jig_core::ProjectionFeed) {
    if feed.closing {
        return;
    }
    for child in &feed.plan {
        let Some(task) = domain.core.tasks.task(child.number) else { continue };
        let relevant = match task.executor {
            tasks::Executor::Procedure { connector, code: 2 } => connector == domain.config.forge_connector,
            tasks::Executor::Procedure { .. } | tasks::Executor::Agent { .. } | tasks::Executor::Person(_) => false,
        };
        if !relevant {
            continue;
        }
        let Some(topic) = change_topic(domain, task) else { continue };
        subscribe_goal(domain, env, feed.goal.number, topic);
    }
}

fn change_topic(domain: &Domain, task: &tasks::TaskRecord) -> Option<forge::Topic> {
    let mut provider = None;
    let mut branch = None;
    for parameter in &task.spec.parameters {
        match parameter {
            tasks::Parameter::Resource { name: 1, connector, resource }
                if *connector == domain.config.forge_connector =>
            {
                if let Ok(tag) = u32::try_from(*resource) {
                    let repository = domain.forge.repository_tag(task.project, tag)?;
                    provider = Some(repository.provider);
                }
            }
            tasks::Parameter::Bytes { name: 2, value } => branch = Some(value.clone()),
            tasks::Parameter::Resource { .. } | tasks::Parameter::Bytes { .. } | tasks::Parameter::Number { .. } => {}
        }
    }
    let repository = provider?;
    let adopted = domain.forge.repository(repository)?;
    Some(forge::Topic::Landings { repository, branch: branch.unwrap_or(adopted.settings.default_branch.clone()) })
}

pub(super) fn subscribe_goal(domain: &mut Domain, env: &Env<Limits>, goal: u64, topic: forge::Topic) {
    if domain.core.tasks.task(goal).is_none() {
        return;
    }
    let mut previous = domain.forge.topic_subscriber(goal, &topic).cloned();
    for work in &domain.work {
        match work {
            Work::GoalSubscribe(subscriber) if subscriber.task == goal && subscriber.topic == topic => {
                previous = Some(subscriber.clone());
            }
            Work::GoalSubscribe(_)
            | Work::AdoptRestored
            | Work::AdoptDone
            | Work::Restart(_)
            | Work::TypedDecoded { .. }
            | Work::Core(_)
            | Work::Tasks(_)
            | Work::People(_)
            | Work::Fleet(_)
            | Work::Brief(_)
            | Work::StartBrief { .. }
            | Work::Forge(_)
            | Work::ProjectGoal(_)
            | Work::Activate(_)
            | Work::EscalationLoaded { .. }
            | Work::EscalationFailed { .. }
            | Work::ProposalLoaded { .. }
            | Work::ProposalFailed { .. } => {}
        }
    }
    let Some(members) = goal_members(domain, env, goal, &topic, previous.as_ref()) else { return };
    let number = match &previous {
        Some(previous) => previous.number,
        None => match crate::fresh(&mut domain.core.counters, Family::Message) {
            Some(number) => number,
            None => return,
        },
    };
    let subscriber = forge::Subscriber {
        task: goal,
        number,
        topic,
        own_change: None,
        goal_tasks: Some(members.tasks),
        paths: members.paths,
    };
    if previous.as_ref() == Some(&subscriber) {
        return;
    }
    if !domain.forge.can_subscribe(&env.limits.forge, &subscriber)
        || watch_names(domain, &env.limits, &subscriber).is_none()
    {
        return;
    }
    if previous.is_none() {
        domain.work.push(Work::Tasks(tasks::Event::SubscribeTopic {
            reply_to: super::internal(u64::MAX),
            task: goal,
            subscription: tasks::Subscription {
                number,
                kind: tasks::SubscriptionKind::Topic { connector: domain.config.forge_connector, topic: number },
            },
        }));
    }
    domain.work.push(Work::GoalSubscribe(subscriber));
}

struct GoalMembers {
    tasks: Box<[u64]>,
    paths: Box<[Box<[u8]>]>,
}

fn goal_members(
    domain: &Domain,
    env: &Env<Limits>,
    goal: u64,
    topic: &forge::Topic,
    previous: Option<&forge::Subscriber>,
) -> Option<GoalMembers> {
    let mut goal_tasks = List::with_capacity(env.limits.forge.issue_policy.plan_items);
    let mut paths = List::with_capacity(env.limits.forge.paths_per_subscription);
    let mut hints_complete = true;
    if let Some(previous) = previous
        && let Some(tasks) = &previous.goal_tasks
    {
        for task in tasks {
            if goal_tasks.push(*task).is_err() {
                return None;
            }
        }
    }
    for view in domain.core.tasks.view_tasks() {
        let Some(task) = domain.core.tasks.task(view.number) else { continue };
        if task.root != goal || task.number == goal {
            continue;
        }
        match task.executor {
            tasks::Executor::Procedure { connector, code: 2 } if connector == domain.config.forge_connector => {}
            tasks::Executor::Procedure { .. } | tasks::Executor::Agent { .. } | tasks::Executor::Person(_) => continue,
        }
        let Some(change_topic) = change_topic(domain, task) else { continue };
        match topic {
            forge::Topic::Landings { .. } if *topic != change_topic => continue,
            forge::Topic::Ci { repository, .. } => match change_topic {
                forge::Topic::Landings { repository: home, .. } if home == *repository => {}
                forge::Topic::Landings { .. }
                | forge::Topic::Ci { .. }
                | forge::Topic::Pull { .. }
                | forge::Topic::Participation { .. } => continue,
            },
            forge::Topic::Landings { .. } | forge::Topic::Pull { .. } | forge::Topic::Participation { .. } => {}
        }
        let mut hinted = false;
        let mut present = false;
        for kept in &goal_tasks {
            if *kept == task.number {
                present = true;
            }
        }
        if !present && goal_tasks.push(task.number).is_err() {
            return None;
        }
        for parameter in &task.spec.parameters {
            match parameter {
                tasks::Parameter::Bytes { name: 6, value } => {
                    hinted = true;
                    if paths.push(value.clone()).is_err() {
                        return None;
                    }
                }
                tasks::Parameter::Bytes { .. }
                | tasks::Parameter::Number { .. }
                | tasks::Parameter::Resource { .. } => {}
            }
        }
        let produced = match domain.forge.change(task.number) {
            Some(change) => change.pull.is_some(),
            None => false,
        };
        if !produced && !hinted {
            hints_complete = false;
        }
    }
    if !hints_complete {
        paths = List::with_capacity(env.limits.forge.paths_per_subscription);
    }
    Some(GoalMembers { tasks: goal_tasks.into_boxed(), paths: paths.into_boxed() })
}

pub(super) fn goal_subscribed(domain: &mut Domain, env: &Env<Limits>, subscriber: forge::Subscriber) {
    let Some(task) = domain.core.tasks.task(subscriber.task) else { return };
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
    let Some(bound) = authority::max_out(domain.core.authority.limits()) else { return false };
    let mut findings = Queue::with_capacity(bound);
    domain.core.connector_run_write_admit(
        context,
        authority::Effect {
            connector: domain.config.forge_connector,
            kind: 2,
            name,
            state: [0; 32],
            price: None,
            access: effect_access(repository, &what, 2),
            additional: Box::new([]),
            guards: Box::new([]),
        },
        env.wall,
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
        let row = domain.core.tasks.task(task)?;
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
    _attempt: u64,
) -> Option<RunWorkspace> {
    let root = domain.core.tasks.root(context.task)?;
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
        let saved = saved_tags(&context.saved, domain.config.forge_connector)?;
        for tag in &saved {
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
    let mut key = context.task;
    for repository in &selected {
        let saved = context.saved.contains(&tasks::SavedResource {
            connector: domain.config.forge_connector,
            path: Box::new([Box::from(repository.provider.repository.to_be_bytes())]),
        });
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
            let branch = tree_branch(repository, root, b'r', context.task, None, env.limits.forge.name_bytes)?;
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
                    if domain.core.tasks.holder(&hub_name(domain.config.forge_connector, &name, &env.limits.tasks)?)
                        != Some(owner)
                    {
                        return None;
                    }
                    authority::Writer::Ancestor
                }
                None => {
                    if domain.core.tasks.holder(&hub_name(domain.config.forge_connector, &name, &env.limits.tasks)?)
                        != Some(context.task)
                    {
                        return None;
                    }
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
    })
}

/// Reconstruct the connector's provisional writer projection from the
/// assignment only after the hub has durably admitted its writer slots.
#[expect(clippy::type_complexity, reason = "the connector claim carries paired bounded names and ancestor holders")]
pub(super) fn claimed_writes(
    domain: &Domain,
    env: &Env<Limits>,
    task: u64,
) -> Option<(Box<[forge::Name]>, Box<[u64]>)> {
    let assignment = domain.assignments.get(&task)?;
    let mut writes = List::with_capacity(env.limits.forge.resources_per_task);
    let mut holders = List::with_capacity(env.limits.forge.resources_per_task);
    for repository in &assignment.workspace.repositories {
        let Some(push) = &repository.push else { continue };
        let name = forge::Name {
            forge: repository.provider.forge,
            repository: repository.provider.repository,
            what: branch_what(push, env.limits.forge.name_bytes)?,
        };
        let holder = domain.core.tasks.holder(&hub_name(domain.config.forge_connector, &name, &env.limits.tasks)?)?;
        if holder != task {
            holders.push(holder).ok()?;
        }
        writes.push(name).ok()?;
    }
    Some((writes.into_boxed(), holders.into_boxed()))
}

#[expect(clippy::manual_map, reason = "the strict subset uses an explicit option match")]
fn pull_what(number: Option<u64>) -> Option<forge::What> {
    match number {
        Some(number) => Some(forge::What::Pull(number)),
        None => None,
    }
}

/// Project landing gates are procedure gates before they become effect requirements.
fn gate_required(domain: &Domain, project: u32, name: &authority::Name, parameters: u32, project_scope: bool) -> bool {
    domain.core.connector_gate_required(project, domain.config.forge_connector, name, parameters, project_scope)
}

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
    let empty: &[LandingRule] = &[];
    let project_rules = match project {
        Some(rules) => rules.as_ref(),
        None => empty,
    };
    let mut project_scope = false;
    for rules in [&domain.config.landing.deployment[..], project_rules] {
        let mut criterion = 0_u32;
        for rule in rules {
            if rule.ci {
                criterion = criterion.checked_add(1)?;
            }
            if rule.up_to_date {
                criterion = criterion.checked_add(1)?;
            }
            let applies = rule.connector == domain.config.forge_connector
                && rule.kind == 4
                && authority::pattern_covers(&rule.pattern, &name);
            for gate in &rule.gates {
                let parameters = criterion | if project_scope { 0x8000_0000 } else { 0 };
                if gate.blocking {
                    criterion = criterion.checked_add(1)?;
                }
                if !applies {
                    continue;
                }
                let mut present = false;
                for prior in gates.as_slice() {
                    if prior.number == u64::from(gate.number) {
                        present = true;
                    }
                }
                let is_check = !repository.ci && repository.checks.contains(&gate.number);
                let required =
                    gate.blocking && gate_required(domain, repository.project, &name, parameters, project_scope);
                if present || (!is_check && gate.blocking && !required) {
                    continue;
                }
                gates
                    .push(forge_change::Gate {
                        number: u64::from(gate.number),
                        kind: if is_check { forge_change::GateKind::Check } else { forge_change::GateKind::Agent },
                        blocking: is_check || required,
                        freshness: if is_check {
                            forge_change::Freshness::Exact
                        } else {
                            match gate.freshness {
                                Freshness::Exact => forge_change::Freshness::Exact,
                                Freshness::Clean => forge_change::Freshness::Clean,
                            }
                        },
                        eager: false,
                    })
                    .ok()?;
            }
            criterion = criterion.checked_add(u32::try_from(rule.approvals.len()).ok()?)?;
        }
        project_scope = true;
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
        let Some((person, role)) = domain.core.people.role_for_identity(
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

fn describe_change_effect(
    domain: &Domain,
    env: &Env<Limits>,
    task: u64,
    effect: forge_change::Effect,
    evidence: &forge::ChangeEvidence,
) -> Option<EffectDescription> {
    let row = domain.forge.change(task)?;
    let repository = domain.forge.repository(row.repository)?;
    let context = domain.core.tasks.delegation(task)?;
    if repository.project != context.project || repository.role == forge::Role::Context {
        return None;
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
    let what = what?;
    let name = resource_name(repository, &what, env.limits.authority.segments)?;
    let effect = authority::Effect {
        connector: domain.config.forge_connector,
        kind,
        name,
        state,
        price: None,
        access: effect_access(repository, &what, kind),
        additional: Box::new([]),
        guards: Box::new([]),
    };
    let (forge_form, forge_recovery) = forge::effect_shape(kind)?;
    let form = match forge_form {
        forge::EffectForm::Creation => EffectForm::Creation,
        forge::EffectForm::Transition => EffectForm::Transition,
        forge::EffectForm::Set => EffectForm::Set,
    };
    let recovery = match forge_recovery {
        forge::Recovery::Keyed => Recovery::Keyed,
        forge::Recovery::Conditional => Recovery::Conditional,
        forge::Recovery::Idempotent => Recovery::Idempotent,
        forge::Recovery::Unrecoverable => Recovery::Unrecoverable,
    };
    Some(EffectDescription {
        connector: domain.config.forge_connector,
        purpose: u64::from(kind),
        effect,
        form,
        recovery,
    })
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
            tasks::Parameter::Bytes { name: 6, .. } => {}
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
            let Some(root) = domain.core.tasks.root(task) else { return false };
            let Some(branch) = tree_branch(adopted, root, b'c', task, None, env.limits.forge.name_bytes) else {
                return false;
            };
            branch
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
    let Some(entry) = crate::fresh(&mut domain.core.counters, Family::ConnectorRow) else { return false };
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
            Some((_, repair)) => domain.core.tasks.task(repair).is_some(),
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
    let context = domain.core.tasks.delegation(task)?;
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
            tasks::Executor::Agent { charter: domain.core.settings.charter },
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
                tasks::Executor::Agent { charter: domain.core.settings.charter },
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
                tasks::Executor::Agent { charter: domain.core.settings.charter },
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
                    tasks::Executor::Agent { charter: domain.core.settings.charter }
                }
                forge_change::GateKind::Person => tasks::Executor::Person(tasks::PersonAddress::Role(1)),
            };
            (
                executor,
                Box::from(&b"Review this change at the named head"[..]),
                tasks::Contract::Verdict {
                    choices: Box::new([
                        tasks::Verdict { code: 1, words: env.limits.tasks.result_bytes, followups: 0 },
                        tasks::Verdict { code: 2, words: env.limits.tasks.result_bytes, followups: 0 },
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
        && domain.core.tasks.task(repair).is_some()
    {
        return true;
    }
    if domain.forge.queue_repairs(row.repository, &row.base) >= env.limits.forge.change_policy.repairs {
        return false;
    }
    let Some(repository) = domain.forge.repository(row.repository) else { return false };
    let project = repository.project;
    let Some(seed) = domain.core.repair_seed(project) else { return false };
    let repair = seed.number();
    let provider = row.repository;
    let base = row.base.clone();
    let spec = tasks::Spec {
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
    };
    let executor = tasks::Executor::Procedure { connector: domain.config.forge_connector, code: 2 };
    let Some(holdings) = task_holdings(domain, env, project, repair, repair, executor, &spec, None) else {
        return false;
    };
    domain.work.push(Work::Forge(forge::Event::QueueRepairStarted { owner: task, repair }));
    domain.work.push(Work::Core(jig_core::Event::ConnectorRepair {
        seed: Box::new(seed),
        executor,
        spec,
        contract: tasks::Contract::Change {
            connector: domain.config.forge_connector,
            kind: 1,
            words: env.limits.tasks.result_bytes,
        },
        holdings,
    }));
    true
}

fn change_decision(domain: &mut Domain, env: &Env<Limits>, task: u64, choice: forge_change::Decision) {
    let Some((connector, code, step)) = domain.core.tasks.procedure_due(task) else { return };
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
                domain.work.push(Work::Core(jig_core::Event::ProcedureStep {
                    task,
                    step,
                    connector,
                    code,
                    action: ProcedureAction::Hold(tasks::Hold::Effects),
                }));
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
                forge_change::Hold::Stalled => tasks::Hold::Stalled,
                forge_change::Hold::Repairs | forge_change::Hold::Resolutions | forge_change::Hold::Updates => {
                    tasks::Hold::Procedure
                }
                forge_change::Hold::Failed | forge_change::Hold::Rejected => tasks::Hold::EffectFailed,
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
                domain.work.push(Work::Core(jig_core::Event::ProcedureStep {
                    task,
                    step,
                    connector,
                    code,
                    action: ProcedureAction::Hold(tasks::Hold::Effects),
                }));
                return;
            };
            assert!(domain.forge_delegating.insert((task, step), kind).is_ok(), "one forge procedure delegate");
            domain.work.push(Work::Core(jig_core::Event::ProcedureStep {
                task,
                step,
                connector,
                code,
                action: ProcedureAction::Delegate(Box::new([delegate])),
            }));
            return;
        }
    };
    domain.work.push(Work::Core(jig_core::Event::ProcedureStep { task, step, connector, code, action }));
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
            forge::Request::GoalTopic { goal, topic } => subscribe_goal(domain, env, goal, topic),
            forge::Request::GoalUntopic { goal, topic } => {
                if let Some(subscriber) = domain.forge.topic_subscriber(goal, &topic) {
                    domain.work.push(Work::Tasks(tasks::Event::Unsubscribe {
                        reply_to: super::internal(u64::MAX),
                        task: goal,
                        subscription: subscriber.number,
                    }));
                    domain.work.push(Work::Forge(forge::Event::Unsubscribe { task: goal, topic }));
                }
            }
            forge::Request::RestartDone { stage } => {
                let stage = match stage {
                    forge::RestartStage::Restored => jig_core::connector::RestartStage::Restored,
                    forge::RestartStage::ReadAfresh => jig_core::connector::RestartStage::ReadAfresh,
                    forge::RestartStage::Settled => jig_core::connector::RestartStage::Settled,
                };
                super::connector_restart_done(domain, stage);
            }
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
                if let forge::Stored::Hold(row) = &record
                    && row.writer.is_none()
                    && let Some(resource) = hub_name(domain.config.forge_connector, &row.name, &env.limits.tasks)
                {
                    domain.work.push(Work::Tasks(tasks::Event::ReadAfresh { resource }));
                }
                let key = forge::stored_key(&record);
                let first = !domain.forge_keys.contains_key(&key);
                let number = match domain.forge_keys.get(&key) {
                    Some(number) => *number,
                    None => {
                        let number =
                            crate::fresh(&mut domain.core.counters, Family::ConnectorRow).expect("admitted forge row");
                        domain.forge_keys.insert(key, number).expect("bounded connector rows");
                        number
                    }
                };
                let entry = match &record {
                    forge::Stored::Entry(row) => Some((row.number, row.task)),
                    forge::Stored::ProposedEffect(_)
                    | forge::Stored::Repository(_)
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
                    let core_effect = domain.forge_effecting.remove(&entry).is_some();
                    let projection_pending = match domain.forge.issue(task) {
                        Some(row) => row.pending == Some(entry),
                        None => false,
                    };
                    let change_pending = match domain.forge.change(task) {
                        Some(row) => row.pending == Some(entry),
                        None => false,
                    };
                    if first && !change_pending && !projection_pending && !core_effect {
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
                        let mut parties = List::with_capacity(env.limits.forge.collaborators);
                        for collaborator in &adopted.collaborators {
                            if let Some(role) = seeded_role(domain, adopted.repository.project, collaborator.permission)
                            {
                                parties
                                    .push(jig_core::connector::AdoptedParty {
                                        subject: collaborator.user.to_be_bytes().into(),
                                        role,
                                    })
                                    .expect("bounded collaborators");
                            }
                        }
                        match domain.core.connector_adopt_parties(
                            &env.limits.people,
                            adopted.repository.project,
                            0,
                            parties.into_boxed(),
                        ) {
                            Ok(seeds) => {
                                domain.work.push(Work::People(people::Event::Seed {
                                    project: adopted.repository.project,
                                    collaborators: seeds,
                                }));
                                domain.work.push(Work::People(people::Event::Decided {
                                    request: reply_to,
                                    outcome: people::Outcome::Adopted { project: adopted.repository.project },
                                }));
                            }
                            Err(why) => {
                                domain.work.push(Work::Forge(forge::Event::ForgetAdoption {
                                    repository: adopted.repository.provider,
                                    restore: previous,
                                }));
                                domain.work.push(Work::People(people::Event::Decided {
                                    request: reply_to,
                                    outcome: people::Outcome::Refused(why),
                                }));
                            }
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
                    forge::Class::Dropped => tasks::NewsClass::Dropped,
                };
                if let Some(event) = domain.core.connector_news(
                    task,
                    subscription,
                    class,
                    news_words(&news, env.limits.tasks.message_bytes),
                    env.wall,
                ) {
                    domain.work.push(Work::Tasks(event));
                }
            }
            forge::Request::Taken { task, .. } | forge::Request::Refused { task } => {
                if domain.core.claiming.contains_key(&task) {
                    domain.work.push(Work::Core(jig_core::Event::ClaimRefused { task }));
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
                    let (_, key) = domain.forge_effecting.remove(&entry).expect("effect awaiting connector admission");
                    if let Some(CallAnswer::ForgeEffect { deadline, .. }) = domain.connector_calls.get(&key) {
                        let answer = CallAnswer::ForgeEffect {
                            entry,
                            deadline: *deadline,
                            outcome: Some(forge_client::Outcome::Failed(forge_client::api::Error::Busy)),
                        };
                        assert!(domain.connector_calls.insert(key, answer).is_ok(), "retained connector answer");
                    }
                    domain.work.push(Work::Core(jig_core::Event::EffectConnector(
                        jig_core::connector::Event::Outbox {
                            entry,
                            task: key.task,
                            outcome: jig_core::connector::OutboxOutcome::Failed,
                        },
                    )));
                }
                domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
            }
            forge::Request::Outcome { entry, task, outcome } => {
                if outcome != forge_client::Outcome::Uncertain {
                    let mut named = None;
                    for (&key, answer) in &domain.connector_calls {
                        match answer {
                            CallAnswer::ForgeEffect { entry: number, .. } if *number == entry => {
                                named = Some(key);
                                break;
                            }
                            CallAnswer::ForgeEffect { .. }
                            | CallAnswer::ForgeEffectRefused(_)
                            | CallAnswer::ForgeEffectDenied { .. }
                            | CallAnswer::ToolDenied { .. }
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
                            | CallAnswer::NoteWritten { .. }
                            | CallAnswer::NoteRecalled { .. }
                            | CallAnswer::NoteRefused(_)
                            | CallAnswer::Unavailable => {}
                        }
                    }
                    if let Some(key) = named {
                        let deadline = match domain.connector_calls.get(&key) {
                            Some(CallAnswer::ForgeEffect { deadline, .. }) => *deadline,
                            Some(_) | None => unreachable!("retained effect call"),
                        };
                        let answer = CallAnswer::ForgeEffect { entry, deadline, outcome: Some(outcome) };
                        domain.connector_calls.insert(key, answer.clone()).expect("replaces retained named effect");
                        save(
                            decision,
                            &env.limits,
                            Write::Save(Record::Call(crate::CallRecord {
                                key,
                                answer,
                                settled: domain.core.call_settled.get(&key).cloned(),
                            })),
                        );
                    }
                }
                let result = match outcome {
                    forge_client::Outcome::Made { .. } => jig_core::connector::OutboxOutcome::Made,
                    forge_client::Outcome::Failed(_) | forge_client::Outcome::Raced { .. } => {
                        jig_core::connector::OutboxOutcome::Failed
                    }
                    forge_client::Outcome::Withdrawn => jig_core::connector::OutboxOutcome::Withdrawn,
                    forge_client::Outcome::Uncertain => jig_core::connector::OutboxOutcome::Uncertain,
                    forge_client::Outcome::Held => jig_core::connector::OutboxOutcome::Held { entry },
                };
                domain.work.push(Work::Core(jig_core::Event::EffectConnector(jig_core::connector::Event::Outbox {
                    entry,
                    task,
                    outcome: result,
                })));
            }
            forge::Request::ContinueRelease { task } => {
                if let Some(entry) = crate::fresh(&mut domain.core.counters, Family::ConnectorRow) {
                    domain.work.push(Work::Forge(forge::Event::ContinueRelease { task, entry }));
                } else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                }
            }
            forge::Request::Released { task } => {
                domain.work.push(Work::Tasks(tasks::Event::Settled { task }));
            }
            forge::Request::Retained { task, root, resource } => {
                if let Some(holding) = write_holding(domain.config.forge_connector, &resource, &env.limits.tasks) {
                    domain.work.push(Work::Tasks(tasks::Event::Retained { task, root, holding }));
                } else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                }
            }
            forge::Request::EffectsSettled { task } => {
                domain.work.push(Work::Core(jig_core::Event::EffectConnector(jig_core::connector::Event::Closed {
                    task,
                    connector: domain.config.forge_connector,
                })));
            }
            forge::Request::ReleaseFailed { task } => {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::EffectFailed }));
            }
            forge::Request::ProjectionEffect { row, entry, description } => {
                let Some(adopted) = domain.forge.repository(entry.repository) else { continue };
                let Some(name) = resource_name(adopted, &description.resource, env.limits.authority.segments) else {
                    continue;
                };
                let access = match description.access {
                    forge::Access::Owned => authority::EffectAccess::Owned,
                    forge::Access::Participant => authority::EffectAccess::Participant,
                    forge::Access::Context => authority::EffectAccess::Context,
                    forge::Access::Unavailable => authority::EffectAccess::Unavailable,
                };
                let description = EffectDescription {
                    connector: domain.config.forge_connector,
                    purpose: entry.number,
                    effect: authority::Effect {
                        connector: domain.config.forge_connector,
                        kind: description.kind,
                        name,
                        state: [0; 32],
                        price: None,
                        access,
                        additional: Box::new([]),
                        guards: Box::new([]),
                    },
                    form: match description.form {
                        forge::EffectForm::Creation => EffectForm::Creation,
                        forge::EffectForm::Transition => EffectForm::Transition,
                        forge::EffectForm::Set => EffectForm::Set,
                    },
                    recovery: match description.recovery {
                        forge::Recovery::Keyed => Recovery::Keyed,
                        forge::Recovery::Conditional => Recovery::Conditional,
                        forge::Recovery::Idempotent => Recovery::Idempotent,
                        forge::Recovery::Unrecoverable => Recovery::Unrecoverable,
                    },
                };
                let goal = row.goal;
                let number = entry.number;
                let owner = effect_owner(domain);
                domain
                    .forge_effects
                    .insert(
                        owner,
                        EffectFlight::Projection { description: Box::new(description), row: Box::new(row), entry },
                    )
                    .expect("bounded projection handoff");
                domain.work.push(Work::Core(jig_core::Event::EffectStart {
                    owner,
                    connector: domain.config.forge_connector,
                    origin: jig_core::EffectOrigin::Projection { goal, entry: Some(number) },
                }));
            }
            forge::Request::ProjectionFailed { goal } => {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: goal, why: tasks::Hold::Effects }));
            }
            forge::Request::Drift { task, .. } => {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Drift }));
            }
            forge::Request::ChangeDecision { task, decision: choice, entry, evidence } => match choice {
                forge_change::Decision::Effect(effect) => {
                    if let Some(entry) = entry
                        && let Some(evidence) = evidence
                    {
                        match describe_change_effect(domain, env, task, effect, &evidence) {
                            Some(description) => {
                                let Some((_, _, step)) = domain.core.tasks.procedure_due(task) else { continue };
                                let owner = effect_owner(domain);
                                assert!(
                                    domain
                                        .forge_effects
                                        .insert(
                                            owner,
                                            EffectFlight::Procedure {
                                                description: Box::new(description),
                                                entry,
                                                task,
                                                evidence: Box::new(evidence)
                                            }
                                        )
                                        .is_ok(),
                                    "admitted procedure effect handoff"
                                );
                                domain.work.push(Work::Core(jig_core::Event::EffectStart {
                                    owner,
                                    connector: domain.config.forge_connector,
                                    origin: jig_core::EffectOrigin::Procedure { task, step, entry: Some(entry) },
                                }));
                            }
                            None => {
                                domain.work.push(Work::Forge(forge::Event::VetoChange {
                                    task,
                                    entry,
                                    prior: evidence.prior,
                                }));
                                change_decision(
                                    domain,
                                    env,
                                    task,
                                    forge_change::Decision::Hold(forge_change::Hold::Failed),
                                );
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
                | forge_change::Decision::Hold(_) => change_decision(domain, env, task, choice),
            },
            forge::Request::ProjectionSettled { goal } => {
                if domain.core.tasks.task(goal).is_none() {
                    domain.forge_projection_due.remove(&goal);
                    domain.work.push(Work::Core(jig_core::Event::ProjectionSettled {
                        goal,
                        connector: domain.config.forge_connector,
                    }));
                }
            }
            forge::Request::ProjectAfter { goal, when } => {
                if let Some(when) = when {
                    domain.forge_projection_due.insert(goal, when).expect("projection count bounded by issue rows");
                } else if match domain.forge.issue(goal) {
                    Some(row) => row.pending.is_none(),
                    None => false,
                } && let Some(entry) = crate::fresh(&mut domain.core.counters, Family::ConnectorRow)
                {
                    domain.work.push(Work::Forge(forge::Event::ProjectDesired { entry, goal }));
                }
            }
        }
    }
}

/// Connector-owned effect data while the core chooses its requirements.
#[derive(Debug)]
pub(super) enum EffectFlight {
    Call {
        description: Box<EffectDescription>,
        repository: forge_client::api::Repository,
        effect: forge_client::Effect,
        key: CallKey,
        resource: forge::What,
    },
    Projection {
        description: Box<EffectDescription>,
        row: Box<forge::IssueRow>,
        entry: forge_client::Entry,
    },
    Procedure {
        description: Box<EffectDescription>,
        entry: u64,
        task: u64,
        evidence: Box<forge::ChangeEvidence>,
    },
}

fn effect_owner(domain: &mut Domain) -> Token {
    let owner = Token::new(domain.next_effect_owner);
    domain.next_effect_owner = domain.next_effect_owner.checked_add(1).expect("transient effect owner counter");
    owner
}

/// Assemble connector evidence without replacing the core's status decision.
pub(super) fn effect_answer(domain: &Domain, key: CallKey, part: &jig_core::CallPart) -> CallAnswer {
    match part {
        jig_core::CallPart::Effect { .. } => match domain.connector_calls.get(&key) {
            Some(answer) => answer.clone(),
            None => CallAnswer::from_core(part).expect("core effect status"),
        },
        jig_core::CallPart::Connector { .. }
        | jig_core::CallPart::EffectDenied { .. }
        | jig_core::CallPart::ToolDenied { .. }
        | jig_core::CallPart::EscalationDecided { .. }
        | jig_core::CallPart::EscalationRefused(_)
        | jig_core::CallPart::Proposed { .. }
        | jig_core::CallPart::ProposalDecided { .. }
        | jig_core::CallPart::ProposalRefused(_)
        | jig_core::CallPart::Controlled
        | jig_core::CallPart::ControlRefused(_)
        | jig_core::CallPart::ControlDenied { .. }
        | jig_core::CallPart::Sent { .. }
        | jig_core::CallPart::Introduced
        | jig_core::CallPart::MessageRefused(_)
        | jig_core::CallPart::Subscribed { .. }
        | jig_core::CallPart::Unsubscribed
        | jig_core::CallPart::SubscriptionRefused(_)
        | jig_core::CallPart::Delegated(_)
        | jig_core::CallPart::DelegationDenied { .. }
        | jig_core::CallPart::DelegationRefused(_)
        | jig_core::CallPart::NoteWritten { .. }
        | jig_core::CallPart::NoteRecalled { .. }
        | jig_core::CallPart::NoteRefused(_)
        | jig_core::CallPart::Unavailable => CallAnswer::from_core(part).expect("core owns its answer"),
    }
}

/// The root only translates each handoff that the core selected.
#[expect(clippy::too_many_lines, reason = "one exhaustive core effect handoff translator")]
pub(super) fn effect_ask(domain: &mut Domain, env: &Env<Limits>, number: u16, ask: jig_core::connector::Ask) {
    match ask {
        jig_core::connector::Ask::Describe { owner } => {
            let description = match domain.forge_effects.get(&owner) {
                Some(
                    EffectFlight::Call { description, .. }
                    | EffectFlight::Procedure { description, .. }
                    | EffectFlight::Projection { description, .. },
                ) => description.clone(),
                None => return,
            };
            domain.work.push(Work::Core(jig_core::Event::EffectConnector(jig_core::connector::Event::Described {
                owner,
                description,
            })));
        }
        jig_core::connector::Ask::DescribeProposal { owner, proposal } => {
            let flight = match domain.forge.proposed_effect(proposal) {
                Some(row) => {
                    let key = match domain.core.tasks.proposal(row.entry.task, proposal) {
                        Some(pending) => match pending.action {
                            tasks::ProposalAction::Effect { attempt, completion, position, .. } => {
                                CallKey { task: pending.proposer, attempt, completion, position }
                            }
                            tasks::ProposalAction::Batch(_)
                            | tasks::ProposalAction::Amend { .. }
                            | tasks::ProposalAction::Widen { .. }
                            | tasks::ProposalAction::Release { .. } => return,
                        },
                        None => return,
                    };
                    describe_agent(
                        domain,
                        env,
                        key,
                        row.entry.repository,
                        row.resource.clone(),
                        row.entry.effect.write.clone(),
                    )
                }
                None => Err(forge_client::api::Error::Missing),
            };
            match flight {
                Ok(flight) => {
                    assert!(domain.forge_effects.insert(owner, flight).is_ok(), "fresh proposal description");
                    effect_ask(domain, env, number, jig_core::connector::Ask::Describe { owner });
                }
                Err(why) => domain.work.push(Work::Core(jig_core::Event::EffectConnector(
                    if why == forge_client::api::Error::Busy {
                        jig_core::connector::Event::DescribeBusy { owner }
                    } else {
                        jig_core::connector::Event::DescribeRefused { owner }
                    },
                ))),
            }
        }
        jig_core::connector::Ask::KeepProposal { owner, proposal, task } => match domain.forge_effects.remove(&owner) {
            Some(EffectFlight::Call { repository, effect, resource, .. }) => {
                domain.work.push(Work::Forge(forge::Event::KeepProposedEffect {
                    row: forge::ProposedEffect {
                        number: proposal,
                        resource,
                        entry: forge_client::Entry {
                            number: 0,
                            task,
                            repository,
                            effect,
                            start: None,
                            attempt: None,
                            failures: 0,
                        },
                    },
                }));
            }
            Some(EffectFlight::Procedure { .. } | EffectFlight::Projection { .. }) | None => {
                unreachable!("agent effect proposal was described")
            }
        },
        jig_core::connector::Ask::DropProposal { proposal } => {
            domain.work.push(Work::Forge(forge::Event::DropProposedEffect { number: proposal }));
        }
        jig_core::connector::Ask::Keep { owner, entry, task, key: core_key } => {
            match domain.forge_effects.remove(&owner) {
                Some(EffectFlight::Call { repository, effect, key, description, .. }) => {
                    assert!(
                        core_key
                            == jig_core::EffectKey {
                                deployment: domain.core.counters.deployment().id,
                                task: key.task,
                                origin: jig_core::EffectPurpose::Call {
                                    attempt: key.attempt,
                                    completion: key.completion,
                                    position: key.position,
                                },
                                purpose: description.purpose,
                            },
                        "core preserves the connector's original call purpose"
                    );
                    assert!(
                        domain.forge_effecting.insert(entry, (ReplyTo::new(owner), key)).is_ok(),
                        "admitted connector keep"
                    );
                    domain.work.push(Work::Forge(forge::Event::Enqueue {
                        entry: forge_client::Entry {
                            number: entry,
                            task,
                            repository,
                            effect,
                            start: None,
                            attempt: None,
                            failures: 0,
                        },
                    }));
                }
                Some(EffectFlight::Projection { row, entry: kept, .. }) => {
                    assert!(kept.number == entry && kept.task == task, "admitted projection entry");
                    domain.work.push(Work::Forge(forge::Event::KeepProjection { row: *row, entry: kept }));
                }
                Some(EffectFlight::Procedure { entry: kept, .. }) => {
                    assert!(kept == entry, "core releases the described procedure entry");
                }
                None => {}
            }
        }
        jig_core::connector::Ask::Drop { owner, answer } => match domain.forge_effects.remove(&owner) {
            Some(EffectFlight::Procedure { entry, task, evidence, .. }) => {
                domain.work.push(Work::Forge(forge::Event::VetoChange { task, entry, prior: evidence.prior }));
                if answer == authority::Answer::Wait {
                    let when = skein_lib::Wall::from_nanos(
                        env.wall.as_nanos().saturating_add(env.limits.forge.queue_window.as_nanos()),
                    );
                    domain.forge_change_due.insert(task, when).expect("one timer per procedure");
                } else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                }
            }
            Some(EffectFlight::Call { .. } | EffectFlight::Projection { .. }) | None => {}
        },
        jig_core::connector::Ask::Judge { owner, judge, state, resources: _ } => {
            let (verdict, guarded) = judge_effect(domain, env, owner, number, judge, state);
            domain.work.push(Work::Core(jig_core::Event::EffectConnector(jig_core::connector::Event::Verdict {
                owner,
                judge,
                verdict,
                at: env.wall,
                guarded,
                state,
            })));
        }
        jig_core::connector::Ask::Names { .. }
        | jig_core::connector::Ask::Unname { .. }
        | jig_core::connector::Ask::Hold { .. }
        | jig_core::connector::Ask::ReleaseHold { .. }
        | jig_core::connector::Ask::Writer { .. }
        | jig_core::connector::Ask::Make { .. }
        | jig_core::connector::Ask::ProcedureMade { .. }
        | jig_core::connector::Ask::ProcedureActivate { .. }
        | jig_core::connector::Ask::ProcedureMessage { .. }
        | jig_core::connector::Ask::ProcedureClose { .. }
        | jig_core::connector::Ask::Project { .. }
        | jig_core::connector::Ask::Release { .. }
        | jig_core::connector::Ask::Subscribe { .. }
        | jig_core::connector::Ask::Unsubscribe { .. }
        | jig_core::connector::Ask::Read { .. }
        | jig_core::connector::Ask::Gather { .. }
        | jig_core::connector::Ask::CutTo { .. }
        | jig_core::connector::Ask::Take { .. }
        | jig_core::connector::Ask::Prepare { .. }
        | jig_core::connector::Ask::Left { .. }
        | jig_core::connector::Ask::Adopt { .. }
        | jig_core::connector::Ask::Restore { .. }
        | jig_core::connector::Ask::ReadAfresh
        | jig_core::connector::Ask::SettleOutbox => unreachable!("core effect handoff"),
    }
}

fn judge_effect(
    domain: &Domain,
    env: &Env<Limits>,
    owner: Token,
    number: u16,
    judge: authority::Judge,
    state: [u8; 32],
) -> (authority::Verdict, bool) {
    if number != domain.config.forge_connector {
        return (authority::Verdict::Wait, false);
    }
    match domain.forge_effects.get(&owner) {
        Some(EffectFlight::Procedure { task, evidence, description, .. }) => {
            let Some(row) = domain.forge.change(*task) else { return (authority::Verdict::Refuse, false) };
            let Some(repository) = domain.forge.repository(row.repository) else {
                return (authority::Verdict::Refuse, false);
            };
            let Some(reviewers) = landing_reviewers(domain, env, row, evidence) else {
                return (authority::Verdict::Refuse, false);
            };
            let guarded = description.effect.kind == 4
                && domain.forge.guards_landing(repository.project, judge.requirement, judge.parameters);
            let verdict = domain.forge.judge_landing(
                repository.project,
                judge.requirement,
                judge.parameters,
                *task,
                state,
                evidence,
                &reviewers,
            );
            let verdict = match verdict {
                Some(forge::JudgeVerdict::Met) => authority::Verdict::Met,
                Some(forge::JudgeVerdict::Wait) | None => authority::Verdict::Wait,
                Some(forge::JudgeVerdict::Refuse) => authority::Verdict::Refuse,
            };
            (verdict, guarded)
        }
        Some(EffectFlight::Call { .. } | EffectFlight::Projection { .. }) | None => (authority::Verdict::Wait, false),
    }
}
