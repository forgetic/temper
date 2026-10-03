use alloc::boxed::Box;
use core::mem::size_of;

use skein_lib::{Env, Id, List, Map, Queue, Slab, Time, Token, Wall};
use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_forge as forge;
use temper_engine_domain_notes as notes;
use temper_engine_domain_plan as plan;
use temper_engine_domain_rules as rules;
use temper_engine_domain_views as views;
use temper_engine_domain_work as work;

use crate::config::Config;
use crate::facts::Fact;
use crate::items::{Entry, Noted};
use crate::waits::{Carried, Wait};

/// The engine domain's limits (programming-model.md, section 7), handed to
/// every step read-only: its child domains', each handed down to the one it
/// bounds, and the top level's own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub work: work::Limits,
    pub plan: plan::Limits,
    pub rules: rules::Limits,
    pub forge: forge::Limits,
    pub fleet: fleet::Limits,
    pub brief: brief::Limits,
    pub notes: notes::Limits,
    pub views: views::Limits,
    /// People's calls in hand at once. One beyond them is refused as busy.
    pub asks: u32,
    /// The most bytes of what the domain writes on the forge from a person
    /// or a run: a message, a title, a comment, an outcome's words.
    pub text_bytes: u32,
    /// The configured model selection copied into each run's charter.
    pub models_bytes: u32,
    /// The most child domain steps an entry point takes of each child domain,
    /// as its hand-offs cascade (the `route` module): what sizes the queues
    /// that hold their requests until they are routed.
    pub steps: u32,
    /// Facts kept until the loop drains them, beyond the child domains' own.
    pub facts: u32,
}

/// The most memory the domain holds under `limits`, in bytes
/// (programming-model.md, 6.3), or `None` if it does not fit a `u64` or the
/// limits cannot be honoured: the child domains' own, or limits under which
/// one child domain would refuse what another passes it within its own. The
/// hub and the forge child domain hold the same working set; the fleet tracks
/// one attempt per item, and room for what workers list; the rules know
/// every repository the plan does; and a step bound must leave room for an
/// entry point's first hand-offs.
///
/// It is the child domains', plus what the top level keeps: an entry for each
/// item, with its record's parts and its inbox; what waits for an answer;
/// the payloads it carries for the fleet; the queues that hold what each
/// child domain emits in an entry point until it is routed; and the facts.
/// The bytes of what an entry holds (its step, its relations, a charter) are
/// counted by their bounds.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let fits = limits.work.items == limits.forge.items
        && limits.work.items <= limits.fleet.attempts
        && limits.rules.repositories >= limits.forge.repositories
        && limits.steps >= 2
        && limits.fleet.workstream_bytes >= crate::translate::WORKSTREAM_BYTES
        && limits.asks > 0;
    if !fits {
        return None;
    }
    let children = work::worst_case(&limits.work)?
        .checked_add(plan::worst_case(&limits.plan)?)?
        .checked_add(rules::worst_case(&limits.rules)?)?
        .checked_add(forge::worst_case(&limits.forge)?)?
        .checked_add(fleet::worst_case(&limits.fleet)?)?
        .checked_add(brief::worst_case(&limits.brief)?)?
        .checked_add(notes::worst_case(&limits.notes)?)?
        .checked_add(views::worst_case(&limits.views)?)?;
    let items = limits.work.items;
    let entry = entry_bytes(limits)?;
    let table = Slab::<Entry>::worst_case(entries(limits)?)?
        .checked_add(Map::<work::Item, Id<Entry>>::worst_case(items)?)?
        .checked_add(u64::from(entries(limits)?).checked_mul(entry)?)?;
    let waited = Slab::<Wait>::worst_case(waits(limits)?)?
        .checked_add(u64::from(waits(limits)?).checked_mul(wait_bytes(limits)?)?)?;
    let retried = Queue::<Id<Entry>>::worst_case(items)?.checked_add(Queue::<Id<Entry>>::worst_case(items)?)?;
    let carried = Slab::<Carried>::worst_case(carried(limits)?)?
        .checked_add(u64::from(carried(limits)?).checked_mul(u64::from(limits.text_bytes))?)?;
    let handed = Map::<(work::Item, u64), Token>::worst_case(limits.fleet.attempts)?;
    let starts = Queue::<work::Item>::worst_case(items)?;
    let roomless = Queue::<work::Item>::worst_case(items)?;
    let queues = Queue::<work::Request>::worst_case(work_out(limits))?
        .checked_add(Queue::<forge::Request>::worst_case(forge_out(limits))?)?
        .checked_add(Queue::<fleet::Request>::worst_case(fleet_out(limits))?)?
        .checked_add(Queue::<brief::Request>::worst_case(brief_out(limits))?)?
        .checked_add(Queue::<notes::Request>::worst_case(notes_out(limits))?)?
        .checked_add(Queue::<views::Request>::worst_case(views_out(limits))?)?
        .checked_add(Queue::<plan::Write>::worst_case(plan::max_out(&limits.plan))?)?
        .checked_add(Queue::<rules::Finding>::worst_case(rules::max_out(&limits.rules))?)?;
    let facts = Queue::<Fact>::worst_case(facts(limits)?)?;
    // What the top level asks of the protocol itself as it routes, and the
    // people's watches being taken.
    let own =
        Queue::<crate::boundary::Request>::worst_case(routed(limits))?
            .checked_add(Map::<Token, skein_lib::ReplyTo>::worst_case(limits.asks)?)?;
    children
        .checked_add(config_bytes(limits)?)?
        .checked_add(table)?
        .checked_add(waited)?
        .checked_add(retried)?
        .checked_add(carried)?
        .checked_add(handed)?
        .checked_add(starts)?
        .checked_add(roomless)?
        .checked_add(queues)?
        .checked_add(own)?
        .checked_add(facts)
}

