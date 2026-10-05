//! Typed v1 payload translation. The channel owns every byte schema.
use alloc::boxed::Box;
use skein_lib::{List, Time};
use temper_channel::{Sizes, payload::v1 as wire};
use temper_legacy_engine_domain::{self as engine, brief, forge, notes, plan, views};

pub(crate) fn charter_to(value: engine::Charter) -> Option<wire::Charter> {
    let mut brief = List::with_capacity(u32::try_from(value.brief.len()).ok()?);
    for entry in value.brief {
        brief.push(section_to(entry)).expect("capacity is the source array length");
    }
    let brief = brief.into_boxed();
    let mut models = List::with_capacity(u32::try_from(value.models.len()).ok()?);
    for entry in value.models {
        models.push(model_to(entry)).expect("capacity is the source array length");
    }
    let models = models.into_boxed();
    Some(wire::Charter {
        why: why_to(value.why),
        brief,
        instructions: value.instructions,
        grants: permissions_to(value.grants),
        finish: finish_to(value.finish),
        budget: budget_to(value.budget),
        models,
        policy: policy_to(value.policy),
    })
}

pub(crate) fn charter_from(value: wire::Charter) -> Option<engine::Charter> {
    let mut brief = List::with_capacity(u32::try_from(value.brief.len()).ok()?);
    for entry in value.brief {
        brief.push(section_from(entry)).expect("capacity is the source array length");
    }
    let brief = brief.into_boxed();
    let mut models = List::with_capacity(u32::try_from(value.models.len()).ok()?);
    for entry in value.models {
        models.push(model_from(entry)).expect("capacity is the source array length");
    }
    let models = models.into_boxed();
    Some(engine::Charter {
        why: why_from(value.why),
        brief,
        instructions: value.instructions,
        grants: permissions_from(value.grants),
        finish: finish_from(value.finish),
        budget: budget_from(value.budget),
        models,
        policy: policy_from(value.policy),
    })
}

pub(crate) fn plan_charter_to(value: plan::Charter) -> wire::PlanCharter {
    wire::PlanCharter {
        instructions: value.instructions,
        template: value.template,
        grants: permissions_to(value.grants),
        budget: budget_to(value.budget),
    }
}

pub(crate) fn plan_charter_from(value: wire::PlanCharter) -> plan::Charter {
    plan::Charter {
        instructions: value.instructions,
        template: value.template,
        grants: permissions_from(value.grants),
        budget: budget_from(value.budget),
    }
}

pub(crate) fn section_to(value: brief::Section) -> wire::Section {
    wire::Section { kind: section_kind_to(value.kind), body: section_body_to(value.body) }
}

pub(crate) fn section_from(value: wire::Section) -> brief::Section {
    brief::Section { kind: section_kind_from(value.kind), body: section_body_from(value.body) }
}

pub(crate) fn permissions_to(value: plan::Grants) -> wire::Permissions {
    wire::Permissions {
        modify: value.modify,
        shell: value.shell,
        forge: value.forge,
        subagents: value.subagents,
        note: value.note,
    }
}

pub(crate) fn permissions_from(value: wire::Permissions) -> plan::Grants {
    plan::Grants {
        modify: value.modify,
        shell: value.shell,
        forge: value.forge,
        subagents: value.subagents,
        note: value.note,
    }
}

pub(crate) fn budget_to(value: plan::Budget) -> wire::Budget {
    wire::Budget { tokens: value.tokens, turns: value.turns, time: value.time }
}

pub(crate) fn budget_from(value: wire::Budget) -> plan::Budget {
    plan::Budget { tokens: value.tokens, turns: value.turns, time: value.time }
}

pub(crate) fn model_to(value: engine::Model) -> wire::Model {
    wire::Model { endpoint: value.endpoint, model: value.model, max_tokens: value.max_tokens }
}

pub(crate) fn model_from(value: wire::Model) -> engine::Model {
    engine::Model { endpoint: value.endpoint, model: value.model, max_tokens: value.max_tokens }
}

pub(crate) fn policy_to(value: views::Policy) -> wire::Policy {
    wire::Policy {
        text: capture_to(value.text),
        progress: capture_to(value.progress),
        calls: capture_to(value.calls),
        tools: capture_to(value.tools),
        usage: capture_to(value.usage),
    }
}

pub(crate) fn policy_from(value: wire::Policy) -> views::Policy {
    views::Policy {
        text: capture_from(value.text),
        progress: capture_from(value.progress),
        calls: capture_from(value.calls),
        tools: capture_from(value.tools),
        usage: capture_from(value.usage),
    }
}

pub(crate) fn plan_to(value: plan::Plan) -> Option<wire::Plan> {
    let mut steps = List::with_capacity(u32::try_from(value.steps.len()).ok()?);
    for entry in value.steps {
        steps.push(step_to(entry)?).expect("capacity is the source array length");
    }
    let steps = steps.into_boxed();
    Some(wire::Plan { steps, envelope: envelope_to(value.envelope)?, budget: value.budget })
}

pub(crate) fn plan_from(value: wire::Plan) -> Option<plan::Plan> {
    let mut steps = List::with_capacity(u32::try_from(value.steps.len()).ok()?);
    for entry in value.steps {
        steps.push(step_from(entry)?).expect("capacity is the source array length");
    }
    let steps = steps.into_boxed();
    Some(plan::Plan { steps, envelope: envelope_from(value.envelope)?, budget: value.budget })
}

pub(crate) fn step_to(value: plan::Step) -> Option<wire::Step> {
    let mut gates = List::with_capacity(u32::try_from(value.gates.len()).ok()?);
    for entry in value.gates {
        gates.push(gate_to(entry)).expect("capacity is the source array length");
    }
    let gates = gates.into_boxed();
    Some(wire::Step {
        name: value.name,
        repository: repository_to(value.repository),
        work: step_work_to(value.work),
        after: value.after,
        gates,
    })
}

pub(crate) fn step_from(value: wire::Step) -> Option<plan::Step> {
    let mut gates = List::with_capacity(u32::try_from(value.gates.len()).ok()?);
    for entry in value.gates {
        gates.push(gate_from(entry)).expect("capacity is the source array length");
    }
    let gates = gates.into_boxed();
    Some(plan::Step {
        name: value.name,
        repository: repository_from(value.repository),
        work: step_work_from(value.work),
        after: value.after,
        gates,
    })
}

pub(crate) fn envelope_to(value: plan::Envelope) -> Option<wire::Envelope> {
    let mut repositories = List::with_capacity(u32::try_from(value.repositories.len()).ok()?);
    for entry in value.repositories {
        repositories.push(repository_to(entry)).expect("capacity is the source array length");
    }
    let repositories = repositories.into_boxed();
    let mut into = List::with_capacity(u32::try_from(value.into.len()).ok()?);
    for entry in value.into {
        into.push(target_to(entry)).expect("capacity is the source array length");
    }
    let into = into.into_boxed();
    Some(wire::Envelope {
        agents: value.agents,
        changes: value.changes,
        waits: value.waits,
        sessions: value.sessions,
        repositories,
        into,
    })
}

