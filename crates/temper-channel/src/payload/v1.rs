//! Typed payloads shared by their two endpoints; workers pass the encoded bytes through.
use crate::{
    Sizes,
    primitives::{self as p, Encoder},
};
use alloc::boxed::Box;
use skein_lib::{Duration, List, Reader};
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item {
    pub repository: u32,
    pub number: u64,
}
pub(crate) fn put_item(out: &mut Encoder, value: &Item, _sizes: &Sizes) -> Option<()> {
    let repository = &value.repository;
    out.u32(*repository)?;
    let number = &value.number;
    out.u64(*number)?;
    Some(())
}
pub(crate) fn get_item(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Item> {
    Some(Item { repository: input.u32()?, number: input.u64()? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Section {
    pub kind: SectionKind,
    pub body: SectionBody,
}
pub(crate) fn put_section(out: &mut Encoder, value: &Section, sizes: &Sizes) -> Option<()> {
    let kind = &value.kind;
    put_section_kind(out, kind, sizes)?;
    let body = &value.body;
    put_section_body(out, body, sizes)?;
    Some(())
}
pub(crate) fn get_section(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Section> {
    Some(Section { kind: get_section_kind(input, sizes)?, body: get_section_body(input, sizes)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent wire flags")]
pub struct Permissions {
    pub modify: bool,
    pub shell: bool,
    pub forge: bool,
    pub subagents: bool,
    pub note: bool,
}
pub(crate) fn put_permissions(out: &mut Encoder, value: &Permissions, _sizes: &Sizes) -> Option<()> {
    let modify = &value.modify;
    out.bool(*modify)?;
    let shell = &value.shell;
    out.bool(*shell)?;
    let forge = &value.forge;
    out.bool(*forge)?;
    let subagents = &value.subagents;
    out.bool(*subagents)?;
    let note = &value.note;
    out.bool(*note)?;
    Some(())
}
pub(crate) fn get_permissions(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Permissions> {
    Some(Permissions {
        modify: p::boolean(input)?,
        shell: p::boolean(input)?,
        forge: p::boolean(input)?,
        subagents: p::boolean(input)?,
        note: p::boolean(input)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Budget {
    pub tokens: u64,
    pub turns: u32,
    pub time: Duration,
}
pub(crate) fn put_budget(out: &mut Encoder, value: &Budget, _sizes: &Sizes) -> Option<()> {
    let tokens = &value.tokens;
    out.u64(*tokens)?;
    let turns = &value.turns;
    out.u32(*turns)?;
    let time = &value.time;
    out.u64(time.as_nanos())?;
    Some(())
}
pub(crate) fn get_budget(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Budget> {
    Some(Budget { tokens: input.u64()?, turns: input.u32()?, time: Duration::from_nanos(input.u64()?) })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Model {
    pub endpoint: u32,
    pub model: Box<[u8]>,
    pub max_tokens: u32,
}
pub(crate) fn put_model(out: &mut Encoder, value: &Model, sizes: &Sizes) -> Option<()> {
    let endpoint = &value.endpoint;
    out.u32(*endpoint)?;
    let model = &value.model;
    out.bytes(model, sizes.name_bytes)?;
    let max_tokens = &value.max_tokens;
    out.u32(*max_tokens)?;
    Some(())
}
pub(crate) fn get_model(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Model> {
    Some(Model { endpoint: input.u32()?, model: p::bytes(input, sizes.name_bytes)?, max_tokens: input.u32()? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Policy {
    pub text: Capture,
    pub progress: Capture,
    pub calls: Capture,
    pub tools: Capture,
    pub usage: Capture,
}
pub(crate) fn put_policy(out: &mut Encoder, value: &Policy, sizes: &Sizes) -> Option<()> {
    let text = &value.text;
    put_capture(out, text, sizes)?;
    let progress = &value.progress;
    put_capture(out, progress, sizes)?;
    let calls = &value.calls;
    put_capture(out, calls, sizes)?;
    let tools = &value.tools;
    put_capture(out, tools, sizes)?;
    let usage = &value.usage;
    put_capture(out, usage, sizes)?;
    Some(())
}
pub(crate) fn get_policy(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Policy> {
    Some(Policy {
        text: get_capture(input, sizes)?,
        progress: get_capture(input, sizes)?,
        calls: get_capture(input, sizes)?,
        tools: get_capture(input, sizes)?,
        usage: get_capture(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Charter {
    pub why: Why,
    pub brief: Box<[Section]>,
    pub instructions: Box<[u8]>,
    pub grants: Permissions,
    pub finish: FinishSpec,
    pub budget: Budget,
    pub models: Box<[Model]>,
    pub policy: Policy,
}
pub(crate) fn put_charter(out: &mut Encoder, value: &Charter, sizes: &Sizes) -> Option<()> {
    let why = &value.why;
    put_why(out, why, sizes)?;
    let brief = &value.brief;
    let count_21 = u32::try_from(brief.len()).ok()?;
    if count_21 > sizes.entries {
        return None;
    }
    out.u32(count_21)?;
    for value in brief {
        put_section(out, value, sizes)?;
    }
    let instructions = &value.instructions;
    out.bytes(instructions, sizes.charter)?;
    let grants = &value.grants;
    put_permissions(out, grants, sizes)?;
    let finish = &value.finish;
    put_finish_spec(out, finish, sizes)?;
    let budget = &value.budget;
    put_budget(out, budget, sizes)?;
    let models = &value.models;
    let count_22 = u32::try_from(models.len()).ok()?;
    if count_22 > sizes.endpoints {
        return None;
    }
    out.u32(count_22)?;
    for value in models {
        put_model(out, value, sizes)?;
    }
    let policy = &value.policy;
    put_policy(out, policy, sizes)?;
    Some(())
}
pub(crate) fn get_charter(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Charter> {
    Some(Charter {
        why: get_why(input, sizes)?,
        brief: {
            let count_23 = p::count(input, sizes.entries, 3)?;
            let mut values_23 = List::with_capacity(count_23);
            for _ in 0..count_23 {
                let value = get_section(input, sizes)?;
                values_23.push(value).expect("the validated count reserves capacity");
            }
            values_23.into_boxed()
        },
        instructions: p::bytes(input, sizes.charter)?,
        grants: get_permissions(input, sizes)?,
        finish: get_finish_spec(input, sizes)?,
        budget: get_budget(input, sizes)?,
        models: {
            let count_24 = p::count(input, sizes.endpoints, 12)?;
            let mut values_24 = List::with_capacity(count_24);
            for _ in 0..count_24 {
                let value = get_model(input, sizes)?;
                values_24.push(value).expect("the validated count reserves capacity");
            }
            values_24.into_boxed()
        },
        policy: get_policy(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlanCharter {
    pub instructions: Box<[u8]>,
    pub template: Option<Box<[u8]>>,
    pub grants: Permissions,
    pub budget: Budget,
}
pub(crate) fn put_plan_charter(out: &mut Encoder, value: &PlanCharter, sizes: &Sizes) -> Option<()> {
    let instructions = &value.instructions;
    out.bytes(instructions, sizes.outcome)?;
    let template = &value.template;
    match template {
        Some(value) => {
            out.u8(1)?;
            out.bytes(value, sizes.name_bytes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let grants = &value.grants;
    put_permissions(out, grants, sizes)?;
    let budget = &value.budget;
    put_budget(out, budget, sizes)?;
    Some(())
}
pub(crate) fn get_plan_charter(input: &mut Reader<'_>, sizes: &Sizes) -> Option<PlanCharter> {
    Some(PlanCharter {
        instructions: p::bytes(input, sizes.outcome)?,
        template: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.name_bytes)?),
            _ => return None,
        },
        grants: get_permissions(input, sizes)?,
        budget: get_budget(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Target {
    pub repository: u32,
    pub base: Box<[u8]>,
}
pub(crate) fn put_target(out: &mut Encoder, value: &Target, sizes: &Sizes) -> Option<()> {
    let repository = &value.repository;
    out.u32(*repository)?;
    let base = &value.base;
    out.bytes(base, sizes.name_bytes)?;
    Some(())
}
pub(crate) fn get_target(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Target> {
    Some(Target { repository: input.u32()?, base: p::bytes(input, sizes.name_bytes)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Envelope {
    pub agents: u32,
    pub changes: u32,
    pub waits: u32,
    pub sessions: u32,
    pub repositories: Box<[u32]>,
    pub into: Box<[Target]>,
}
pub(crate) fn put_envelope(out: &mut Encoder, value: &Envelope, sizes: &Sizes) -> Option<()> {
    let agents = &value.agents;
    out.u32(*agents)?;
    let changes = &value.changes;
    out.u32(*changes)?;
    let waits = &value.waits;
    out.u32(*waits)?;
    let sessions = &value.sessions;
    out.u32(*sessions)?;
    let repositories = &value.repositories;
    let count_25 = u32::try_from(repositories.len()).ok()?;
    if count_25 > sizes.repositories {
        return None;
    }
    out.u32(count_25)?;
    for value in repositories {
        out.u32(*value)?;
    }
    let into = &value.into;
    let count_26 = u32::try_from(into.len()).ok()?;
    if count_26 > sizes.entries {
        return None;
    }
    out.u32(count_26)?;
    for value in into {
        put_target(out, value, sizes)?;
    }
    Some(())
}
pub(crate) fn get_envelope(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Envelope> {
    Some(Envelope {
        agents: input.u32()?,
        changes: input.u32()?,
        waits: input.u32()?,
        sessions: input.u32()?,
        repositories: {
            let count_27 = p::count(input, sizes.repositories, 4)?;
            let mut values_27 = List::with_capacity(count_27);
            for _ in 0..count_27 {
                let value = input.u32()?;
                values_27.push(value).expect("the validated count reserves capacity");
            }
            values_27.into_boxed()
        },
        into: {
            let count_28 = p::count(input, sizes.entries, 8)?;
            let mut values_28 = List::with_capacity(count_28);
            for _ in 0..count_28 {
                let value = get_target(input, sizes)?;
                values_28.push(value).expect("the validated count reserves capacity");
            }
            values_28.into_boxed()
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent wire flags")]
pub struct Sources {
    pub own: bool,
    pub related: bool,
    pub subscribed: bool,
    pub messages: bool,
}
pub(crate) fn put_sources(out: &mut Encoder, value: &Sources, _sizes: &Sizes) -> Option<()> {
    let own = &value.own;
    out.bool(*own)?;
    let related = &value.related;
    out.bool(*related)?;
    let subscribed = &value.subscribed;
    out.bool(*subscribed)?;
    let messages = &value.messages;
    out.bool(*messages)?;
    Some(())
}
pub(crate) fn get_sources(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Sources> {
    Some(Sources {
        own: p::boolean(input)?,
        related: p::boolean(input)?,
        subscribed: p::boolean(input)?,
        messages: p::boolean(input)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Batch {
    pub count: u32,
    pub age: Option<Duration>,
}
pub(crate) fn put_batch(out: &mut Encoder, value: &Batch, _sizes: &Sizes) -> Option<()> {
    let count = &value.count;
    out.u32(*count)?;
    let age = &value.age;
    match age {
        Some(value) => {
            out.u8(1)?;
            out.u64(value.as_nanos())?;
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_batch(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Batch> {
    Some(Batch {
        count: input.u32()?,
        age: match input.u8()? {
            0 => None,
            1 => Some(Duration::from_nanos(input.u64()?)),
            _ => return None,
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Wake {
    pub on: Sources,
    pub every: Option<Duration>,
    pub batch: Batch,
}
pub(crate) fn put_wake(out: &mut Encoder, value: &Wake, sizes: &Sizes) -> Option<()> {
    let on = &value.on;
    put_sources(out, on, sizes)?;
    let every = &value.every;
    match every {
        Some(value) => {
            out.u8(1)?;
            out.u64(value.as_nanos())?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let batch = &value.batch;
    put_batch(out, batch, sizes)?;
    Some(())
}
pub(crate) fn get_wake(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Wake> {
    Some(Wake {
        on: get_sources(input, sizes)?,
        every: match input.u8()? {
            0 => None,
            1 => Some(Duration::from_nanos(input.u64()?)),
            _ => return None,
        },
        batch: get_batch(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub name: Box<[u8]>,
    pub repository: u32,
    pub work: StepWork,
    pub after: Box<[Box<[u8]>]>,
    pub gates: Box<[Gate]>,
}
pub(crate) fn put_step(out: &mut Encoder, value: &Step, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let repository = &value.repository;
    out.u32(*repository)?;
    let work = &value.work;
    put_step_work(out, work, sizes)?;
    let after = &value.after;
    let count_29 = u32::try_from(after.len()).ok()?;
    if count_29 > sizes.entries {
        return None;
    }
    out.u32(count_29)?;
    for value in after {
        out.bytes(value, sizes.name_bytes)?;
    }
    let gates = &value.gates;
    let count_30 = u32::try_from(gates.len()).ok()?;
    if count_30 > sizes.entries {
        return None;
    }
    out.u32(count_30)?;
    for value in gates {
        put_gate(out, value, sizes)?;
    }
    Some(())
}
pub(crate) fn get_step(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Step> {
    Some(Step {
        name: p::bytes(input, sizes.name_bytes)?,
        repository: input.u32()?,
        work: get_step_work(input, sizes)?,
        after: {
            let count_31 = p::count(input, sizes.entries, 4)?;
            let mut values_31 = List::with_capacity(count_31);
            for _ in 0..count_31 {
                let value = p::bytes(input, sizes.name_bytes)?;
                values_31.push(value).expect("the validated count reserves capacity");
            }
            values_31.into_boxed()
        },
        gates: {
            let count_32 = p::count(input, sizes.entries, 1)?;
            let mut values_32 = List::with_capacity(count_32);
            for _ in 0..count_32 {
                let value = get_gate(input, sizes)?;
                values_32.push(value).expect("the validated count reserves capacity");
            }
            values_32.into_boxed()
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub steps: Box<[Step]>,
    pub envelope: Envelope,
    pub budget: u64,
}
pub(crate) fn put_plan(out: &mut Encoder, value: &Plan, sizes: &Sizes) -> Option<()> {
    let steps = &value.steps;
    let count_33 = u32::try_from(steps.len()).ok()?;
    if count_33 > sizes.entries {
        return None;
    }
    out.u32(count_33)?;
    for value in steps {
        put_step(out, value, sizes)?;
    }
    let envelope = &value.envelope;
    put_envelope(out, envelope, sizes)?;
    let budget = &value.budget;
    out.u64(*budget)?;
    Some(())
}
pub(crate) fn get_plan(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Plan> {
    Some(Plan {
        steps: {
            let count_34 = p::count(input, sizes.entries, 18)?;
            let mut values_34 = List::with_capacity(count_34);
            for _ in 0..count_34 {
                let value = get_step(input, sizes)?;
                values_34.push(value).expect("the validated count reserves capacity");
            }
            values_34.into_boxed()
        },
        envelope: get_envelope(input, sizes)?,
        budget: input.u64()?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fact {
    pub kind: FactKind,
    pub content: Box<[u8]>,
}
pub(crate) fn put_fact(out: &mut Encoder, value: &Fact, sizes: &Sizes) -> Option<()> {
    let kind = &value.kind;
    put_fact_kind(out, kind, sizes)?;
    let content = &value.content;
    out.bytes(content, sizes.fact)?;
    Some(())
}
pub(crate) fn get_fact(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Fact> {
    Some(Fact { kind: get_fact_kind(input, sizes)?, content: p::bytes(input, sizes.fact)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Snapshot {
    pub version: u16,
    pub body: Box<[u8]>,
}
pub(crate) fn put_snapshot(out: &mut Encoder, value: &Snapshot, sizes: &Sizes) -> Option<()> {
    let version = &value.version;
    out.u16(*version)?;
    let body = &value.body;
    out.bytes(body, sizes.snapshot)?;
    Some(())
}
pub(crate) fn get_snapshot(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Snapshot> {
    Some(Snapshot { version: input.u16()?, body: p::bytes(input, sizes.snapshot)? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Scopes {
    pub repository: u32,
    pub goal: Option<Item>,
}
pub(crate) fn put_scopes(out: &mut Encoder, value: &Scopes, sizes: &Sizes) -> Option<()> {
    let repository = &value.repository;
    out.u32(*repository)?;
    let goal = &value.goal;
    match goal {
        Some(value) => {
            out.u8(1)?;
            put_item(out, value, sizes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_scopes(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Scopes> {
    Some(Scopes {
        repository: input.u32()?,
        goal: match input.u8()? {
            0 => None,
            1 => Some(get_item(input, sizes)?),
            _ => return None,
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Page {
    pub description: Box<[u8]>,
    pub author: Author,
    pub references: Box<[Item]>,
    pub body: Box<[u8]>,
}
pub(crate) fn put_page(out: &mut Encoder, value: &Page, sizes: &Sizes) -> Option<()> {
    let description = &value.description;
    out.bytes(description, sizes.answer)?;
    let author = &value.author;
    put_author(out, author, sizes)?;
    let references = &value.references;
    let count_35 = u32::try_from(references.len()).ok()?;
    if count_35 > sizes.entries {
        return None;
    }
    out.u32(count_35)?;
    for value in references {
        put_item(out, value, sizes)?;
    }
    let body = &value.body;
    out.bytes(body, sizes.answer)?;
    Some(())
}
pub(crate) fn get_page(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Page> {
    Some(Page {
        description: p::bytes(input, sizes.answer)?,
        author: get_author(input, sizes)?,
        references: {
            let count_36 = p::count(input, sizes.entries, 12)?;
            let mut values_36 = List::with_capacity(count_36);
            for _ in 0..count_36 {
                let value = get_item(input, sizes)?;
                values_36.push(value).expect("the validated count reserves capacity");
            }
            values_36.into_boxed()
        },
        body: p::bytes(input, sizes.answer)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub scope: Scope,
    pub name: Box<[u8]>,
    pub revision: u64,
    pub page: Page,
}
pub(crate) fn put_entry(out: &mut Encoder, value: &Entry, sizes: &Sizes) -> Option<()> {
    let scope = &value.scope;
    put_scope(out, scope, sizes)?;
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let revision = &value.revision;
    out.u64(*revision)?;
    let page = &value.page;
    put_page(out, page, sizes)?;
    Some(())
}
pub(crate) fn get_entry(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Entry> {
    Some(Entry {
        scope: get_scope(input, sizes)?,
        name: p::bytes(input, sizes.name_bytes)?,
        revision: input.u64()?,
        page: get_page(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Position {
    pub comment: u64,
    pub pull_comment: u64,
    pub reviews: u64,
    pub head: Option<[u8; 32]>,
    pub ci: Ci,
}
pub(crate) fn put_position(out: &mut Encoder, value: &Position, sizes: &Sizes) -> Option<()> {
    let comment = &value.comment;
    out.u64(*comment)?;
    let pull_comment = &value.pull_comment;
    out.u64(*pull_comment)?;
    let reviews = &value.reviews;
    out.u64(*reviews)?;
    let head = &value.head;
    match head {
        Some(value) => {
            out.u8(1)?;
            out.raw(value)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let ci = &value.ci;
    put_ci(out, ci, sizes)?;
    Some(())
}
pub(crate) fn get_position(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Position> {
    Some(Position {
        comment: input.u64()?,
        pull_comment: input.u64()?,
        reviews: input.u64()?,
        head: match input.u8()? {
            0 => None,
            1 => Some(*input.bytes(32)?.first_chunk::<32>()?),
            _ => return None,
        },
        ci: get_ci(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
/// Source timestamps are forge metadata, never a receiver's monotonic clock or deadline.
pub struct Summary {
    pub number: u64,
    pub kind: ForgeKind,
    pub state: State,
    pub author: u64,
    pub key: Option<Box<[u8]>>,
    pub labels: Box<[Box<[u8]>]>,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub updated: u64,
}
pub(crate) fn put_summary(out: &mut Encoder, value: &Summary, sizes: &Sizes) -> Option<()> {
    let number = &value.number;
    out.u64(*number)?;
    let kind = &value.kind;
    put_forge_kind(out, kind, sizes)?;
    let state = &value.state;
    put_state(out, state, sizes)?;
    let author = &value.author;
    out.u64(*author)?;
    let key = &value.key;
    match key {
        Some(value) => {
            out.u8(1)?;
            out.bytes(value, sizes.answer)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let labels = &value.labels;
    let count_37 = u32::try_from(labels.len()).ok()?;
    if count_37 > sizes.entries {
        return None;
    }
    out.u32(count_37)?;
    for value in labels {
        out.bytes(value, sizes.name_bytes)?;
    }
    let title = &value.title;
    out.bytes(title, sizes.answer)?;
    let body = &value.body;
    out.bytes(body, sizes.answer)?;
    let updated = &value.updated;
    out.u64(*updated)?;
    Some(())
}
pub(crate) fn get_summary(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Summary> {
    Some(Summary {
        number: input.u64()?,
        kind: get_forge_kind(input, sizes)?,
        state: get_state(input, sizes)?,
        author: input.u64()?,
        key: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.answer)?),
            _ => return None,
        },
        labels: {
            let count_38 = p::count(input, sizes.entries, 4)?;
            let mut values_38 = List::with_capacity(count_38);
            for _ in 0..count_38 {
                let value = p::bytes(input, sizes.name_bytes)?;
                values_38.push(value).expect("the validated count reserves capacity");
            }
            values_38.into_boxed()
        },
        title: p::bytes(input, sizes.answer)?,
        body: p::bytes(input, sizes.answer)?,
        updated: input.u64()?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
/// `created` is source metadata; connections never arm deadlines with it.
pub struct Comment {
    pub id: u64,
    pub author: u64,
    pub created: u64,
    pub revision: u64,
    pub mark: Mark,
    pub body: Box<[u8]>,
}
pub(crate) fn put_comment(out: &mut Encoder, value: &Comment, sizes: &Sizes) -> Option<()> {
    let id = &value.id;
    out.u64(*id)?;
    let author = &value.author;
    out.u64(*author)?;
    let created = &value.created;
    out.u64(*created)?;
    let revision = &value.revision;
    out.u64(*revision)?;
    let mark = &value.mark;
    put_mark(out, mark, sizes)?;
    let body = &value.body;
    out.bytes(body, sizes.answer)?;
    Some(())
}
pub(crate) fn get_comment(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Comment> {
    Some(Comment {
        id: input.u64()?,
        author: input.u64()?,
        created: input.u64()?,
        revision: input.u64()?,
        mark: get_mark(input, sizes)?,
        body: p::bytes(input, sizes.answer)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pull {
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: [u8; 32],
    pub base_commit: Option<[u8; 32]>,
    pub merged: Option<[u8; 32]>,
    pub mergeable: bool,
    pub ci: Ci,
}
pub(crate) fn put_pull(out: &mut Encoder, value: &Pull, sizes: &Sizes) -> Option<()> {
    let number = &value.number;
    out.u64(*number)?;
    let state = &value.state;
    put_state(out, state, sizes)?;
    let head = &value.head;
    out.bytes(head, sizes.name_bytes)?;
    let base = &value.base;
    out.bytes(base, sizes.name_bytes)?;
    let commit = &value.commit;
    out.raw(commit)?;
    let base_commit = &value.base_commit;
    match base_commit {
        Some(value) => {
            out.u8(1)?;
            out.raw(value)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let merged = &value.merged;
    match merged {
        Some(value) => {
            out.u8(1)?;
            out.raw(value)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let mergeable = &value.mergeable;
    out.bool(*mergeable)?;
    let ci = &value.ci;
    put_ci(out, ci, sizes)?;
    Some(())
}
pub(crate) fn get_pull(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Pull> {
    Some(Pull {
        number: input.u64()?,
        state: get_state(input, sizes)?,
        head: p::bytes(input, sizes.name_bytes)?,
        base: p::bytes(input, sizes.name_bytes)?,
        commit: *input.bytes(32)?.first_chunk::<32>()?,
        base_commit: match input.u8()? {
            0 => None,
            1 => Some(*input.bytes(32)?.first_chunk::<32>()?),
            _ => return None,
        },
        merged: match input.u8()? {
            0 => None,
            1 => Some(*input.bytes(32)?.first_chunk::<32>()?),
            _ => return None,
        },
        mergeable: p::boolean(input)?,
        ci: get_ci(input, sizes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ForgeReview {
    pub id: u64,
    pub author: u64,
    pub verdict: ForgeVerdict,
    pub commit: [u8; 32],
    pub key: Option<Box<[u8]>>,
    pub body: Box<[u8]>,
}
pub(crate) fn put_forge_review(out: &mut Encoder, value: &ForgeReview, sizes: &Sizes) -> Option<()> {
    let id = &value.id;
    out.u64(*id)?;
    let author = &value.author;
    out.u64(*author)?;
    let verdict = &value.verdict;
    put_forge_verdict(out, verdict, sizes)?;
    let commit = &value.commit;
    out.raw(commit)?;
    let key = &value.key;
    match key {
        Some(value) => {
            out.u8(1)?;
            out.bytes(value, sizes.answer)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let body = &value.body;
    out.bytes(body, sizes.answer)?;
    Some(())
}
pub(crate) fn get_forge_review(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgeReview> {
    Some(ForgeReview {
        id: input.u64()?,
        author: input.u64()?,
        verdict: get_forge_verdict(input, sizes)?,
        commit: *input.bytes(32)?.first_chunk::<32>()?,
        key: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.answer)?),
            _ => return None,
        },
        body: p::bytes(input, sizes.answer)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Remark {
    pub id: u64,
    pub author: u64,
    pub path: Box<[u8]>,
    pub line: u32,
    pub body: Box<[u8]>,
}
pub(crate) fn put_remark(out: &mut Encoder, value: &Remark, sizes: &Sizes) -> Option<()> {
    let id = &value.id;
    out.u64(*id)?;
    let author = &value.author;
    out.u64(*author)?;
    let path = &value.path;
    out.bytes(path, sizes.name_bytes)?;
    let line = &value.line;
    out.u32(*line)?;
    let body = &value.body;
    out.bytes(body, sizes.answer)?;
    Some(())
}
pub(crate) fn get_remark(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Remark> {
    Some(Remark {
        id: input.u64()?,
        author: input.u64()?,
        path: p::bytes(input, sizes.name_bytes)?,
        line: input.u32()?,
        body: p::bytes(input, sizes.answer)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    pub context: Box<[u8]>,
    pub check: Check,
    pub description: Box<[u8]>,
    pub url: Box<[u8]>,
}
pub(crate) fn put_status(out: &mut Encoder, value: &Status, sizes: &Sizes) -> Option<()> {
    let context = &value.context;
    out.bytes(context, sizes.name_bytes)?;
    let check = &value.check;
    put_check(out, check, sizes)?;
    let description = &value.description;
    out.bytes(description, sizes.answer)?;
    let url = &value.url;
    out.bytes(url, sizes.name_bytes)?;
    Some(())
}
pub(crate) fn get_status(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Status> {
    Some(Status {
        context: p::bytes(input, sizes.name_bytes)?,
        check: get_check(input, sizes)?,
        description: p::bytes(input, sizes.answer)?,
        url: p::bytes(input, sizes.name_bytes)?,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ForgePage {
    pub name: Box<[u8]>,
    pub content: Box<[u8]>,
    pub revision: u64,
    pub nonce: Option<u64>,
}
pub(crate) fn put_forge_page(out: &mut Encoder, value: &ForgePage, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let content = &value.content;
    out.bytes(content, sizes.answer)?;
    let revision = &value.revision;
    out.u64(*revision)?;
    let nonce = &value.nonce;
    match nonce {
        Some(value) => {
            out.u8(1)?;
            out.u64(*value)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_forge_page(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgePage> {
    Some(ForgePage {
        name: p::bytes(input, sizes.name_bytes)?,
        content: p::bytes(input, sizes.answer)?,
        revision: input.u64()?,
        nonce: match input.u8()? {
            0 => None,
            1 => Some(input.u64()?),
            _ => return None,
        },
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PageName {
    pub name: Box<[u8]>,
    pub revision: u64,
}
pub(crate) fn put_page_name(out: &mut Encoder, value: &PageName, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let revision = &value.revision;
    out.u64(*revision)?;
    Some(())
}
pub(crate) fn get_page_name(input: &mut Reader<'_>, sizes: &Sizes) -> Option<PageName> {
    Some(PageName { name: p::bytes(input, sizes.name_bytes)?, revision: input.u64()? })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Why {
    Work,
    Produce,
    Repair { repair: Repair },
    Review { head: [u8; 32] },
    Turn,
}
pub(crate) fn put_why(out: &mut Encoder, value: &Why, sizes: &Sizes) -> Option<()> {
    match value {
        Why::Work => {
            out.u8(0)?;
        }
        Why::Produce => {
            out.u8(1)?;
        }
        Why::Repair { repair } => {
            out.u8(2)?;
            put_repair(out, repair, sizes)?;
        }
        Why::Review { head } => {
            out.u8(3)?;
            out.raw(head)?;
        }
        Why::Turn => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_why(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Why> {
    Some(match input.u8()? {
        0 => Why::Work,
        1 => Why::Produce,
        2 => Why::Repair { repair: get_repair(input, sizes)? },
        3 => Why::Review { head: *input.bytes(32)?.first_chunk::<32>()? },
        4 => Why::Turn,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Repair {
    CiFailed,
    ChangesRequested,
    BaseMoved,
    Conflicts,
}
pub(crate) fn put_repair(out: &mut Encoder, value: &Repair, _sizes: &Sizes) -> Option<()> {
    match value {
        Repair::CiFailed => {
            out.u8(0)?;
        }
        Repair::ChangesRequested => {
            out.u8(1)?;
        }
        Repair::BaseMoved => {
            out.u8(2)?;
        }
        Repair::Conflicts => {
            out.u8(3)?;
        }
    }
    Some(())
}
pub(crate) fn get_repair(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Repair> {
    Some(match input.u8()? {
        0 => Repair::CiFailed,
        1 => Repair::ChangesRequested,
        2 => Repair::BaseMoved,
        3 => Repair::Conflicts,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SectionKind {
    Item,
    Comments,
    Dependencies,
    Ci,
    Reviews,
    Pull,
    Attempts,
    Plan,
    Notes,
    Template,
}
pub(crate) fn put_section_kind(out: &mut Encoder, value: &SectionKind, _sizes: &Sizes) -> Option<()> {
    match value {
        SectionKind::Item => {
            out.u8(0)?;
        }
        SectionKind::Comments => {
            out.u8(1)?;
        }
        SectionKind::Dependencies => {
            out.u8(2)?;
        }
        SectionKind::Ci => {
            out.u8(3)?;
        }
        SectionKind::Reviews => {
            out.u8(4)?;
        }
        SectionKind::Pull => {
            out.u8(5)?;
        }
        SectionKind::Attempts => {
            out.u8(6)?;
        }
        SectionKind::Plan => {
            out.u8(7)?;
        }
        SectionKind::Notes => {
            out.u8(8)?;
        }
        SectionKind::Template => {
            out.u8(9)?;
        }
    }
    Some(())
}
pub(crate) fn get_section_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<SectionKind> {
    Some(match input.u8()? {
        0 => SectionKind::Item,
        1 => SectionKind::Comments,
        2 => SectionKind::Dependencies,
        3 => SectionKind::Ci,
        4 => SectionKind::Reviews,
        5 => SectionKind::Pull,
        6 => SectionKind::Attempts,
        7 => SectionKind::Plan,
        8 => SectionKind::Notes,
        9 => SectionKind::Template,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unread {
    Failed,
    Late,
    Oversized,
}
pub(crate) fn put_unread(out: &mut Encoder, value: &Unread, _sizes: &Sizes) -> Option<()> {
    match value {
        Unread::Failed => {
            out.u8(0)?;
        }
        Unread::Late => {
            out.u8(1)?;
        }
        Unread::Oversized => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_unread(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Unread> {
    Some(match input.u8()? {
        0 => Unread::Failed,
        1 => Unread::Late,
        2 => Unread::Oversized,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SectionBody {
    Text { text: Box<[u8]> },
    Missing { unread: Unread },
}
pub(crate) fn put_section_body(out: &mut Encoder, value: &SectionBody, sizes: &Sizes) -> Option<()> {
    match value {
        SectionBody::Text { text } => {
            out.u8(0)?;
            out.bytes(text, sizes.charter)?;
        }
        SectionBody::Missing { unread } => {
            out.u8(1)?;
            put_unread(out, unread, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_section_body(input: &mut Reader<'_>, sizes: &Sizes) -> Option<SectionBody> {
    Some(match input.u8()? {
        0 => SectionBody::Text { text: p::bytes(input, sizes.charter)? },
        1 => SectionBody::Missing { unread: get_unread(input, sizes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FinishSpec {
    Report { grows: bool },
    Change { checks: bool },
    Verdict,
    Turn { supervising: bool },
}
pub(crate) fn put_finish_spec(out: &mut Encoder, value: &FinishSpec, _sizes: &Sizes) -> Option<()> {
    match value {
        FinishSpec::Report { grows } => {
            out.u8(0)?;
            out.bool(*grows)?;
        }
        FinishSpec::Change { checks } => {
            out.u8(1)?;
            out.bool(*checks)?;
        }
        FinishSpec::Verdict => {
            out.u8(2)?;
        }
        FinishSpec::Turn { supervising } => {
            out.u8(3)?;
            out.bool(*supervising)?;
        }
    }
    Some(())
}
pub(crate) fn get_finish_spec(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<FinishSpec> {
    Some(match input.u8()? {
        0 => FinishSpec::Report { grows: p::boolean(input)? },
        1 => FinishSpec::Change { checks: p::boolean(input)? },
        2 => FinishSpec::Verdict,
        3 => FinishSpec::Turn { supervising: p::boolean(input)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Capture {
    Nothing,
    Shape,
    Content,
}
pub(crate) fn put_capture(out: &mut Encoder, value: &Capture, _sizes: &Sizes) -> Option<()> {
    match value {
        Capture::Nothing => {
            out.u8(0)?;
        }
        Capture::Shape => {
            out.u8(1)?;
        }
        Capture::Content => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_capture(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Capture> {
    Some(match input.u8()? {
        0 => Capture::Nothing,
        1 => Capture::Shape,
        2 => Capture::Content,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Verdict {
    Approve,
    Changes,
}
pub(crate) fn put_verdict(out: &mut Encoder, value: &Verdict, _sizes: &Sizes) -> Option<()> {
    match value {
        Verdict::Approve => {
            out.u8(0)?;
        }
        Verdict::Changes => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_verdict(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Verdict> {
    Some(match input.u8()? {
        0 => Verdict::Approve,
        1 => Verdict::Changes,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Gate {
    Approvals { count: u32 },
    Accepted,
}
pub(crate) fn put_gate(out: &mut Encoder, value: &Gate, _sizes: &Sizes) -> Option<()> {
    match value {
        Gate::Approvals { count } => {
            out.u8(0)?;
            out.u32(*count)?;
        }
        Gate::Accepted => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_gate(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Gate> {
    Some(match input.u8()? {
        0 => Gate::Approvals { count: input.u32()? },
        1 => Gate::Accepted,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resume {
    Default,
    Always,
    Never,
}
pub(crate) fn put_resume(out: &mut Encoder, value: &Resume, _sizes: &Sizes) -> Option<()> {
    match value {
        Resume::Default => {
            out.u8(0)?;
        }
        Resume::Always => {
            out.u8(1)?;
        }
        Resume::Never => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_resume(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Resume> {
    Some(match input.u8()? {
        0 => Resume::Default,
        1 => Resume::Always,
        2 => Resume::Never,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Review {
    Person,
    Agent { charter: PlanCharter },
}
pub(crate) fn put_review(out: &mut Encoder, value: &Review, sizes: &Sizes) -> Option<()> {
    match value {
        Review::Person => {
            out.u8(0)?;
        }
        Review::Agent { charter } => {
            out.u8(1)?;
            put_plan_charter(out, charter, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_review(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Review> {
    Some(match input.u8()? {
        0 => Review::Person,
        1 => Review::Agent { charter: get_plan_charter(input, sizes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WaitSpec {
    Steps,
    Decision,
    Time { duration: Duration },
}
pub(crate) fn put_wait_spec(out: &mut Encoder, value: &WaitSpec, _sizes: &Sizes) -> Option<()> {
    match value {
        WaitSpec::Steps => {
            out.u8(0)?;
        }
        WaitSpec::Decision => {
            out.u8(1)?;
        }
        WaitSpec::Time { duration } => {
            out.u8(2)?;
            out.u64(duration.as_nanos())?;
        }
    }
    Some(())
}
pub(crate) fn get_wait_spec(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<WaitSpec> {
    Some(match input.u8()? {
        0 => WaitSpec::Steps,
        1 => WaitSpec::Decision,
        2 => WaitSpec::Time { duration: Duration::from_nanos(input.u64()?) },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StepWork {
    Agent { charter: PlanCharter, grows: bool },
    Change { base: Box<[u8]>, produce: PlanCharter, checks: bool, review: Review },
    Wait { wait: WaitSpec },
    Session { charter: PlanCharter, resume: Resume, wake: Wake },
}
pub(crate) fn put_step_work(out: &mut Encoder, value: &StepWork, sizes: &Sizes) -> Option<()> {
    match value {
        StepWork::Agent { charter, grows } => {
            out.u8(0)?;
            put_plan_charter(out, charter, sizes)?;
            out.bool(*grows)?;
        }
        StepWork::Change { base, produce, checks, review } => {
            out.u8(1)?;
            out.bytes(base, sizes.name_bytes)?;
            put_plan_charter(out, produce, sizes)?;
            out.bool(*checks)?;
            put_review(out, review, sizes)?;
        }
        StepWork::Wait { wait } => {
            out.u8(2)?;
            put_wait_spec(out, wait, sizes)?;
        }
        StepWork::Session { charter, resume, wake } => {
            out.u8(3)?;
            put_plan_charter(out, charter, sizes)?;
            put_resume(out, resume, sizes)?;
            put_wake(out, wake, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_step_work(input: &mut Reader<'_>, sizes: &Sizes) -> Option<StepWork> {
    Some(match input.u8()? {
        0 => StepWork::Agent { charter: get_plan_charter(input, sizes)?, grows: p::boolean(input)? },
        1 => StepWork::Change {
            base: p::bytes(input, sizes.name_bytes)?,
            produce: get_plan_charter(input, sizes)?,
            checks: p::boolean(input)?,
            review: get_review(input, sizes)?,
        },
        2 => StepWork::Wait { wait: get_wait_spec(input, sizes)? },
        3 => StepWork::Session {
            charter: get_plan_charter(input, sizes)?,
            resume: get_resume(input, sizes)?,
            wake: get_wake(input, sizes)?,
        },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Change { message: Box<[u8]> },
    Verdict { verdict: Verdict, text: Box<[u8]> },
    Report { text: Box<[u8]> },
    Plan { plan: Plan, text: Box<[u8]> },
    Steps { steps: Box<[Step]>, text: Box<[u8]> },
    Tasks { tasks: Box<[Step]>, text: Box<[u8]> },
    Reply { text: Box<[u8]> },
    Finished { text: Box<[u8]> },
    Release { step: Box<[u8]>, text: Box<[u8]> },
    Escalation { text: Box<[u8]> },
}
pub(crate) fn put_outcome(out: &mut Encoder, value: &Outcome, sizes: &Sizes) -> Option<()> {
    match value {
        Outcome::Change { message } => {
            out.u8(0)?;
            out.bytes(message, sizes.outcome)?;
        }
        Outcome::Verdict { verdict, text } => {
            out.u8(1)?;
            put_verdict(out, verdict, sizes)?;
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Report { text } => {
            out.u8(2)?;
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Plan { plan, text } => {
            out.u8(3)?;
            put_plan(out, plan, sizes)?;
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Steps { steps, text } => {
            out.u8(4)?;
            let count_39 = u32::try_from(steps.len()).ok()?;
            if count_39 > sizes.entries {
                return None;
            }
            out.u32(count_39)?;
            for value in steps {
                put_step(out, value, sizes)?;
            }
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Tasks { tasks, text } => {
            out.u8(5)?;
            let count_40 = u32::try_from(tasks.len()).ok()?;
            if count_40 > sizes.entries {
                return None;
            }
            out.u32(count_40)?;
            for value in tasks {
                put_step(out, value, sizes)?;
            }
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Reply { text } => {
            out.u8(6)?;
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Finished { text } => {
            out.u8(7)?;
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Release { step, text } => {
            out.u8(8)?;
            out.bytes(step, sizes.name_bytes)?;
            out.bytes(text, sizes.outcome)?;
        }
        Outcome::Escalation { text } => {
            out.u8(9)?;
            out.bytes(text, sizes.outcome)?;
        }
    }
    Some(())
}
pub(crate) fn get_outcome(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Outcome> {
    Some(match input.u8()? {
        0 => Outcome::Change { message: p::bytes(input, sizes.outcome)? },
        1 => Outcome::Verdict { verdict: get_verdict(input, sizes)?, text: p::bytes(input, sizes.outcome)? },
        2 => Outcome::Report { text: p::bytes(input, sizes.outcome)? },
        3 => Outcome::Plan { plan: get_plan(input, sizes)?, text: p::bytes(input, sizes.outcome)? },
        4 => Outcome::Steps {
            steps: {
                let count_41 = p::count(input, sizes.entries, 18)?;
                let mut values_41 = List::with_capacity(count_41);
                for _ in 0..count_41 {
                    let value = get_step(input, sizes)?;
                    values_41.push(value).expect("the validated count reserves capacity");
                }
                values_41.into_boxed()
            },
            text: p::bytes(input, sizes.outcome)?,
        },
        5 => Outcome::Tasks {
            tasks: {
                let count_42 = p::count(input, sizes.entries, 18)?;
                let mut values_42 = List::with_capacity(count_42);
                for _ in 0..count_42 {
                    let value = get_step(input, sizes)?;
                    values_42.push(value).expect("the validated count reserves capacity");
                }
                values_42.into_boxed()
            },
            text: p::bytes(input, sizes.outcome)?,
        },
        6 => Outcome::Reply { text: p::bytes(input, sizes.outcome)? },
        7 => Outcome::Finished { text: p::bytes(input, sizes.outcome)? },
        8 => Outcome::Release { step: p::bytes(input, sizes.name_bytes)?, text: p::bytes(input, sizes.outcome)? },
        9 => Outcome::Escalation { text: p::bytes(input, sizes.outcome)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ci {
    None_,
    Pending,
    Passed,
    Failed,
}
pub(crate) fn put_ci(out: &mut Encoder, value: &Ci, _sizes: &Sizes) -> Option<()> {
    match value {
        Ci::None_ => {
            out.u8(0)?;
        }
        Ci::Pending => {
            out.u8(1)?;
        }
        Ci::Passed => {
            out.u8(2)?;
        }
        Ci::Failed => {
            out.u8(3)?;
        }
    }
    Some(())
}
pub(crate) fn get_ci(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Ci> {
    Some(match input.u8()? {
        0 => Ci::None_,
        1 => Ci::Pending,
        2 => Ci::Passed,
        3 => Ci::Failed,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum News {
    Comment { on: u64, id: u64, author: u64 },
    Reviews { commit: [u8; 32] },
    Pull { commit: [u8; 32], ci: Ci, open: bool, merged: Option<[u8; 32]>, mergeable: bool },
}
pub(crate) fn put_news(out: &mut Encoder, value: &News, sizes: &Sizes) -> Option<()> {
    match value {
        News::Comment { on, id, author } => {
            out.u8(0)?;
            out.u64(*on)?;
            out.u64(*id)?;
            out.u64(*author)?;
        }
        News::Reviews { commit } => {
            out.u8(1)?;
            out.raw(commit)?;
        }
        News::Pull { commit, ci, open, merged, mergeable } => {
            out.u8(2)?;
            out.raw(commit)?;
            put_ci(out, ci, sizes)?;
            out.bool(*open)?;
            match merged {
                Some(value) => {
                    out.u8(1)?;
                    out.raw(value)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            out.bool(*mergeable)?;
        }
    }
    Some(())
}
pub(crate) fn get_news(input: &mut Reader<'_>, sizes: &Sizes) -> Option<News> {
    Some(match input.u8()? {
        0 => News::Comment { on: input.u64()?, id: input.u64()?, author: input.u64()? },
        1 => News::Reviews { commit: *input.bytes(32)?.first_chunk::<32>()? },
        2 => News::Pull {
            commit: *input.bytes(32)?.first_chunk::<32>()?,
            ci: get_ci(input, sizes)?,
            open: p::boolean(input)?,
            merged: match input.u8()? {
                0 => None,
                1 => Some(*input.bytes(32)?.first_chunk::<32>()?),
                _ => return None,
            },
            mergeable: p::boolean(input)?,
        },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Inbound {
    News { news: News },
    Finished { item: Item },
    Held { item: Item },
    Decided { accepted: bool },
}
pub(crate) fn put_inbound(out: &mut Encoder, value: &Inbound, sizes: &Sizes) -> Option<()> {
    match value {
        Inbound::News { news } => {
            out.u8(0)?;
            put_news(out, news, sizes)?;
        }
        Inbound::Finished { item } => {
            out.u8(1)?;
            put_item(out, item, sizes)?;
        }
        Inbound::Held { item } => {
            out.u8(2)?;
            put_item(out, item, sizes)?;
        }
        Inbound::Decided { accepted } => {
            out.u8(3)?;
            out.bool(*accepted)?;
        }
    }
    Some(())
}
pub(crate) fn get_inbound(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Inbound> {
    Some(match input.u8()? {
        0 => Inbound::News { news: get_news(input, sizes)? },
        1 => Inbound::Finished { item: get_item(input, sizes)? },
        2 => Inbound::Held { item: get_item(input, sizes)? },
        3 => Inbound::Decided { accepted: p::boolean(input)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FactKind {
    Text,
    Progress,
    Call,
    Tool,
    Usage,
}
pub(crate) fn put_fact_kind(out: &mut Encoder, value: &FactKind, _sizes: &Sizes) -> Option<()> {
    match value {
        FactKind::Text => {
            out.u8(0)?;
        }
        FactKind::Progress => {
            out.u8(1)?;
        }
        FactKind::Call => {
            out.u8(2)?;
        }
        FactKind::Tool => {
            out.u8(3)?;
        }
        FactKind::Usage => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_fact_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<FactKind> {
    Some(match input.u8()? {
        0 => FactKind::Text,
        1 => FactKind::Progress,
        2 => FactKind::Call,
        3 => FactKind::Tool,
        4 => FactKind::Usage,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Read {
    Item { item: Item, after: u64 },
    Pull { item: Item },
    Reviews { item: Item, page: u32 },
    Remarks { item: Item, review: u64, page: u32 },
    PullFor { repository: u32, head: Box<[u8]>, base: Box<[u8]> },
    Statuses { repository: u32, commit: [u8; 32], page: u32 },
    Permission { repository: u32, user: u64 },
    Branch { repository: u32, branch: Box<[u8]> },
    Pages { repository: u32, after: Option<Box<[u8]>> },
    Page { repository: u32, name: Box<[u8]> },
}
pub(crate) fn put_read(out: &mut Encoder, value: &Read, sizes: &Sizes) -> Option<()> {
    match value {
        Read::Item { item, after } => {
            out.u8(0)?;
            put_item(out, item, sizes)?;
            out.u64(*after)?;
        }
        Read::Pull { item } => {
            out.u8(1)?;
            put_item(out, item, sizes)?;
        }
        Read::Reviews { item, page } => {
            out.u8(2)?;
            put_item(out, item, sizes)?;
            out.u32(*page)?;
        }
        Read::Remarks { item, review, page } => {
            out.u8(3)?;
            put_item(out, item, sizes)?;
            out.u64(*review)?;
            out.u32(*page)?;
        }
        Read::PullFor { repository, head, base } => {
            out.u8(4)?;
            out.u32(*repository)?;
            out.bytes(head, sizes.name_bytes)?;
            out.bytes(base, sizes.name_bytes)?;
        }
        Read::Statuses { repository, commit, page } => {
            out.u8(5)?;
            out.u32(*repository)?;
            out.raw(commit)?;
            out.u32(*page)?;
        }
        Read::Permission { repository, user } => {
            out.u8(6)?;
            out.u32(*repository)?;
            out.u64(*user)?;
        }
        Read::Branch { repository, branch } => {
            out.u8(7)?;
            out.u32(*repository)?;
            out.bytes(branch, sizes.name_bytes)?;
        }
        Read::Pages { repository, after } => {
            out.u8(8)?;
            out.u32(*repository)?;
            match after {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.name_bytes)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        Read::Page { repository, name } => {
            out.u8(9)?;
            out.u32(*repository)?;
            out.bytes(name, sizes.name_bytes)?;
        }
    }
    Some(())
}
pub(crate) fn get_read(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Read> {
    Some(match input.u8()? {
        0 => Read::Item { item: get_item(input, sizes)?, after: input.u64()? },
        1 => Read::Pull { item: get_item(input, sizes)? },
        2 => Read::Reviews { item: get_item(input, sizes)?, page: input.u32()? },
        3 => Read::Remarks { item: get_item(input, sizes)?, review: input.u64()?, page: input.u32()? },
        4 => Read::PullFor {
            repository: input.u32()?,
            head: p::bytes(input, sizes.name_bytes)?,
            base: p::bytes(input, sizes.name_bytes)?,
        },
        5 => Read::Statuses {
            repository: input.u32()?,
            commit: *input.bytes(32)?.first_chunk::<32>()?,
            page: input.u32()?,
        },
        6 => Read::Permission { repository: input.u32()?, user: input.u64()? },
        7 => Read::Branch { repository: input.u32()?, branch: p::bytes(input, sizes.name_bytes)? },
        8 => Read::Pages {
            repository: input.u32()?,
            after: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.name_bytes)?),
                _ => return None,
            },
        },
        9 => Read::Page { repository: input.u32()?, name: p::bytes(input, sizes.name_bytes)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Scope {
    Deployment,
    Repository { repository: u32 },
    Goal { repository: u32, number: u64 },
}
pub(crate) fn put_scope(out: &mut Encoder, value: &Scope, _sizes: &Sizes) -> Option<()> {
    match value {
        Scope::Deployment => {
            out.u8(0)?;
        }
        Scope::Repository { repository } => {
            out.u8(1)?;
            out.u32(*repository)?;
        }
        Scope::Goal { repository, number } => {
            out.u8(2)?;
            out.u32(*repository)?;
            out.u64(*number)?;
        }
    }
    Some(())
}
pub(crate) fn get_scope(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Scope> {
    Some(match input.u8()? {
        0 => Scope::Deployment,
        1 => Scope::Repository { repository: input.u32()? },
        2 => Scope::Goal { repository: input.u32()?, number: input.u64()? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Author {
    Person { person: u64 },
    Run { repository: u32, number: u64 },
}
pub(crate) fn put_author(out: &mut Encoder, value: &Author, _sizes: &Sizes) -> Option<()> {
    match value {
        Author::Person { person } => {
            out.u8(0)?;
            out.u64(*person)?;
        }
        Author::Run { repository, number } => {
            out.u8(1)?;
            out.u32(*repository)?;
            out.u64(*number)?;
        }
    }
    Some(())
}
pub(crate) fn get_author(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Author> {
    Some(match input.u8()? {
        0 => Author::Person { person: input.u64()? },
        1 => Author::Run { repository: input.u32()?, number: input.u64()? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Recall {
    Name { scope: Scope, name: Box<[u8]> },
    Search { scopes: Scopes, query: Box<[u8]>, most: u32 },
}
pub(crate) fn put_recall(out: &mut Encoder, value: &Recall, sizes: &Sizes) -> Option<()> {
    match value {
        Recall::Name { scope, name } => {
            out.u8(0)?;
            put_scope(out, scope, sizes)?;
            out.bytes(name, sizes.name_bytes)?;
        }
        Recall::Search { scopes, query, most } => {
            out.u8(1)?;
            put_scopes(out, scopes, sizes)?;
            out.bytes(query, sizes.call)?;
            out.u32(*most)?;
        }
    }
    Some(())
}
pub(crate) fn get_recall(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Recall> {
    Some(match input.u8()? {
        0 => Recall::Name { scope: get_scope(input, sizes)?, name: p::bytes(input, sizes.name_bytes)? },
        1 => Recall::Search {
            scopes: get_scopes(input, sizes)?,
            query: p::bytes(input, sizes.call)?,
            most: input.u32()?,
        },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    New { page: Page },
    Revise { page: Page, revision: u64 },
    Remove,
}
pub(crate) fn put_change(out: &mut Encoder, value: &Change, sizes: &Sizes) -> Option<()> {
    match value {
        Change::New { page } => {
            out.u8(0)?;
            put_page(out, page, sizes)?;
        }
        Change::Revise { page, revision } => {
            out.u8(1)?;
            put_page(out, page, sizes)?;
            out.u64(*revision)?;
        }
        Change::Remove => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_change(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Change> {
    Some(match input.u8()? {
        0 => Change::New { page: get_page(input, sizes)? },
        1 => Change::Revise { page: get_page(input, sizes)?, revision: input.u64()? },
        2 => Change::Remove,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Call {
    Read { read: Read },
    Recall { recall: Recall },
    Note { scope: Scope, name: Box<[u8]>, change: Change },
    Comment { text: Box<[u8]> },
    Escalate { text: Box<[u8]> },
}
pub(crate) fn put_call(out: &mut Encoder, value: &Call, sizes: &Sizes) -> Option<()> {
    match value {
        Call::Read { read } => {
            out.u8(0)?;
            put_read(out, read, sizes)?;
        }
        Call::Recall { recall } => {
            out.u8(1)?;
            put_recall(out, recall, sizes)?;
        }
        Call::Note { scope, name, change } => {
            out.u8(2)?;
            put_scope(out, scope, sizes)?;
            out.bytes(name, sizes.name_bytes)?;
            put_change(out, change, sizes)?;
        }
        Call::Comment { text } => {
            out.u8(3)?;
            out.bytes(text, sizes.call)?;
        }
        Call::Escalate { text } => {
            out.u8(4)?;
            out.bytes(text, sizes.call)?;
        }
    }
    Some(())
}
pub(crate) fn get_call(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Call> {
    Some(match input.u8()? {
        0 => Call::Read { read: get_read(input, sizes)? },
        1 => Call::Recall { recall: get_recall(input, sizes)? },
        2 => Call::Note {
            scope: get_scope(input, sizes)?,
            name: p::bytes(input, sizes.name_bytes)?,
            change: get_change(input, sizes)?,
        },
        3 => Call::Comment { text: p::bytes(input, sizes.call)? },
        4 => Call::Escalate { text: p::bytes(input, sizes.call)? },
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Noted {
    Done,
    Missing,
    Exists,
    Moved,
    Unavailable,
}
pub(crate) fn put_noted(out: &mut Encoder, value: &Noted, _sizes: &Sizes) -> Option<()> {
    match value {
        Noted::Done => {
            out.u8(0)?;
        }
        Noted::Missing => {
            out.u8(1)?;
        }
        Noted::Exists => {
            out.u8(2)?;
        }
        Noted::Moved => {
            out.u8(3)?;
        }
        Noted::Unavailable => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_noted(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Noted> {
    Some(match input.u8()? {
        0 => Noted::Done,
        1 => Noted::Missing,
        2 => Noted::Exists,
        3 => Noted::Moved,
        4 => Noted::Unavailable,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unserved {
    Ungranted,
    Busy,
    Invalid,
    Refused,
    Failed,
}
pub(crate) fn put_unserved(out: &mut Encoder, value: &Unserved, _sizes: &Sizes) -> Option<()> {
    match value {
        Unserved::Ungranted => {
            out.u8(0)?;
        }
        Unserved::Busy => {
            out.u8(1)?;
        }
        Unserved::Invalid => {
            out.u8(2)?;
        }
        Unserved::Refused => {
            out.u8(3)?;
        }
        Unserved::Failed => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_unserved(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Unserved> {
    Some(match input.u8()? {
        0 => Unserved::Ungranted,
        1 => Unserved::Busy,
        2 => Unserved::Invalid,
        3 => Unserved::Refused,
        4 => Unserved::Failed,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeKind {
    Issue,
    Pull,
}
pub(crate) fn put_forge_kind(out: &mut Encoder, value: &ForgeKind, _sizes: &Sizes) -> Option<()> {
    match value {
        ForgeKind::Issue => {
            out.u8(0)?;
        }
        ForgeKind::Pull => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_forge_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<ForgeKind> {
    Some(match input.u8()? {
        0 => ForgeKind::Issue,
        1 => ForgeKind::Pull,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    Open,
    Closed,
}
pub(crate) fn put_state(out: &mut Encoder, value: &State, _sizes: &Sizes) -> Option<()> {
    match value {
        State::Open => {
            out.u8(0)?;
        }
        State::Closed => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_state(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<State> {
    Some(match input.u8()? {
        0 => State::Open,
        1 => State::Closed,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Permission {
    None_,
    Read,
    Write,
    Admin,
}
pub(crate) fn put_permission(out: &mut Encoder, value: &Permission, _sizes: &Sizes) -> Option<()> {
    match value {
        Permission::None_ => {
            out.u8(0)?;
        }
        Permission::Read => {
            out.u8(1)?;
        }
        Permission::Write => {
            out.u8(2)?;
        }
        Permission::Admin => {
            out.u8(3)?;
        }
    }
    Some(())
}
pub(crate) fn get_permission(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Permission> {
    Some(match input.u8()? {
        0 => Permission::None_,
        1 => Permission::Read,
        2 => Permission::Write,
        3 => Permission::Admin,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mark {
    None_,
    Key { key: Box<[u8]>, person: Option<u64> },
    Record { position: Position, nonce: u64 },
    Mangled,
}
pub(crate) fn put_mark(out: &mut Encoder, value: &Mark, sizes: &Sizes) -> Option<()> {
    match value {
        Mark::None_ => {
            out.u8(0)?;
        }
        Mark::Key { key, person } => {
            out.u8(1)?;
            out.bytes(key, sizes.answer)?;
            match person {
                Some(value) => {
                    out.u8(1)?;
                    out.u64(*value)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        Mark::Record { position, nonce } => {
            out.u8(2)?;
            put_position(out, position, sizes)?;
            out.u64(*nonce)?;
        }
        Mark::Mangled => {
            out.u8(3)?;
        }
    }
    Some(())
}
pub(crate) fn get_mark(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Mark> {
    Some(match input.u8()? {
        0 => Mark::None_,
        1 => Mark::Key {
            key: p::bytes(input, sizes.answer)?,
            person: match input.u8()? {
                0 => None,
                1 => Some(input.u64()?),
                _ => return None,
            },
        },
        2 => Mark::Record { position: get_position(input, sizes)?, nonce: input.u64()? },
        3 => Mark::Mangled,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeVerdict {
    Approve,
    RequestChanges,
    Comment,
}
pub(crate) fn put_forge_verdict(out: &mut Encoder, value: &ForgeVerdict, _sizes: &Sizes) -> Option<()> {
    match value {
        ForgeVerdict::Approve => {
            out.u8(0)?;
        }
        ForgeVerdict::RequestChanges => {
            out.u8(1)?;
        }
        ForgeVerdict::Comment => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_forge_verdict(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<ForgeVerdict> {
    Some(match input.u8()? {
        0 => ForgeVerdict::Approve,
        1 => ForgeVerdict::RequestChanges,
        2 => ForgeVerdict::Comment,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Check {
    Pending,
    Passed,
    Failed,
}
pub(crate) fn put_check(out: &mut Encoder, value: &Check, _sizes: &Sizes) -> Option<()> {
    match value {
        Check::Pending => {
            out.u8(0)?;
        }
        Check::Passed => {
            out.u8(1)?;
        }
        Check::Failed => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_check(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Check> {
    Some(match input.u8()? {
        0 => Check::Pending,
        1 => Check::Passed,
        2 => Check::Failed,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeAnswer {
    Items { items: Box<[Summary]>, more: bool, now: u64 },
    Item { item: Summary, comments: Box<[Comment]>, more: bool },
    Comment { comment: Comment },
    Pull { pull: Pull },
    Reviews { reviews: Box<[ForgeReview]>, more: bool },
    Statuses { ci: Ci, statuses: Box<[Status]>, more: bool },
    Remarks { remarks: Box<[Remark]>, more: bool },
    Permission { permission: Permission },
    Commit { commit: [u8; 32] },
    Pages { pages: Box<[PageName]>, next: Option<Box<[u8]>> },
    Page { page: ForgePage },
    Created { number: u64 },
    Commented { id: u64, revision: u64 },
    Edited { revision: u64 },
    Reviewed { review: u64 },
    Merged { commit: [u8; 32] },
    Revision { revision: u64 },
    Done,
}
#[expect(clippy::too_many_lines, reason = "one exhaustive table mirrors the forge answer vocabulary")]
pub(crate) fn put_forge_answer(out: &mut Encoder, value: &ForgeAnswer, sizes: &Sizes) -> Option<()> {
    match value {
        ForgeAnswer::Items { items, more, now } => {
            out.u8(0)?;
            let count_43 = u32::try_from(items.len()).ok()?;
            if count_43 > sizes.entries {
                return None;
            }
            out.u32(count_43)?;
            for value in items {
                put_summary(out, value, sizes)?;
            }
            out.bool(*more)?;
            out.u64(*now)?;
        }
        ForgeAnswer::Item { item, comments, more } => {
            out.u8(1)?;
            put_summary(out, item, sizes)?;
            let count_44 = u32::try_from(comments.len()).ok()?;
            if count_44 > sizes.entries {
                return None;
            }
            out.u32(count_44)?;
            for value in comments {
                put_comment(out, value, sizes)?;
            }
            out.bool(*more)?;
        }
        ForgeAnswer::Comment { comment } => {
            out.u8(2)?;
            put_comment(out, comment, sizes)?;
        }
        ForgeAnswer::Pull { pull } => {
            out.u8(3)?;
            put_pull(out, pull, sizes)?;
        }
        ForgeAnswer::Reviews { reviews, more } => {
            out.u8(4)?;
            let count_45 = u32::try_from(reviews.len()).ok()?;
            if count_45 > sizes.entries {
                return None;
            }
            out.u32(count_45)?;
            for value in reviews {
                put_forge_review(out, value, sizes)?;
            }
            out.bool(*more)?;
        }
        ForgeAnswer::Statuses { ci, statuses, more } => {
            out.u8(5)?;
            put_ci(out, ci, sizes)?;
            let count_46 = u32::try_from(statuses.len()).ok()?;
            if count_46 > sizes.entries {
                return None;
            }
            out.u32(count_46)?;
            for value in statuses {
                put_status(out, value, sizes)?;
            }
            out.bool(*more)?;
        }
        ForgeAnswer::Remarks { remarks, more } => {
            out.u8(6)?;
            let count_47 = u32::try_from(remarks.len()).ok()?;
            if count_47 > sizes.entries {
                return None;
            }
            out.u32(count_47)?;
            for value in remarks {
                put_remark(out, value, sizes)?;
            }
            out.bool(*more)?;
        }
        ForgeAnswer::Permission { permission } => {
            out.u8(7)?;
            put_permission(out, permission, sizes)?;
        }
        ForgeAnswer::Commit { commit } => {
            out.u8(8)?;
            out.raw(commit)?;
        }
        ForgeAnswer::Pages { pages, next } => {
            out.u8(9)?;
            let count_48 = u32::try_from(pages.len()).ok()?;
            if count_48 > sizes.entries {
                return None;
            }
            out.u32(count_48)?;
            for value in pages {
                put_page_name(out, value, sizes)?;
            }
            match next {
                Some(value) => {
                    out.u8(1)?;
                    out.bytes(value, sizes.name_bytes)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        ForgeAnswer::Page { page } => {
            out.u8(10)?;
            put_forge_page(out, page, sizes)?;
        }
        ForgeAnswer::Created { number } => {
            out.u8(11)?;
            out.u64(*number)?;
        }
        ForgeAnswer::Commented { id, revision } => {
            out.u8(12)?;
            out.u64(*id)?;
            out.u64(*revision)?;
        }
        ForgeAnswer::Edited { revision } => {
            out.u8(13)?;
            out.u64(*revision)?;
        }
        ForgeAnswer::Reviewed { review } => {
            out.u8(14)?;
            out.u64(*review)?;
        }
        ForgeAnswer::Merged { commit } => {
            out.u8(15)?;
            out.raw(commit)?;
        }
        ForgeAnswer::Revision { revision } => {
            out.u8(16)?;
            out.u64(*revision)?;
        }
        ForgeAnswer::Done => {
            out.u8(17)?;
        }
    }
    Some(())
}
pub(crate) fn get_forge_answer(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgeAnswer> {
    Some(match input.u8()? {
        0 => ForgeAnswer::Items {
            items: {
                let count_49 = p::count(input, sizes.entries, 39)?;
                let mut values_49 = List::with_capacity(count_49);
                for _ in 0..count_49 {
                    let value = get_summary(input, sizes)?;
                    values_49.push(value).expect("the validated count reserves capacity");
                }
                values_49.into_boxed()
            },
            more: p::boolean(input)?,
            now: input.u64()?,
        },
        1 => ForgeAnswer::Item {
            item: get_summary(input, sizes)?,
            comments: {
                let count_50 = p::count(input, sizes.entries, 37)?;
                let mut values_50 = List::with_capacity(count_50);
                for _ in 0..count_50 {
                    let value = get_comment(input, sizes)?;
                    values_50.push(value).expect("the validated count reserves capacity");
                }
                values_50.into_boxed()
            },
            more: p::boolean(input)?,
        },
        2 => ForgeAnswer::Comment { comment: get_comment(input, sizes)? },
        3 => ForgeAnswer::Pull { pull: get_pull(input, sizes)? },
        4 => ForgeAnswer::Reviews {
            reviews: {
                let count_51 = p::count(input, sizes.entries, 54)?;
                let mut values_51 = List::with_capacity(count_51);
                for _ in 0..count_51 {
                    let value = get_forge_review(input, sizes)?;
                    values_51.push(value).expect("the validated count reserves capacity");
                }
                values_51.into_boxed()
            },
            more: p::boolean(input)?,
        },
        5 => ForgeAnswer::Statuses {
            ci: get_ci(input, sizes)?,
            statuses: {
                let count_52 = p::count(input, sizes.entries, 13)?;
                let mut values_52 = List::with_capacity(count_52);
                for _ in 0..count_52 {
                    let value = get_status(input, sizes)?;
                    values_52.push(value).expect("the validated count reserves capacity");
                }
                values_52.into_boxed()
            },
            more: p::boolean(input)?,
        },
        6 => ForgeAnswer::Remarks {
            remarks: {
                let count_53 = p::count(input, sizes.entries, 28)?;
                let mut values_53 = List::with_capacity(count_53);
                for _ in 0..count_53 {
                    let value = get_remark(input, sizes)?;
                    values_53.push(value).expect("the validated count reserves capacity");
                }
                values_53.into_boxed()
            },
            more: p::boolean(input)?,
        },
        7 => ForgeAnswer::Permission { permission: get_permission(input, sizes)? },
        8 => ForgeAnswer::Commit { commit: *input.bytes(32)?.first_chunk::<32>()? },
        9 => ForgeAnswer::Pages {
            pages: {
                let count_54 = p::count(input, sizes.entries, 12)?;
                let mut values_54 = List::with_capacity(count_54);
                for _ in 0..count_54 {
                    let value = get_page_name(input, sizes)?;
                    values_54.push(value).expect("the validated count reserves capacity");
                }
                values_54.into_boxed()
            },
            next: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.name_bytes)?),
                _ => return None,
            },
        },
        10 => ForgeAnswer::Page { page: get_forge_page(input, sizes)? },
        11 => ForgeAnswer::Created { number: input.u64()? },
        12 => ForgeAnswer::Commented { id: input.u64()?, revision: input.u64()? },
        13 => ForgeAnswer::Edited { revision: input.u64()? },
        14 => ForgeAnswer::Reviewed { review: input.u64()? },
        15 => ForgeAnswer::Merged { commit: *input.bytes(32)?.first_chunk::<32>()? },
        16 => ForgeAnswer::Revision { revision: input.u64()? },
        17 => ForgeAnswer::Done,
        _ => return None,
    })
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Served {
    Read { answer: ForgeAnswer },
    Recalled { entries: Box<[Entry]>, failed: u32 },
    Noted { noted: Noted },
    Posted { comment: u64 },
    Unserved { reason: Unserved },
}
pub(crate) fn put_served(out: &mut Encoder, value: &Served, sizes: &Sizes) -> Option<()> {
    match value {
        Served::Read { answer } => {
            out.u8(0)?;
            put_forge_answer(out, answer, sizes)?;
        }
        Served::Recalled { entries, failed } => {
            out.u8(1)?;
            let count_55 = u32::try_from(entries.len()).ok()?;
            if count_55 > sizes.entries {
                return None;
            }
            out.u32(count_55)?;
            for value in entries {
                put_entry(out, value, sizes)?;
            }
            out.u32(*failed)?;
        }
        Served::Noted { noted } => {
            out.u8(2)?;
            put_noted(out, noted, sizes)?;
        }
        Served::Posted { comment } => {
            out.u8(3)?;
            out.u64(*comment)?;
        }
        Served::Unserved { reason } => {
            out.u8(4)?;
            put_unserved(out, reason, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_served(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Served> {
    Some(match input.u8()? {
        0 => Served::Read { answer: get_forge_answer(input, sizes)? },
        1 => Served::Recalled {
            entries: {
                let count_56 = p::count(input, sizes.entries, 34)?;
                let mut values_56 = List::with_capacity(count_56);
                for _ in 0..count_56 {
                    let value = get_entry(input, sizes)?;
                    values_56.push(value).expect("the validated count reserves capacity");
                }
                values_56.into_boxed()
            },
            failed: input.u32()?,
        },
        2 => Served::Noted { noted: get_noted(input, sizes)? },
        3 => Served::Posted { comment: input.u64()? },
        4 => Served::Unserved { reason: get_unserved(input, sizes)? },
        _ => return None,
    })
}
/// Encode a complete charter, refusing values outside the configured bounds.
#[must_use]
pub fn encode_charter(value: &Charter, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut measure = Encoder::measure();
    put_charter(&mut measure, value, sizes)?;
    if measure.length() > sizes.charter {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_charter(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one charter; trailing bytes are rejected.
#[must_use]
pub fn decode_charter(bytes: &[u8], sizes: &Sizes) -> Option<Charter> {
    if u32::try_from(bytes.len()).ok()? > sizes.charter {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_charter(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
/// Encode a complete outcome, refusing values outside the configured bounds.
#[must_use]
pub fn encode_outcome(value: &Outcome, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut measure = Encoder::measure();
    put_outcome(&mut measure, value, sizes)?;
    if measure.length() > sizes.outcome {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_outcome(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one outcome; trailing bytes are rejected.
#[must_use]
pub fn decode_outcome(bytes: &[u8], sizes: &Sizes) -> Option<Outcome> {
    if u32::try_from(bytes.len()).ok()? > sizes.outcome {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_outcome(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
/// Encode a complete inbound, refusing values outside the configured bounds.
#[must_use]
pub fn encode_inbound(value: &Inbound, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut measure = Encoder::measure();
    put_inbound(&mut measure, value, sizes)?;
    if measure.length() > sizes.inbound {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_inbound(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one inbound; trailing bytes are rejected.
#[must_use]
pub fn decode_inbound(bytes: &[u8], sizes: &Sizes) -> Option<Inbound> {
    if u32::try_from(bytes.len()).ok()? > sizes.inbound {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_inbound(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
/// Encode a complete call, refusing values outside the configured bounds.
#[must_use]
pub fn encode_call(value: &Call, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut measure = Encoder::measure();
    put_call(&mut measure, value, sizes)?;
    if measure.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_call(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one call; trailing bytes are rejected.
#[must_use]
pub fn decode_call(bytes: &[u8], sizes: &Sizes) -> Option<Call> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_call(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
/// Encode a complete served, refusing values outside the configured bounds.
#[must_use]
pub fn encode_served(value: &Served, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut measure = Encoder::measure();
    put_served(&mut measure, value, sizes)?;
    if measure.length() > sizes.answer {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_served(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one served; trailing bytes are rejected.
#[must_use]
pub fn decode_served(bytes: &[u8], sizes: &Sizes) -> Option<Served> {
    if u32::try_from(bytes.len()).ok()? > sizes.answer {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_served(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
/// Encode a complete fact, refusing values outside the configured bounds.
#[must_use]
pub fn encode_fact(value: &Fact, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut measure = Encoder::measure();
    put_fact(&mut measure, value, sizes)?;
    if measure.length() > sizes.fact {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_fact(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one fact; trailing bytes are rejected.
#[must_use]
pub fn decode_fact(bytes: &[u8], sizes: &Sizes) -> Option<Fact> {
    if u32::try_from(bytes.len()).ok()? > sizes.fact {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_fact(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
/// Encode a complete snapshot, refusing values outside the configured bounds.
#[must_use]
pub fn encode_snapshot(value: &Snapshot, sizes: &Sizes) -> Option<Box<[u8]>> {
    if value.version != 1 {
        return None;
    }
    let mut measure = Encoder::measure();
    put_snapshot(&mut measure, value, sizes)?;
    if measure.length() > sizes.snapshot {
        return None;
    }
    let mut out = Encoder::writing(measure.length());
    put_snapshot(&mut out, value, sizes)?;
    Some(out.finish())
}
/// Decode exactly one snapshot; trailing bytes are rejected.
#[must_use]
pub fn decode_snapshot(bytes: &[u8], sizes: &Sizes) -> Option<Snapshot> {
    if u32::try_from(bytes.len()).ok()? > sizes.snapshot {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_snapshot(&mut input, sizes)?;
    if value.version != 1 {
        return None;
    }
    if input.is_empty() { Some(value) } else { None }
}