/// Whether `config` is one the domain can work under `limits`: the same
/// repositories for the plan, the rules and the forge, a home among them,
/// the rules within their own limits, and a session's step one the plan
/// could have written, so a session's record reads back sound.
#[must_use]
pub fn accepts(config: &Config, limits: &Limits) -> bool {
    let repositories = config.repositories();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.plan };
    repositories > 0
        && repositories == config.rules.repositories
        && repositories <= limits.forge.repositories
        && config.home < repositories
        && config.rules.fits(&limits.rules)
        && config.plan.fits(limits.forge.repositories, &limits.plan)
        && within(config.models.len(), limits.models_bytes)
        && forge_config_fits(&config.forge, &limits.forge)
        && within(config.session.charter.instructions.len(), limits.plan.instruction_bytes)
        && match &config.session.charter.template {
            Some(name) => within(name.len(), limits.plan.name_bytes),
            None => true,
        }
        && within(config.branches.len(), limits.forge.name_bytes)
        && within(config.saved.len(), limits.forge.name_bytes)
        && plan::check_record(&config.plan, &env, &config.session_step(0)).is_ok()
}

fn forge_config_fits(config: &forge::Config, limits: &forge::Limits) -> bool {
    if !within(config.tracking.len(), limits.name_bytes)
        || !within(config.hand_in.len(), limits.name_bytes)
        || !within(config.projected.len(), limits.labels)
    {
        return false;
    }
    for label in &config.projected {
        if !within(label.len(), limits.name_bytes) {
            return false;
        }
    }
    true
}

/// The root's retained configuration. Rules are counted by their child;
/// the forge child counts its own copy, so count the root's copy here too.
fn config_bytes(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.forge.name_bytes);
    let label = u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_add(name)?;
    let forge = name.checked_mul(2)?.checked_add(u64::from(limits.forge.labels).checked_mul(label)?)?;
    // The retained session and the temporary step made to validate it.
    let session =
        u64::from(limits.plan.instruction_bytes).checked_add(u64::from(limits.plan.name_bytes))?.checked_mul(2)?;
    plan::config_worst_case(limits.forge.repositories, &limits.plan)?
        .checked_add(forge)?
        .checked_add(session)?
        .checked_add(u64::from(limits.models_bytes))?
        .checked_add(name.checked_mul(2)?)
}

pub(crate) fn within(len: usize, most: u32) -> bool {
    match u32::try_from(len) {
        Ok(len) => len <= most,
        Err(_) => false,
    }
}

/// What one entry holds beyond its own size, by the bounds of what it holds:
/// its step and its goal's plan (names, dependencies, instructions), its
/// relations, its inbox, an assignment's charter and snapshot, the outcome
/// being applied, and the writes of an application.
fn entry_bytes(limits: &Limits) -> Option<u64> {
    let plan = &limits.plan;
    let name = u64::from(plan.name_bytes);
    let step = name
        .checked_mul(u64::from(plan.dependencies).checked_add(1)?)?
        .checked_add(u64::from(plan.instruction_bytes).checked_mul(2)?)?;
    let goal = u64::from(plan.steps).checked_mul(step)?;
    let relations = u64::from(plan.steps).checked_mul(name.checked_add(64)?)?.checked_mul(2)?;
    let inbox = List::<Noted>::worst_case(inbox(limits)?)?;
    let charter = u64::from(limits.brief.brief_bytes)
        .checked_add(u64::from(plan.instruction_bytes))?
        .checked_add(u64::from(limits.models_bytes))?
        .checked_add(u64::from(limits.fleet.workstream_bytes))?;
    let writes = u64::from(plan::max_out(plan)).checked_mul(step)?;
    // The outcome being applied: its words, and a plan or steps as large as
    // a goal's.
    let outcome = u64::from(limits.text_bytes).checked_add(goal)?;
    step.checked_mul(2)?
        .checked_add(outcome)?
        .checked_add(goal.checked_mul(2)?)?
        .checked_add(relations)?
        .checked_add(inbox)?
        .checked_add(charter)?
        .checked_add(writes)
}