pub(crate) fn envelope_from(value: wire::Envelope) -> Option<plan::Envelope> {
    let mut repositories = List::with_capacity(u32::try_from(value.repositories.len()).ok()?);
    for entry in value.repositories {
        repositories.push(repository_from(entry)).expect("capacity is the source array length");
    }
    let repositories = repositories.into_boxed();
    let mut into = List::with_capacity(u32::try_from(value.into.len()).ok()?);
    for entry in value.into {
        into.push(target_from(entry)).expect("capacity is the source array length");
    }
    let into = into.into_boxed();
    Some(plan::Envelope {
        agents: value.agents,
        changes: value.changes,
        waits: value.waits,
        sessions: value.sessions,
        repositories,
        into,
    })
}

pub(crate) fn target_to(value: plan::Target) -> wire::Target {
    wire::Target { repository: repository_to(value.repository), base: value.base }
}

pub(crate) fn target_from(value: wire::Target) -> plan::Target {
    plan::Target { repository: repository_from(value.repository), base: value.base }
}

pub(crate) fn sources_to(value: plan::Sources) -> wire::Sources {
    wire::Sources { own: value.own, related: value.related, subscribed: value.subscribed, messages: value.messages }
}

pub(crate) fn sources_from(value: wire::Sources) -> plan::Sources {
    plan::Sources { own: value.own, related: value.related, subscribed: value.subscribed, messages: value.messages }
}

pub(crate) fn batch_to(value: plan::Batch) -> wire::Batch {
    wire::Batch { count: value.count, age: value.age }
}

pub(crate) fn batch_from(value: wire::Batch) -> plan::Batch {
    plan::Batch { count: value.count, age: value.age }
}

pub(crate) fn wake_to(value: plan::Wake) -> wire::Wake {
    wire::Wake { on: sources_to(value.on), every: value.every, batch: batch_to(value.batch) }
}

pub(crate) fn wake_from(value: wire::Wake) -> plan::Wake {
    plan::Wake { on: sources_from(value.on), every: value.every, batch: batch_from(value.batch) }
}

pub(crate) fn repair_to(value: plan::Repair) -> wire::Repair {
    match value {
        plan::Repair::CiFailed => wire::Repair::CiFailed,
        plan::Repair::ChangesRequested => wire::Repair::ChangesRequested,
        plan::Repair::BaseMoved => wire::Repair::BaseMoved,
        plan::Repair::Conflicts => wire::Repair::Conflicts,
    }
}

pub(crate) fn repair_from(value: wire::Repair) -> plan::Repair {
    match value {
        wire::Repair::CiFailed => plan::Repair::CiFailed,
        wire::Repair::ChangesRequested => plan::Repair::ChangesRequested,
        wire::Repair::BaseMoved => plan::Repair::BaseMoved,
        wire::Repair::Conflicts => plan::Repair::Conflicts,
    }
}

pub(crate) fn section_kind_to(value: brief::Kind) -> wire::SectionKind {
    match value {
        brief::Kind::Task => unreachable!("the legacy root never assigns a task brief on v1"),
        brief::Kind::Item => wire::SectionKind::Item,
        brief::Kind::Comments => wire::SectionKind::Comments,
        brief::Kind::Dependencies => wire::SectionKind::Dependencies,
        brief::Kind::Ci => wire::SectionKind::Ci,
        brief::Kind::Reviews => wire::SectionKind::Reviews,
        brief::Kind::Pull => wire::SectionKind::Pull,
        brief::Kind::Attempts => wire::SectionKind::Attempts,
        brief::Kind::Plan => wire::SectionKind::Plan,
        brief::Kind::Notes => wire::SectionKind::Notes,
        brief::Kind::Template => wire::SectionKind::Template,
    }
}

pub(crate) fn section_kind_from(value: wire::SectionKind) -> brief::Kind {
    match value {
        wire::SectionKind::Item => brief::Kind::Item,
        wire::SectionKind::Comments => brief::Kind::Comments,
        wire::SectionKind::Dependencies => brief::Kind::Dependencies,
        wire::SectionKind::Ci => brief::Kind::Ci,
        wire::SectionKind::Reviews => brief::Kind::Reviews,
        wire::SectionKind::Pull => brief::Kind::Pull,
        wire::SectionKind::Attempts => brief::Kind::Attempts,
        wire::SectionKind::Plan => brief::Kind::Plan,
        wire::SectionKind::Notes => brief::Kind::Notes,
        wire::SectionKind::Template => brief::Kind::Template,
    }
}

pub(crate) fn unread_to(value: brief::Unread) -> wire::Unread {
    match value {
        brief::Unread::Failed => wire::Unread::Failed,
        brief::Unread::Late => wire::Unread::Late,
        brief::Unread::Oversized => wire::Unread::Oversized,
    }
}

pub(crate) fn unread_from(value: wire::Unread) -> brief::Unread {
    match value {
        wire::Unread::Failed => brief::Unread::Failed,
        wire::Unread::Late => brief::Unread::Late,
        wire::Unread::Oversized => brief::Unread::Oversized,
    }
}

pub(crate) fn capture_to(value: views::Capture) -> wire::Capture {
    match value {
        views::Capture::Nothing => wire::Capture::Nothing,
        views::Capture::Shape => wire::Capture::Shape,
        views::Capture::Content => wire::Capture::Content,
    }
}

pub(crate) fn capture_from(value: wire::Capture) -> views::Capture {
    match value {
        wire::Capture::Nothing => views::Capture::Nothing,
        wire::Capture::Shape => views::Capture::Shape,
        wire::Capture::Content => views::Capture::Content,
    }
}

pub(crate) fn verdict_to(value: plan::Verdict) -> wire::Verdict {
    match value {
        plan::Verdict::Approve => wire::Verdict::Approve,
        plan::Verdict::Changes => wire::Verdict::Changes,
    }
}

pub(crate) fn verdict_from(value: wire::Verdict) -> plan::Verdict {
    match value {
        wire::Verdict::Approve => plan::Verdict::Approve,
        wire::Verdict::Changes => plan::Verdict::Changes,
    }
}

pub(crate) fn resume_to(value: plan::Resume) -> wire::Resume {
    match value {
        plan::Resume::Default => wire::Resume::Default,
        plan::Resume::Always => wire::Resume::Always,
        plan::Resume::Never => wire::Resume::Never,
    }
}

pub(crate) fn resume_from(value: wire::Resume) -> plan::Resume {
    match value {
        wire::Resume::Default => plan::Resume::Default,
        wire::Resume::Always => plan::Resume::Always,
        wire::Resume::Never => plan::Resume::Never,
    }
}

fn repository_to(value: plan::Repository) -> u32 {
    value.0
}
fn repository_from(value: u32) -> plan::Repository {
    plan::Repository(value)
}

fn why_to(value: plan::Why) -> wire::Why {
    match value {
        plan::Why::Work => wire::Why::Work,
        plan::Why::Produce => wire::Why::Produce,
        plan::Why::Repair(value) => wire::Why::Repair { repair: repair_to(value) },
        plan::Why::Review { head } => wire::Why::Review { head: head.0 },
        plan::Why::Turn => wire::Why::Turn,
    }
}
fn why_from(value: wire::Why) -> plan::Why {
    match value {
        wire::Why::Work => plan::Why::Work,
        wire::Why::Produce => plan::Why::Produce,
        wire::Why::Repair { repair } => plan::Why::Repair(repair_from(repair)),
        wire::Why::Review { head } => plan::Why::Review { head: plan::Commit(head) },
        wire::Why::Turn => plan::Why::Turn,
    }
}
fn section_body_to(value: brief::Body) -> wire::SectionBody {
    match value {
        brief::Body::Text(text) => wire::SectionBody::Text { text },
        brief::Body::Missing(unread) => wire::SectionBody::Missing { unread: unread_to(unread) },
    }
}
fn section_body_from(value: wire::SectionBody) -> brief::Body {
    match value {
        wire::SectionBody::Text { text } => brief::Body::Text(text),
        wire::SectionBody::Missing { unread } => brief::Body::Missing(unread_from(unread)),
    }
}
fn finish_to(value: plan::Finish) -> wire::FinishSpec {
    match value {
        plan::Finish::Report { grows } => wire::FinishSpec::Report { grows },
        plan::Finish::Change { checks } => wire::FinishSpec::Change { checks },
        plan::Finish::Verdict => wire::FinishSpec::Verdict,
        plan::Finish::Turn { supervising } => wire::FinishSpec::Turn { supervising },
    }
}
fn finish_from(value: wire::FinishSpec) -> plan::Finish {
    match value {
        wire::FinishSpec::Report { grows } => plan::Finish::Report { grows },
        wire::FinishSpec::Change { checks } => plan::Finish::Change { checks },
        wire::FinishSpec::Verdict => plan::Finish::Verdict,
        wire::FinishSpec::Turn { supervising } => plan::Finish::Turn { supervising },
    }
}
fn gate_to(value: plan::Gate) -> wire::Gate {
    match value {
        plan::Gate::Approvals(count) => wire::Gate::Approvals { count },
        plan::Gate::Accepted => wire::Gate::Accepted,
    }
}
fn gate_from(value: wire::Gate) -> plan::Gate {
    match value {
        wire::Gate::Approvals { count } => plan::Gate::Approvals(count),
        wire::Gate::Accepted => plan::Gate::Accepted,
    }
}
fn review_to(value: plan::Review) -> wire::Review {
    match value {
        plan::Review::Person => wire::Review::Person,
        plan::Review::Agent(charter) => wire::Review::Agent { charter: plan_charter_to(charter) },
    }
}
fn review_from(value: wire::Review) -> plan::Review {
    match value {
        wire::Review::Person => plan::Review::Person,
        wire::Review::Agent { charter } => plan::Review::Agent(plan_charter_from(charter)),
    }
}
fn wait_to(value: plan::WaitSpec) -> wire::WaitSpec {
    match value {
        plan::WaitSpec::Steps => wire::WaitSpec::Steps,
        plan::WaitSpec::Decision => wire::WaitSpec::Decision,
        plan::WaitSpec::Time(duration) => wire::WaitSpec::Time { duration },
    }
}
fn wait_from(value: wire::WaitSpec) -> plan::WaitSpec {
    match value {
        wire::WaitSpec::Steps => plan::WaitSpec::Steps,
        wire::WaitSpec::Decision => plan::WaitSpec::Decision,
        wire::WaitSpec::Time { duration } => plan::WaitSpec::Time(duration),
    }
}
fn step_work_to(value: plan::Work) -> wire::StepWork {
    match value {
        plan::Work::Agent(value) => {
            wire::StepWork::Agent { charter: plan_charter_to(value.charter), grows: value.grows }
        }
        plan::Work::Change(value) => wire::StepWork::Change {
            base: value.base,
            produce: plan_charter_to(value.produce),
            checks: value.checks,
            review: review_to(value.review),
        },
        plan::Work::Wait(wait) => wire::StepWork::Wait { wait: wait_to(wait) },
        plan::Work::Session(value) => wire::StepWork::Session {
            charter: plan_charter_to(value.charter),
            resume: resume_to(value.resume),
            wake: wake_to(value.wake),
        },
    }
}
fn step_work_from(value: wire::StepWork) -> plan::Work {
    match value {
        wire::StepWork::Agent { charter, grows } => {
            plan::Work::Agent(plan::AgentSpec { charter: plan_charter_from(charter), grows })
        }
        wire::StepWork::Change { base, produce, checks, review } => plan::Work::Change(plan::ChangeSpec {
            base,
            produce: plan_charter_from(produce),
            checks,
            review: review_from(review),
        }),
        wire::StepWork::Wait { wait } => plan::Work::Wait(wait_from(wait)),
        wire::StepWork::Session { charter, resume, wake } => plan::Work::Session(plan::SessionSpec {
            charter: plan_charter_from(charter),
            resume: resume_from(resume),
            wake: wake_from(wake),
        }),
    }
}
pub(crate) fn outcome_to(value: engine::Outcome) -> Option<wire::Outcome> {
    Some(match value {
        engine::Outcome::Change { message } => wire::Outcome::Change { message },
        engine::Outcome::Verdict { verdict, text } => wire::Outcome::Verdict { verdict: verdict_to(verdict), text },
        engine::Outcome::Report { text } => wire::Outcome::Report { text },
        engine::Outcome::Plan { plan, text } => wire::Outcome::Plan { plan: plan_to(plan)?, text },
        engine::Outcome::Steps { steps, text } => {
            let mut found = List::with_capacity(u32::try_from(steps.len()).ok()?);
            for value in steps {
                found.push(step_to(value)?).expect("source length");
            }
            wire::Outcome::Steps { steps: found.into_boxed(), text }
        }
        engine::Outcome::Tasks { tasks, text } => {
            let mut found = List::with_capacity(u32::try_from(tasks.len()).ok()?);
            for value in tasks {
                found.push(step_to(value)?).expect("source length");
            }
            wire::Outcome::Tasks { tasks: found.into_boxed(), text }
        }
        engine::Outcome::Reply { text } => wire::Outcome::Reply { text },
        engine::Outcome::Finished { text } => wire::Outcome::Finished { text },
        engine::Outcome::Release { step, text } => wire::Outcome::Release { step, text },
        engine::Outcome::Escalation { text } => wire::Outcome::Escalation { text },
    })
}