/// What one wait holds beyond its own size: a payload to fill in (a page, a
/// text), or the pages of a listing gathered.
fn wait_bytes(limits: &Limits) -> Option<u64> {
    let notes = &limits.notes;
    let page = u64::from(notes.body_bytes).checked_add(u64::from(notes.description_bytes))?;
    let listing = u64::from(notes.entries).checked_mul(u64::from(notes.name_bytes).checked_add(32)?)?;
    Some(page.max(listing).max(u64::from(limits.text_bytes)))
}

/// The entries the table holds: one per item the hub holds, and as many
/// again of items it is done with whose records written on the side are
/// still on their way.
pub(crate) fn entries(limits: &Limits) -> Option<u32> {
    limits.work.items.checked_mul(2)
}

/// The inbox an entry keeps: as much news as the forge child domain holds for
/// an item, and its notices' share beside it.
pub(crate) fn inbox(limits: &Limits) -> Option<u32> {
    limits.forge.inbox.checked_add(notices(limits)?)
}

/// The notices an entry's inbox keeps, merged, one of each kind: one for
/// each related item done (its dependencies and its children, as many as a
/// plan's steps each) and for the item itself, one for each child held, and
/// a decision. One beyond them is dropped, never news.
pub(crate) fn notices(limits: &Limits) -> Option<u32> {
    limits.plan.steps.checked_mul(3)?.checked_add(2)
}

/// What may wait for an answer at once: three steps of each item's job (a
/// run prepared renders its brief, gets its snapshot and reads its branch)
/// or its take, a record written on the side of each entry, a read of each
/// section of each brief, an operation of the notes', a call of each run,
/// people's calls, and the store's operations for snapshots and traces.
pub(crate) fn waits(limits: &Limits) -> Option<u32> {
    let briefs = limits.brief.briefs.checked_mul(limits.brief.sections)?;
    let notes = limits.notes.scopes.checked_add(limits.notes.calls)?.checked_mul(2)?;
    let views = limits.views.appends.checked_add(1)?.checked_mul(2)?;
    limits
        .work
        .items
        .checked_mul(3)?
        .checked_add(entries(limits)?)?
        .checked_add(briefs)?
        .checked_add(notes)?
        .checked_add(limits.fleet.calls)?
        .checked_add(limits.asks)?
        .checked_add(views)
}

/// What the top level carries for the fleet at once: the answers it hands
/// up until they are acknowledged, the runs' calls and their answers, and a
/// run's fact on its way.
pub(crate) fn carried(limits: &Limits) -> Option<u32> {
    let tracked = limits.fleet.attempts.checked_add(limits.fleet.workers.checked_mul(limits.fleet.slots)?)?;
    tracked.checked_add(limits.fleet.calls.checked_mul(2)?)?.checked_add(1)
}

/// Facts kept until the loop drains them: as many as the child domains keep,
/// and the top level's own.
pub(crate) fn facts(limits: &Limits) -> Option<u32> {
    limits
        .work
        .facts
        .checked_add(limits.forge.facts)?
        .checked_add(limits.fleet.facts)?
        .checked_add(limits.brief.facts)?
        .checked_add(limits.notes.facts)?
        .checked_add(limits.views.facts)?
        .checked_add(limits.facts)
}

/// Room for what the hub emits in an entry point: its steps, each within its
/// bound.
pub(crate) const fn work_out(limits: &Limits) -> u32 {
    limits.steps.saturating_mul(work::max_out(&limits.work))
}

pub(crate) const fn forge_out(limits: &Limits) -> u32 {
    limits.steps.saturating_mul(forge::max_out(&limits.forge))
}

pub(crate) const fn fleet_out(limits: &Limits) -> u32 {
    limits.steps.saturating_mul(fleet::max_out(&limits.fleet))
}

pub(crate) const fn brief_out(limits: &Limits) -> u32 {
    limits.steps.saturating_mul(brief::max_out(&limits.brief))
}

pub(crate) const fn notes_out(limits: &Limits) -> u32 {
    limits.steps.saturating_mul(notes::MAX_OUT)
}

pub(crate) const fn views_out(limits: &Limits) -> u32 {
    limits.steps.saturating_mul(views::max_out(&limits.views))
}

/// The most requests the child domains emit in an entry point, each routed
/// once: to the protocol layer, or to a sibling.
pub(crate) const fn routed(limits: &Limits) -> u32 {
    work_out(limits)
        .saturating_add(forge_out(limits))
        .saturating_add(fleet_out(limits))
        .saturating_add(brief_out(limits))
        .saturating_add(notes_out(limits))
        .saturating_add(views_out(limits))
}