pub(crate) fn outcome_from(value: wire::Outcome) -> Option<engine::Outcome> {
    Some(match value {
        wire::Outcome::Change { message } => engine::Outcome::Change { message },
        wire::Outcome::Verdict { verdict, text } => engine::Outcome::Verdict { verdict: verdict_from(verdict), text },
        wire::Outcome::Report { text } => engine::Outcome::Report { text },
        wire::Outcome::Plan { plan, text } => engine::Outcome::Plan { plan: plan_from(plan)?, text },
        wire::Outcome::Steps { steps, text } => {
            let mut found = List::with_capacity(u32::try_from(steps.len()).ok()?);
            for value in steps {
                found.push(step_from(value)?).expect("source length");
            }
            engine::Outcome::Steps { steps: found.into_boxed(), text }
        }
        wire::Outcome::Tasks { tasks, text } => {
            let mut found = List::with_capacity(u32::try_from(tasks.len()).ok()?);
            for value in tasks {
                found.push(step_from(value)?).expect("source length");
            }
            engine::Outcome::Tasks { tasks: found.into_boxed(), text }
        }
        wire::Outcome::Reply { text } => engine::Outcome::Reply { text },
        wire::Outcome::Finished { text } => engine::Outcome::Finished { text },
        wire::Outcome::Release { step, text } => engine::Outcome::Release { step, text },
        wire::Outcome::Escalation { text } => engine::Outcome::Escalation { text },
    })
}

#[must_use]
pub fn encode_charter(value: &engine::Charter, sizes: &Sizes) -> Option<Box<[u8]>> {
    wire::encode_charter(&charter_to(value.clone())?, sizes)
}
#[must_use]
pub fn decode_charter(bytes: &[u8], sizes: &Sizes) -> Option<engine::Charter> {
    charter_from(wire::decode_charter(bytes, sizes)?)
}
#[must_use]
pub fn encode_outcome(value: &engine::Outcome, sizes: &Sizes) -> Option<Box<[u8]>> {
    wire::encode_outcome(&outcome_to(value.clone())?, sizes)
}
#[must_use]
pub fn decode_outcome(bytes: &[u8], sizes: &Sizes) -> Option<engine::Outcome> {
    outcome_from(wire::decode_outcome(bytes, sizes)?)
}

use forge::api;
pub(crate) fn item_to(value: engine::Item) -> wire::Item {
    wire::Item { repository: value.repository, number: value.number }
}

pub(crate) fn item_from(value: wire::Item) -> engine::Item {
    engine::Item { repository: value.repository, number: value.number }
}

pub(crate) fn forge_item_to(value: forge::Item) -> wire::Item {
    wire::Item { repository: value.repository, number: value.number }
}

pub(crate) fn forge_item_from(value: wire::Item) -> forge::Item {
    forge::Item { repository: value.repository, number: value.number }
}

pub(crate) fn note_item_to(value: notes::Item) -> wire::Item {
    wire::Item { repository: value.repository, number: value.number }
}

pub(crate) fn note_item_from(value: wire::Item) -> notes::Item {
    notes::Item { repository: value.repository, number: value.number }
}

pub(crate) fn reference_to(value: notes::Reference) -> wire::Item {
    wire::Item { repository: value.repository, number: value.number }
}

pub(crate) fn reference_from(value: wire::Item) -> notes::Reference {
    notes::Reference { repository: value.repository, number: value.number }
}

pub(crate) fn scopes_to(value: notes::Scopes) -> wire::Scopes {
    let mut goal = None;
    if let Some(value) = value.goal {
        goal = Some(note_item_to(value));
    }
    wire::Scopes { repository: value.repository, goal }
}

pub(crate) fn scopes_from(value: wire::Scopes) -> notes::Scopes {
    let mut goal = None;
    if let Some(value) = value.goal {
        goal = Some(note_item_from(value));
    }
    notes::Scopes { repository: value.repository, goal }
}

pub(crate) fn page_to(value: notes::Page) -> Option<wire::Page> {
    let mut references = List::with_capacity(u32::try_from(value.references.len()).ok()?);
    for entry in value.references {
        references.push(reference_to(entry)).expect("capacity is the source array length");
    }
    let references = references.into_boxed();
    Some(wire::Page { description: value.description, author: author_to(value.author), references, body: value.body })
}

pub(crate) fn page_from(value: wire::Page) -> Option<notes::Page> {
    let mut references = List::with_capacity(u32::try_from(value.references.len()).ok()?);
    for entry in value.references {
        references.push(reference_from(entry)).expect("capacity is the source array length");
    }
    let references = references.into_boxed();
    Some(notes::Page {
        description: value.description,
        author: author_from(value.author),
        references,
        body: value.body,
    })
}

pub(crate) fn entry_to(value: notes::Entry) -> Option<wire::Entry> {
    Some(wire::Entry {
        scope: scope_to(value.scope),
        name: value.name,
        revision: value.revision,
        page: page_to(value.page)?,
    })
}

pub(crate) fn entry_from(value: wire::Entry) -> Option<notes::Entry> {
    Some(notes::Entry {
        scope: scope_from(value.scope),
        name: value.name,
        revision: value.revision,
        page: page_from(value.page)?,
    })
}

pub(crate) fn position_to(value: forge::Position) -> wire::Position {
    wire::Position {
        comment: value.comment,
        pull_comment: value.pull_comment,
        reviews: value.reviews,
        head: value.head,
        ci: ci_to(value.ci),
    }
}

pub(crate) fn position_from(value: wire::Position) -> forge::Position {
    forge::Position {
        comment: value.comment,
        pull_comment: value.pull_comment,
        reviews: value.reviews,
        head: value.head,
        ci: ci_from(value.ci),
    }
}

pub(crate) fn summary_to(value: api::Summary) -> wire::Summary {
    wire::Summary {
        number: value.number,
        kind: forge_kind_to(value.kind),
        state: state_to(value.state),
        author: value.author,
        key: value.key,
        labels: value.labels,
        title: value.title,
        body: value.body,
        updated: time_to(value.updated),
    }
}

pub(crate) fn summary_from(value: wire::Summary) -> api::Summary {
    api::Summary {
        number: value.number,
        kind: forge_kind_from(value.kind),
        state: state_from(value.state),
        author: value.author,
        key: value.key,
        labels: value.labels,
        title: value.title,
        body: value.body,
        updated: time_from(value.updated),
    }
}

pub(crate) fn comment_to(value: api::Comment) -> wire::Comment {
    wire::Comment {
        id: value.id,
        author: value.author,
        created: time_to(value.created),
        revision: value.revision,
        mark: mark_to(value.mark),
        body: value.body,
    }
}

pub(crate) fn comment_from(value: wire::Comment) -> api::Comment {
    api::Comment {
        id: value.id,
        author: value.author,
        created: time_from(value.created),
        revision: value.revision,
        mark: mark_from(value.mark),
        body: value.body,
    }
}

pub(crate) fn pull_to(value: api::Pull) -> wire::Pull {
    wire::Pull {
        number: value.number,
        state: state_to(value.state),
        head: value.head,
        base: value.base,
        commit: value.commit,
        base_commit: value.base_commit,
        merged: value.merged,
        mergeable: value.mergeable,
        ci: ci_to(value.ci),
    }
}

pub(crate) fn pull_from(value: wire::Pull) -> api::Pull {
    api::Pull {
        number: value.number,
        state: state_from(value.state),
        head: value.head,
        base: value.base,
        commit: value.commit,
        base_commit: value.base_commit,
        merged: value.merged,
        mergeable: value.mergeable,
        ci: ci_from(value.ci),
    }
}

pub(crate) fn forge_review_to(value: api::Review) -> wire::ForgeReview {
    wire::ForgeReview {
        id: value.id,
        author: value.author,
        verdict: forge_verdict_to(value.verdict),
        commit: value.commit,
        key: value.key,
        body: value.body,
    }
}

pub(crate) fn forge_review_from(value: wire::ForgeReview) -> api::Review {
    api::Review {
        id: value.id,
        author: value.author,
        verdict: forge_verdict_from(value.verdict),
        commit: value.commit,
        key: value.key,
        body: value.body,
    }
}

pub(crate) fn remark_to(value: api::Remark) -> wire::Remark {
    wire::Remark { id: value.id, author: value.author, path: value.path, line: value.line, body: value.body }
}

pub(crate) fn remark_from(value: wire::Remark) -> api::Remark {
    api::Remark { id: value.id, author: value.author, path: value.path, line: value.line, body: value.body }
}

pub(crate) fn status_to(value: api::Status) -> wire::Status {
    wire::Status {
        context: value.context,
        check: check_to(value.check),
        description: value.description,
        url: value.url,
    }
}

pub(crate) fn status_from(value: wire::Status) -> api::Status {
    api::Status {
        context: value.context,
        check: check_from(value.check),
        description: value.description,
        url: value.url,
    }
}

pub(crate) fn forge_page_to(value: api::Page) -> wire::ForgePage {
    wire::ForgePage { name: value.name, content: value.content, revision: value.revision, nonce: value.nonce }
}

pub(crate) fn forge_page_from(value: wire::ForgePage) -> api::Page {
    api::Page { name: value.name, content: value.content, revision: value.revision, nonce: value.nonce }
}

pub(crate) fn page_name_to(value: api::PageName) -> wire::PageName {
    wire::PageName { name: value.name, revision: value.revision }
}

pub(crate) fn page_name_from(value: wire::PageName) -> api::PageName {
    api::PageName { name: value.name, revision: value.revision }
}

pub(crate) fn ci_to(value: forge::Ci) -> wire::Ci {
    match value {
        forge::Ci::None => wire::Ci::None_,
        forge::Ci::Pending => wire::Ci::Pending,
        forge::Ci::Passed => wire::Ci::Passed,
        forge::Ci::Failed => wire::Ci::Failed,
    }
}

pub(crate) fn ci_from(value: wire::Ci) -> forge::Ci {
    match value {
        wire::Ci::None_ => forge::Ci::None,
        wire::Ci::Pending => forge::Ci::Pending,
        wire::Ci::Passed => forge::Ci::Passed,
        wire::Ci::Failed => forge::Ci::Failed,
    }
}

pub(crate) fn forge_kind_to(value: api::Kind) -> wire::ForgeKind {
    match value {
        api::Kind::Issue => wire::ForgeKind::Issue,
        api::Kind::Pull => wire::ForgeKind::Pull,
    }
}

pub(crate) fn forge_kind_from(value: wire::ForgeKind) -> api::Kind {
    match value {
        wire::ForgeKind::Issue => api::Kind::Issue,
        wire::ForgeKind::Pull => api::Kind::Pull,
    }
}

pub(crate) fn state_to(value: api::State) -> wire::State {
    match value {
        api::State::Open => wire::State::Open,
        api::State::Closed => wire::State::Closed,
    }
}

pub(crate) fn state_from(value: wire::State) -> api::State {
    match value {
        wire::State::Open => api::State::Open,
        wire::State::Closed => api::State::Closed,
    }
}

pub(crate) fn permission_to(value: api::Permission) -> wire::Permission {
    match value {
        api::Permission::None => wire::Permission::None_,
        api::Permission::Read => wire::Permission::Read,
        api::Permission::Write => wire::Permission::Write,
        api::Permission::Admin => wire::Permission::Admin,
    }
}

pub(crate) fn permission_from(value: wire::Permission) -> api::Permission {
    match value {
        wire::Permission::None_ => api::Permission::None,
        wire::Permission::Read => api::Permission::Read,
        wire::Permission::Write => api::Permission::Write,
        wire::Permission::Admin => api::Permission::Admin,
    }
}

pub(crate) fn forge_verdict_to(value: api::Verdict) -> wire::ForgeVerdict {
    match value {
        api::Verdict::Approve => wire::ForgeVerdict::Approve,
        api::Verdict::RequestChanges => wire::ForgeVerdict::RequestChanges,
        api::Verdict::Comment => wire::ForgeVerdict::Comment,
    }
}

pub(crate) fn forge_verdict_from(value: wire::ForgeVerdict) -> api::Verdict {
    match value {
        wire::ForgeVerdict::Approve => api::Verdict::Approve,
        wire::ForgeVerdict::RequestChanges => api::Verdict::RequestChanges,
        wire::ForgeVerdict::Comment => api::Verdict::Comment,
    }
}

pub(crate) fn check_to(value: api::Check) -> wire::Check {
    match value {
        api::Check::Pending => wire::Check::Pending,
        api::Check::Passed => wire::Check::Passed,
        api::Check::Failed => wire::Check::Failed,
    }
}

pub(crate) fn check_from(value: wire::Check) -> api::Check {
    match value {
        wire::Check::Pending => api::Check::Pending,
        wire::Check::Passed => api::Check::Passed,
        wire::Check::Failed => api::Check::Failed,
    }
}

pub(crate) fn noted_to(value: notes::Noted) -> wire::Noted {
    match value {
        notes::Noted::Done => wire::Noted::Done,
        notes::Noted::Missing => wire::Noted::Missing,
        notes::Noted::Exists => wire::Noted::Exists,
        notes::Noted::Moved => wire::Noted::Moved,
        notes::Noted::Unavailable => wire::Noted::Unavailable,
    }
}

pub(crate) fn noted_from(value: wire::Noted) -> notes::Noted {
    match value {
        wire::Noted::Done => notes::Noted::Done,
        wire::Noted::Missing => notes::Noted::Missing,
        wire::Noted::Exists => notes::Noted::Exists,
        wire::Noted::Moved => notes::Noted::Moved,
        wire::Noted::Unavailable => notes::Noted::Unavailable,
    }
}

pub(crate) fn unserved_to(value: engine::Unserved) -> wire::Unserved {
    match value {
        engine::Unserved::Ungranted => wire::Unserved::Ungranted,
        engine::Unserved::Busy => wire::Unserved::Busy,
        engine::Unserved::Invalid => wire::Unserved::Invalid,
        engine::Unserved::Refused => wire::Unserved::Refused,
        engine::Unserved::Failed => wire::Unserved::Failed,
    }
}

pub(crate) fn unserved_from(value: wire::Unserved) -> engine::Unserved {
    match value {
        wire::Unserved::Ungranted => engine::Unserved::Ungranted,
        wire::Unserved::Busy => engine::Unserved::Busy,
        wire::Unserved::Invalid => engine::Unserved::Invalid,
        wire::Unserved::Refused => engine::Unserved::Refused,
        wire::Unserved::Failed => engine::Unserved::Failed,
    }
}

pub(crate) fn scope_to(value: notes::Scope) -> wire::Scope {
    match value {
        notes::Scope::Deployment => wire::Scope::Deployment,
        notes::Scope::Repository(repository) => wire::Scope::Repository { repository },
        notes::Scope::Goal { repository, number } => wire::Scope::Goal { repository, number },
    }
}

pub(crate) fn scope_from(value: wire::Scope) -> notes::Scope {
    match value {
        wire::Scope::Deployment => notes::Scope::Deployment,
        wire::Scope::Repository { repository } => notes::Scope::Repository(repository),
        wire::Scope::Goal { repository, number } => notes::Scope::Goal { repository, number },
    }
}

pub(crate) fn author_to(value: notes::Author) -> wire::Author {
    match value {
        notes::Author::Person(person) => wire::Author::Person { person },
        notes::Author::Run { repository, number } => wire::Author::Run { repository, number },
    }
}

pub(crate) fn author_from(value: wire::Author) -> notes::Author {
    match value {
        wire::Author::Person { person } => notes::Author::Person(person),
        wire::Author::Run { repository, number } => notes::Author::Run { repository, number },
    }
}

pub(crate) fn recall_to(value: notes::Recall) -> wire::Recall {
    match value {
        notes::Recall::Name { scope, name } => wire::Recall::Name { scope: scope_to(scope), name },
        notes::Recall::Search { scopes, query, most } => {
            wire::Recall::Search { scopes: scopes_to(scopes), query, most }
        }
    }
}

pub(crate) fn recall_from(value: wire::Recall) -> notes::Recall {
    match value {
        wire::Recall::Name { scope, name } => notes::Recall::Name { scope: scope_from(scope), name },
        wire::Recall::Search { scopes, query, most } => {
            notes::Recall::Search { scopes: scopes_from(scopes), query, most }
        }
    }
}

pub(crate) fn change_to(value: notes::Change) -> Option<wire::Change> {
    Some(match value {
        notes::Change::New(page) => wire::Change::New { page: page_to(page)? },
        notes::Change::Revise { page, revision } => wire::Change::Revise { page: page_to(page)?, revision },
        notes::Change::Remove => wire::Change::Remove,
    })
}

pub(crate) fn change_from(value: wire::Change) -> Option<notes::Change> {
    Some(match value {
        wire::Change::New { page } => notes::Change::New(page_from(page)?),
        wire::Change::Revise { page, revision } => notes::Change::Revise { page: page_from(page)?, revision },
        wire::Change::Remove => notes::Change::Remove,
    })
}

pub(crate) fn mark_to(value: api::Mark) -> wire::Mark {
    match value {
        api::Mark::None => wire::Mark::None_,
        api::Mark::Key { key, person } => wire::Mark::Key { key, person },
        api::Mark::Record { position, nonce } => wire::Mark::Record { position: position_to(position), nonce },
        api::Mark::Mangled => wire::Mark::Mangled,
    }
}

pub(crate) fn mark_from(value: wire::Mark) -> api::Mark {
    match value {
        wire::Mark::None_ => api::Mark::None,
        wire::Mark::Key { key, person } => api::Mark::Key { key, person },
        wire::Mark::Record { position, nonce } => api::Mark::Record { position: position_from(position), nonce },
        wire::Mark::Mangled => api::Mark::Mangled,
    }
}

pub(crate) fn news_to(value: forge::News) -> wire::News {
    match value {
        forge::News::Comment { on, id, author } => wire::News::Comment { on, id, author },
        forge::News::Reviews { commit } => wire::News::Reviews { commit },
        forge::News::Pull { commit, ci, open, merged, mergeable } => {
            wire::News::Pull { commit, ci: ci_to(ci), open, merged, mergeable }
        }
    }
}

pub(crate) fn news_from(value: wire::News) -> forge::News {
    match value {
        wire::News::Comment { on, id, author } => forge::News::Comment { on, id, author },
        wire::News::Reviews { commit } => forge::News::Reviews { commit },
        wire::News::Pull { commit, ci, open, merged, mergeable } => {
            forge::News::Pull { commit, ci: ci_from(ci), open, merged, mergeable }
        }
    }
}

pub(crate) fn inbound_to(value: engine::Inbound) -> wire::Inbound {
    match value {
        engine::Inbound::News(news) => wire::Inbound::News { news: news_to(news) },
        engine::Inbound::Finished { item } => wire::Inbound::Finished { item: item_to(item) },
        engine::Inbound::Held { item } => wire::Inbound::Held { item: item_to(item) },
        engine::Inbound::Decided { accepted } => wire::Inbound::Decided { accepted },
    }
}

pub(crate) fn inbound_from(value: wire::Inbound) -> engine::Inbound {
    match value {
        wire::Inbound::News { news } => engine::Inbound::News(news_from(news)),
        wire::Inbound::Finished { item } => engine::Inbound::Finished { item: item_from(item) },
        wire::Inbound::Held { item } => engine::Inbound::Held { item: item_from(item) },
        wire::Inbound::Decided { accepted } => engine::Inbound::Decided { accepted },
    }
}

pub(crate) fn call_to(value: engine::Call) -> Option<wire::Call> {
    Some(match value {
        engine::Call::Read(read) => wire::Call::Read { read: read_to(read) },
        engine::Call::Recall(recall) => wire::Call::Recall { recall: recall_to(recall) },
        engine::Call::Note { scope, name, change } => {
            wire::Call::Note { scope: scope_to(scope), name, change: change_to(change)? }
        }
        engine::Call::Comment { text } => wire::Call::Comment { text },
        engine::Call::Escalate { text } => wire::Call::Escalate { text },
    })
}

pub(crate) fn call_from(value: wire::Call) -> Option<engine::Call> {
    Some(match value {
        wire::Call::Read { read } => engine::Call::Read(read_from(read)),
        wire::Call::Recall { recall } => engine::Call::Recall(recall_from(recall)),
        wire::Call::Note { scope, name, change } => {
            engine::Call::Note { scope: scope_from(scope), name, change: change_from(change)? }
        }
        wire::Call::Comment { text } => engine::Call::Comment { text },
        wire::Call::Escalate { text } => engine::Call::Escalate { text },
    })
}

pub(crate) fn read_to(value: forge::Read) -> wire::Read {
    match value {
        forge::Read::Item { item, after } => wire::Read::Item { item: forge_item_to(item), after },
        forge::Read::Pull { item } => wire::Read::Pull { item: forge_item_to(item) },
        forge::Read::Reviews { item, page } => wire::Read::Reviews { item: forge_item_to(item), page },
        forge::Read::Remarks { item, review, page } => wire::Read::Remarks { item: forge_item_to(item), review, page },
        forge::Read::PullFor { repository, head, base } => wire::Read::PullFor { repository, head, base },
        forge::Read::Statuses { repository, commit, page } => wire::Read::Statuses { repository, commit, page },
        forge::Read::Permission { repository, user } => wire::Read::Permission { repository, user },
        forge::Read::Branch { repository, branch } => wire::Read::Branch { repository, branch },
        forge::Read::Pages { repository, after } => wire::Read::Pages { repository, after },
        forge::Read::Page { repository, name } => wire::Read::Page { repository, name },
    }
}

pub(crate) fn read_from(value: wire::Read) -> forge::Read {
    match value {
        wire::Read::Item { item, after } => forge::Read::Item { item: forge_item_from(item), after },
        wire::Read::Pull { item } => forge::Read::Pull { item: forge_item_from(item) },
        wire::Read::Reviews { item, page } => forge::Read::Reviews { item: forge_item_from(item), page },
        wire::Read::Remarks { item, review, page } => {
            forge::Read::Remarks { item: forge_item_from(item), review, page }
        }
        wire::Read::PullFor { repository, head, base } => forge::Read::PullFor { repository, head, base },
        wire::Read::Statuses { repository, commit, page } => forge::Read::Statuses { repository, commit, page },
        wire::Read::Permission { repository, user } => forge::Read::Permission { repository, user },
        wire::Read::Branch { repository, branch } => forge::Read::Branch { repository, branch },
        wire::Read::Pages { repository, after } => forge::Read::Pages { repository, after },
        wire::Read::Page { repository, name } => forge::Read::Page { repository, name },
    }
}

pub(crate) fn forge_answer_to(value: api::Answer) -> Option<wire::ForgeAnswer> {
    Some(match value {
        api::Answer::Items { items, more, now } => {
            let mut found_items = List::with_capacity(u32::try_from(items.len()).ok()?);
            for value in items {
                found_items.push(summary_to(value)).expect("source array length");
            }
            let items = found_items.into_boxed();
            wire::ForgeAnswer::Items { items, more, now: time_to(now) }
        }
        api::Answer::Item { item, comments, more } => {
            let mut found_comments = List::with_capacity(u32::try_from(comments.len()).ok()?);
            for value in comments {
                found_comments.push(comment_to(value)).expect("source array length");
            }
            let comments = found_comments.into_boxed();
            wire::ForgeAnswer::Item { item: summary_to(item), comments, more }
        }
        api::Answer::Comment(comment) => wire::ForgeAnswer::Comment { comment: comment_to(comment) },
        api::Answer::Pull(pull) => wire::ForgeAnswer::Pull { pull: pull_to(pull) },
        api::Answer::Reviews { reviews, more } => {
            let mut found_reviews = List::with_capacity(u32::try_from(reviews.len()).ok()?);
            for value in reviews {
                found_reviews.push(forge_review_to(value)).expect("source array length");
            }
            let reviews = found_reviews.into_boxed();
            wire::ForgeAnswer::Reviews { reviews, more }
        }
        api::Answer::Statuses { ci, statuses, more } => {
            let mut found_statuses = List::with_capacity(u32::try_from(statuses.len()).ok()?);
            for value in statuses {
                found_statuses.push(status_to(value)).expect("source array length");
            }
            let statuses = found_statuses.into_boxed();
            wire::ForgeAnswer::Statuses { ci: ci_to(ci), statuses, more }
        }
        api::Answer::Remarks { remarks, more } => {
            let mut found_remarks = List::with_capacity(u32::try_from(remarks.len()).ok()?);
            for value in remarks {
                found_remarks.push(remark_to(value)).expect("source array length");
            }
            let remarks = found_remarks.into_boxed();
            wire::ForgeAnswer::Remarks { remarks, more }
        }
        api::Answer::Permission(permission) => wire::ForgeAnswer::Permission { permission: permission_to(permission) },
        api::Answer::Commit(commit) => wire::ForgeAnswer::Commit { commit },
        api::Answer::Pages { pages, next } => {
            let mut found_pages = List::with_capacity(u32::try_from(pages.len()).ok()?);
            for value in pages {
                found_pages.push(page_name_to(value)).expect("source array length");
            }
            let pages = found_pages.into_boxed();
            wire::ForgeAnswer::Pages { pages, next }
        }
        api::Answer::Page(page) => wire::ForgeAnswer::Page { page: forge_page_to(page) },
        api::Answer::Created(number) => wire::ForgeAnswer::Created { number },
        api::Answer::Commented { id, revision } => wire::ForgeAnswer::Commented { id, revision },
        api::Answer::Edited { revision } => wire::ForgeAnswer::Edited { revision },
        api::Answer::Reviewed(review) => wire::ForgeAnswer::Reviewed { review },
        api::Answer::Merged(commit) => wire::ForgeAnswer::Merged { commit },
        api::Answer::Revision(revision) => wire::ForgeAnswer::Revision { revision },
        api::Answer::Done => wire::ForgeAnswer::Done,
    })
}

pub(crate) fn forge_answer_from(value: wire::ForgeAnswer) -> Option<api::Answer> {
    Some(match value {
        wire::ForgeAnswer::Items { items, more, now } => {
            let mut found_items = List::with_capacity(u32::try_from(items.len()).ok()?);
            for value in items {
                found_items.push(summary_from(value)).expect("source array length");
            }
            let items = found_items.into_boxed();
            api::Answer::Items { items, more, now: time_from(now) }
        }
        wire::ForgeAnswer::Item { item, comments, more } => {
            let mut found_comments = List::with_capacity(u32::try_from(comments.len()).ok()?);
            for value in comments {
                found_comments.push(comment_from(value)).expect("source array length");
            }
            let comments = found_comments.into_boxed();
            api::Answer::Item { item: summary_from(item), comments, more }
        }
        wire::ForgeAnswer::Comment { comment } => api::Answer::Comment(comment_from(comment)),
        wire::ForgeAnswer::Pull { pull } => api::Answer::Pull(pull_from(pull)),
        wire::ForgeAnswer::Reviews { reviews, more } => {
            let mut found_reviews = List::with_capacity(u32::try_from(reviews.len()).ok()?);
            for value in reviews {
                found_reviews.push(forge_review_from(value)).expect("source array length");
            }
            let reviews = found_reviews.into_boxed();
            api::Answer::Reviews { reviews, more }
        }
        wire::ForgeAnswer::Statuses { ci, statuses, more } => {
            let mut found_statuses = List::with_capacity(u32::try_from(statuses.len()).ok()?);
            for value in statuses {
                found_statuses.push(status_from(value)).expect("source array length");
            }
            let statuses = found_statuses.into_boxed();
            api::Answer::Statuses { ci: ci_from(ci), statuses, more }
        }
        wire::ForgeAnswer::Remarks { remarks, more } => {
            let mut found_remarks = List::with_capacity(u32::try_from(remarks.len()).ok()?);
            for value in remarks {
                found_remarks.push(remark_from(value)).expect("source array length");
            }
            let remarks = found_remarks.into_boxed();
            api::Answer::Remarks { remarks, more }
        }
        wire::ForgeAnswer::Permission { permission } => api::Answer::Permission(permission_from(permission)),
        wire::ForgeAnswer::Commit { commit } => api::Answer::Commit(commit),
        wire::ForgeAnswer::Pages { pages, next } => {
            let mut found_pages = List::with_capacity(u32::try_from(pages.len()).ok()?);
            for value in pages {
                found_pages.push(page_name_from(value)).expect("source array length");
            }
            let pages = found_pages.into_boxed();
            api::Answer::Pages { pages, next }
        }
        wire::ForgeAnswer::Page { page } => api::Answer::Page(forge_page_from(page)),
        wire::ForgeAnswer::Created { number } => api::Answer::Created(number),
        wire::ForgeAnswer::Commented { id, revision } => api::Answer::Commented { id, revision },
        wire::ForgeAnswer::Edited { revision } => api::Answer::Edited { revision },
        wire::ForgeAnswer::Reviewed { review } => api::Answer::Reviewed(review),
        wire::ForgeAnswer::Merged { commit } => api::Answer::Merged(commit),
        wire::ForgeAnswer::Revision { revision } => api::Answer::Revision(revision),
        wire::ForgeAnswer::Done => api::Answer::Done,
    })
}

pub(crate) fn served_to(value: engine::Served) -> Option<wire::Served> {
    Some(match value {
        engine::Served::Read(answer) => wire::Served::Read { answer: forge_answer_to(answer)? },
        engine::Served::Recalled { entries, failed } => {
            let mut found_entries = List::with_capacity(u32::try_from(entries.len()).ok()?);
            for value in entries {
                found_entries.push(entry_to(value)?).expect("source array length");
            }
            let entries = found_entries.into_boxed();
            wire::Served::Recalled { entries, failed }
        }
        engine::Served::Noted(noted) => wire::Served::Noted { noted: noted_to(noted) },
        engine::Served::Posted { comment } => wire::Served::Posted { comment },
        engine::Served::Unserved(reason) => wire::Served::Unserved { reason: unserved_to(reason) },
    })
}

pub(crate) fn served_from(value: wire::Served) -> Option<engine::Served> {
    Some(match value {
        wire::Served::Read { answer } => engine::Served::Read(forge_answer_from(answer)?),
        wire::Served::Recalled { entries, failed } => {
            let mut found_entries = List::with_capacity(u32::try_from(entries.len()).ok()?);
            for value in entries {
                found_entries.push(entry_from(value)?).expect("source array length");
            }
            let entries = found_entries.into_boxed();
            engine::Served::Recalled { entries, failed }
        }
        wire::Served::Noted { noted } => engine::Served::Noted(noted_from(noted)),
        wire::Served::Posted { comment } => engine::Served::Posted { comment },
        wire::Served::Unserved { reason } => engine::Served::Unserved(unserved_from(reason)),
    })
}

pub(crate) fn fact_kind_to(value: views::Kind) -> wire::FactKind {
    match value {
        views::Kind::Text => wire::FactKind::Text,
        views::Kind::Progress => wire::FactKind::Progress,
        views::Kind::Call => wire::FactKind::Call,
        views::Kind::Tool => wire::FactKind::Tool,
        views::Kind::Usage => wire::FactKind::Usage,
    }
}

pub(crate) fn fact_kind_from(value: wire::FactKind) -> views::Kind {
    match value {
        wire::FactKind::Text => views::Kind::Text,
        wire::FactKind::Progress => views::Kind::Progress,
        wire::FactKind::Call => views::Kind::Call,
        wire::FactKind::Tool => views::Kind::Tool,
        wire::FactKind::Usage => views::Kind::Usage,
    }
}

fn time_to(value: Time) -> u64 {
    value.as_nanos()
}
fn time_from(value: u64) -> Time {
    Time::from_nanos(value)
}
#[must_use]
pub fn encode_inbound(value: engine::Inbound, sizes: &Sizes) -> Option<Box<[u8]>> {
    wire::encode_inbound(&inbound_to(value), sizes)
}
#[must_use]
pub fn decode_inbound(bytes: &[u8], sizes: &Sizes) -> Option<engine::Inbound> {
    Some(inbound_from(wire::decode_inbound(bytes, sizes)?))
}
#[must_use]
pub fn encode_call(value: engine::Call, sizes: &Sizes) -> Option<Box<[u8]>> {
    wire::encode_call(&call_to(value)?, sizes)
}
#[must_use]
pub fn decode_call(bytes: &[u8], sizes: &Sizes) -> Option<engine::Call> {
    call_from(wire::decode_call(bytes, sizes)?)
}
#[must_use]
pub fn encode_served(value: engine::Served, sizes: &Sizes) -> Option<Box<[u8]>> {
    wire::encode_served(&served_to(value)?, sizes)
}
#[must_use]
pub fn decode_served(bytes: &[u8], sizes: &Sizes) -> Option<engine::Served> {
    served_from(wire::decode_served(bytes, sizes)?)
}
#[must_use]
pub fn encode_fact(kind: views::Kind, content: Box<[u8]>, sizes: &Sizes) -> Option<Box<[u8]>> {
    wire::encode_fact(&wire::Fact { kind: fact_kind_to(kind), content }, sizes)
}
#[must_use]
pub fn decode_fact(bytes: &[u8], sizes: &Sizes) -> Option<(views::Kind, Box<[u8]>)> {
    let fact = wire::decode_fact(bytes, sizes)?;
    Some((fact_kind_from(fact.kind), fact.content))
}
