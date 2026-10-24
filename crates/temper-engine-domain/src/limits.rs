//! Child limits and checked route and heap sums (jig's domain/root.md, 10).
use crate::boundary::{
    Assignment, BriefConnector, BriefSection, Event, Fact, Payload, PreparedWorkspace, Read, Request, Work,
};
use crate::domain::{Domain, max_out};
use crate::route::{LandingRule, forge_route, host_route};
use crate::{CallKey, Decision, JournalLimits, Output, loads};
use alloc::boxed::Box;
use jig_domain_accounts as accounts;
use jig_domain_authority as authority;
use jig_domain_brief as brief;
use jig_domain_core::{Model, RunCharter, RunPolicy};
use jig_domain_fleet as fleet;
use jig_domain_notes as notes;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use jig_domain_views as views;
use skein_lib::{Env, List, Map, Queue, ReplyTo, Slab, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_change as forge_change;

/// How many of the root's own facts wait to be drained.
pub const FACTS: u32 = 32;
/// A fresh run's transcript tail, in bytes, jig's small-profile figure; each
/// protocol-rendered turn shares this bound.
pub const TRANSCRIPT_TAIL_BYTES: u32 = 2048;

/// Root startup bounds, supplied by configuration and immutable at every step
/// (domain/root.md, 10). `worst_case` checks the cross-child route, page,
/// journal and payload capacities before `Domain::new` allocates fixed room.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Ordered commit/write/delivery room, including all synchronous child routes.
    pub journal: JournalLimits,
    /// Paged store reads; every startup page must fit whole.
    pub loads: loads::Limits,
    /// Authentic task/funding room, also bounding root current-claim proof slots; transport
    /// evidence belongs to root.
    pub tasks: tasks::Limits,
    /// Deployment/project policy and finding room, supplied at startup.
    pub authority: authority::Limits,
    /// Identity, session and keyed request room.
    pub people: people::Limits,
    /// Worker attempts and unacknowledged turns.
    pub fleet: fleet::Limits,
    /// Maximum named decisions awaiting a turn or task end across live tasks.
    pub call_records: u32,
    /// Required task section gathering and cuts.
    pub brief: BriefLimits,
    /// Secret-free credential policy.
    pub accounts: accounts::Limits,
    /// Bounded notes indexes and pages kept by the core.
    pub notes: notes::Limits,
    /// Configured typed document construction bounds and served versions.
    pub documents: jig_domain_core::documents::DocumentLimits,
    /// Maximum configured templates accepted by the core.
    pub templates: u32,
    /// Settings history and value bounds parsed by jig.
    pub settings: jig_domain_core::settings::Limits,
    /// Forge connector subtree and its bounded outbox.
    pub forge: forge::Limits,
}

/// Root brief configuration, including temper's source and forge budgets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BriefLimits {
    pub briefs: u32,
    pub sections: u32,
    pub parts: u32,
    pub read_bytes: u32,
    pub budgets: BriefBudgets,
    pub brief_bytes: u32,
    pub gather: skein_lib::Duration,
}

/// Byte ceilings for the root's core sections and its forge connector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BriefBudgets {
    pub task: u32,
    pub dependencies: u32,
    pub ci: u32,
    pub reviews: u32,
    pub pull: u32,
    pub attempts: u32,
    pub plan: u32,
    pub notes: u32,
}

pub(crate) fn root_journal_limits(limits: &Limits) -> skein_lib::JournalLimits {
    let mut journal = crate::journal_limits(&limits.journal);
    journal.now = max_out(limits);
    journal
}

pub(crate) fn environment_core(env: &Env<Limits>) -> Env<jig_domain_core::Limits> {
    Env { now: env.now, wall: env.wall, limits: core_limits(&env.limits) }
}

/// The store map's bytes at or above which new party work is refused, as
/// jig's `crates/jig-protocol-config/src/profile/small.rs` sets it until
/// temper's configuration names its own (domain/root.md, 10).
pub const FILL_MARK: u64 = 268_435_456;
/// Streams use temper's client's own budget of 16, above jig's small
/// profile's 4 (`small.rs`, clients; protocol/configuration.md, 7).
pub const WATCHES_PER_STREAM: u32 = 16;
/// Watches use the client's budget above jig's small profile's 4 (`small.rs`, views).
/// Live runs and provisional text retain jig's small profile's limits.
pub const VIEWS: views::Limits = views::Limits {
    runs: 4,
    watchers: 16,
    watchers_per_party: 16,
    subjects_per_change: 32,
    text_bytes: 65_536,
    facts: 32,
};

pub(crate) fn core_limits(limits: &Limits) -> jig_domain_core::Limits {
    jig_domain_core::Limits {
        fill_mark: FILL_MARK,
        load_bytes: limits.loads.read_bytes,
        documents: limits.documents,
        templates: limits.templates,
        settings: limits.settings,
        connectors: 1,
        resume_bytes: limits.journal.transcript_bytes,
        run_bytes: limits.journal.run_bytes,
        policy_bytes: u64::from(limits.journal.transcript_bytes).min(row_bound(limits).expect("validated row bound")),
        escalation_reason_bytes: limits
            .tasks
            .result_bytes
            .min(limits.journal.result_bytes)
            .min(limits.journal.transcript_bytes),
        tasks: limits.tasks,
        authority: limits.authority,
        load_slots: limits.loads.loads,
        call_records: limits.call_records,
        call_answer_bytes: limits.journal.transcript_bytes,
        people: limits.people,
        fleet: limits.fleet,
        brief: brief_limits(&limits.brief),
        brief_parts: limits.brief.parts,
        brief_core_budgets: jig_domain_core::CoreBriefBudgets {
            tail: TRANSCRIPT_TAIL_BYTES,
            task: limits.brief.budgets.task,
            dependencies: limits.brief.budgets.dependencies,
            attempts: limits.brief.budgets.attempts,
            plan: limits.brief.budgets.plan,
            notes: limits.brief.budgets.notes,
        },
        accounts: limits.accounts,
        notes: limits.notes,
        views: VIEWS,
    }
}

pub(crate) fn brief_limits(limits: &BriefLimits) -> brief::Limits {
    brief::Limits {
        briefs: limits.briefs,
        sections: limits.sections,
        read_bytes: limits.read_bytes,
        brief_bytes: limits.brief_bytes,
    }
}

pub(crate) fn payload_slots(limits: &Limits) -> Option<u32> {
    limits
        .fleet
        .turns
        .checked_add(limits.fleet.attempts)?
        .checked_mul(2)?
        .checked_add(limits.fleet.calls.checked_mul(2)?)
}

pub(crate) fn route_bound(limits: &Limits) -> Option<u32> {
    let routes = tasks::max_out(&limits.tasks)
        .checked_mul(8)?
        .checked_add(people::max_out(&limits.people).checked_mul(4)?)?
        // Every in-flight historical decision may complete under journal pressure:
        // one retained IO terminal and one people Decided callback per flight.
        .checked_add(limits.people.pending.checked_mul(2)?)?
        .checked_add(fleet::max_out(&limits.fleet).checked_mul(4)?)?
        .checked_add(forge::max_out(&limits.forge))?
        // One serialized role cohort revisits each Waiting task, then answers.
        .checked_add(limits.tasks.tasks.checked_add(4)?)?;
    Some(routes.max(jig_domain_core::settings::writes_worst_case(&core_limits(limits))?))
}

// The walking root drains its pending callbacks in the same decision as a newly
// admitted event. Thus any event may reach the full child route. The bound sums
// each child's maximum output cohort, the retained callbacks, call records and
// the deployment header; the configured journal may have more per-commit room.

pub(crate) fn route_room(limits: &Limits) -> skein_lib::JournalRoom {
    skein_lib::JournalRoom {
        writes: route_bound(limits)
            .expect("validated route bound")
            .checked_add(limits.call_records)
            .expect("validated call record bound"),
        held: limits.journal.deliveries,
    }
}

pub(crate) fn route_takes(domain: &Domain, limits: &Limits) -> bool {
    domain.journal.takes(&route_room(limits))
}

pub(crate) fn route_decision(domain: &mut Domain, limits: &Limits) -> Option<Decision> {
    domain.settled = None;
    Decision::reserve_room(&mut domain.journal, &limits.journal, route_room(limits))
}

/// Count participating child state, fixed handoffs, decoded input and all simultaneously retained
/// owned payloads, including current proofs, terminal scratch copies, transient restore
/// correlations and preparation contexts. Child limits validate before route capacity arithmetic;
/// malformed or unrepresentable bounds return `None`. Pure startup heap calculation, excluding
/// allocator overhead. Child declared output bounds must already be representable; checked root
/// arithmetic or refused child/cross-route bounds return `None`. It allocates no state and emits no
/// request or terminal.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one checked sum of the root's bounded participating state")]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.forge.tasks < limits.tasks.tasks
        || limits.brief.parts == 0
        || limits.brief.gather == skein_lib::Duration::ZERO
    {
        return None;
    }
    let brief_fetches = limits.brief.briefs.checked_mul(limits.brief.sections)?.checked_mul(2)?;
    if limits.forge.brief_sections < brief_fetches || limits.forge.brief_bytes < limits.brief.read_bytes {
        return None;
    }
    let core_bytes = jig_domain_core::worst_case(&core_limits(limits))?;
    // A saved settings revision owns a whole set. Both journal write cohorts and
    // the synchronous work queue may retain one per slot, independently of Current.
    let settings_row = jig_domain_core::settings::revision_worst_case(&core_limits(limits))?;
    let settings_writes = u64::from(limits.journal.writes)
        .checked_mul(2)?
        .checked_add(u64::from(route_bound(limits)?))?
        .checked_mul(settings_row)?;
    let forge_bytes = forge::worst_case(&limits.forge)?;
    let landing_bytes = Map::<u32, Box<[LandingRule]>>::worst_case(limits.authority.projects)?.checked_add(
        u64::from(limits.authority.projects).checked_add(1)?.checked_mul(u64::from(limits.journal.transcript_bytes))?,
    )?;
    let permission_bytes = Map::<u32, Box<[people::PermissionRole]>>::worst_case(limits.authority.projects)?
        .checked_add(u64::from(limits.authority.projects).checked_mul(u64::from(limits.journal.transcript_bytes))?)?;
    let load_bytes = loads::worst_case(&limits.loads)?;
    let routes = route_bound(limits)?;
    let tool_bytes = u64::from(limits.tasks.batch).checked_mul(row_bound(limits)?)?;
    if limits.forge.named_calls < limits.call_records
        || limits.journal.writes < routes
        || limits.call_records == 0
        || limits.call_records > limits.journal.writes.checked_sub(routes)?
        || limits.journal.deliveries
            < limits
                .tasks
                .tasks
                .checked_mul(4)?
                .checked_add(8)?
                .checked_add(limits.people.pending.checked_mul(limits.people.waiters)?)?
        || limits.loads.loads < 2
        || limits.loads.rows < 2
        || limits.journal.held < limits.journal.deliveries.checked_mul(3)?
        || limits.journal.deliveries
            < limits.fleet.workers.checked_mul(fleet::max_out(&limits.fleet))?.checked_add(1)?
        || limits.loads.rows.checked_add(limits.fleet.workers)? > routes
        || limits.journal.result_bytes < limits.brief.brief_bytes
        || limits.journal.deliveries < limits.brief.sections
        || limits.journal.deliveries < limits.tasks.saved_resources
        || limits.journal.result_bytes < limits.tasks.result_bytes
        || limits.journal.result_bytes < limits.people.words
        || limits.people.inbox_entries == 0
        || u64::from(limits.journal.transcript_bytes) < row_bound(limits)?
    {
        return None;
    }
    // Owners price all retained semantic state. The remaining terms are
    // bounded transport continuations and simultaneously assembled envelopes.
    let mut bytes = core_bytes
        .checked_add(u64::from(limits.forge.name_bytes).checked_mul(5)?)?
        .checked_add(u64::from(limits.forge.change_policy.gates).checked_mul(4)?)?
        .checked_add(
            u64::from(limits.journal.held).checked_add(u64::from(limits.journal.deliveries))?.checked_mul(
                u64::from(limits.forge.name_bytes)
                    .checked_mul(2)?
                    .checked_add(u64::from(limits.forge.client.answer_bytes))?,
            )?,
        )?
        .checked_add(forge_bytes)?
        .checked_add(settings_writes)?
        .checked_add(crate::journal_worst_case(&limits.journal)?)?
        .checked_add(load_bytes)?
        .checked_add(landing_bytes)?
        .checked_add(permission_bytes)?;
    // Checked Return paths can be owned by the journal and output queues, plus one admission.
    let returns = u64::from(limits.journal.held)
        .checked_add(u64::from(limits.journal.deliveries))?
        .checked_add(u64::from(max_out(limits)).checked_mul(2)?)?
        .checked_add(1)?;
    bytes = bytes.checked_add(returns.checked_mul(512)?)?;
    bytes = bytes.checked_add(Queue::<Fact>::worst_case(FACTS)?)?;
    bytes = bytes.checked_add(Map::<Token, forge::DemoAct>::worst_case(forge::demo_act_bound(&limits.forge)?)?)?;
    bytes = bytes.checked_add(returns.checked_mul(256)?)?;
    // The root selects into one owned event, while the forge takes that selection.
    bytes = bytes.checked_add(
        u64::from(forge::demo_act_bound(&limits.forge)?)
            .checked_mul(u64::try_from(size_of::<forge::DemoAct>()).ok()?)?
            .checked_mul(2)?,
    )?;
    bytes = bytes
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(u64::try_from(size_of::<tasks::ViewTask>()).ok()?)?)?;
    bytes = bytes.checked_add(Map::<Token, crate::route::demo::Context>::worst_case(limits.forge.applications)?)?;
    let effects = limits.call_records.checked_add(limits.tasks.tasks)?;
    let brief_fetches = limits.brief.briefs.checked_mul(limits.brief.sections)?.checked_mul(2)?;
    bytes = bytes
        .checked_add(Slab::<BriefConnector>::worst_case(brief_fetches)?)?
        .checked_add(Map::<u64, Box<[BriefSection]>>::worst_case(limits.tasks.tasks)?)?
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.brief.brief_bytes))?)?
        .checked_add(Map::<u64, PreparedWorkspace>::worst_case(limits.tasks.tasks)?)?
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(row_bound(limits)?.checked_mul(2)?)?)?
        .checked_add(Map::<Token, forge_route::EffectFlight>::worst_case(effects)?)?
        .checked_add(Map::<Token, (skein_lib::Time, skein_lib::Wall)>::worst_case(limits.people.requests)?)?
        .checked_add(u64::from(effects).checked_mul(row_bound(limits)?.checked_mul(3)?)?)?
        .checked_add(Map::<Token, Option<forge::Repository>>::worst_case(limits.forge.adoptions)?)?
        .checked_add(
            u64::from(limits.forge.adoptions).checked_mul(u64::from(limits.forge.name_bytes).checked_mul(4)?)?,
        )?
        .checked_add(Map::<Token, forge::Subscriber>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<Token, (u64, forge::Topic)>::worst_case(limits.fleet.calls)?)?
        .checked_add(
            u64::from(limits.fleet.calls)
                .checked_mul(u64::from(limits.forge.paths_per_subscription))?
                .checked_mul(u64::from(limits.forge.name_bytes))?,
        )?
        .checked_add(Map::<Token, (ReplyTo, CallKey)>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<u64, (ReplyTo, CallKey)>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<(u64, u64), forge_change::Delegate>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<Token, host_route::Flight>::worst_case(limits.fleet.calls)?)?
        .checked_add(
            u64::from(limits.fleet.calls).checked_mul(u64::from(limits.journal.transcript_bytes).checked_mul(4)?)?,
        )?;
    let slots = payload_slots(limits)?;
    bytes = bytes
        .checked_add(Slab::<Option<Payload>>::worst_case(slots)?)?
        .checked_add(
            u64::from(slots).checked_mul(
                u64::from(limits.journal.transcript_bytes)
                    .max(u64::from(limits.tasks.result_bytes).checked_mul(2)?)
                    .max(tool_bytes),
            )?,
        )?
        .checked_add(Slab::<Option<Read>>::worst_case(limits.loads.loads)?)?
        .checked_add(
            u64::from(limits.loads.loads).checked_mul(
                u64::from(limits.loads.reply_bytes)
                    .checked_add(u64::from(limits.people.words))?
                    // A transcript load carries one optional rendering per row beside its raw bytes.
                    .checked_add(List::<Option<Box<[u8]>>>::worst_case(limits.loads.rows)?)?
                    .checked_add(u64::from(limits.loads.rows).checked_mul(u64::from(TRANSCRIPT_TAIL_BYTES))?)?,
            )?,
        )?
        // Read answers that waited for journal room.
        .checked_add(Queue::<Event>::worst_case(limits.loads.loads)?)?
        .checked_add(
            u64::from(limits.loads.loads)
                .checked_mul(jig_domain_core::reads::answer_worst_case(&core_limits(limits))?)?,
        )?
        // A forge input the drain hands back waits boxed in the journal's door.
        .checked_add(u64::from(limits.journal.deliveries).checked_mul(
            u64::try_from(size_of::<forge::Event>()).ok()?.checked_add(u64::from(limits.forge.client.answer_bytes))?,
        )?)?
        .checked_add(Queue::<Work>::worst_case(routes)?)?
        // One admitted client ask moves its original names into Work::People and drains
        // in this route. Child callbacks do not enqueue more client asks. Restored answers
        // are already covered by row_bound below; core/people price their retained contexts.
        .checked_add(people::model_names_worst_case()?)?
        .checked_add(u64::from(routes).checked_mul(row_bound(limits)?)?)?;
    let pushed =
        u64::from(limits.forge.resources_per_task).checked_mul(u64::try_from(size_of::<forge::Pushed>()).ok()?)?;
    bytes = bytes
        .checked_add(Map::<(u64, u64), Box<[forge::Pushed]>>::worst_case(limits.tasks.tasks)?)?
        .checked_add(u64::from(limits.tasks.tasks).checked_add(u64::from(slots).checked_mul(2)?)?.checked_mul(pushed)?)?
        .checked_add(Map::<u64, Assignment>::worst_case(limits.tasks.tasks)?)?
        .checked_add(
            u64::from(limits.tasks.tasks).checked_add(u64::from(limits.journal.held))?.checked_mul(
                u64::from(limits.brief.brief_bytes)
                    .checked_add(List::<BriefSection>::worst_case(limits.brief.sections)?)?
                    .checked_add(u64::from(limits.tasks.inbox_bytes))?
                    .checked_add(
                        u64::from(limits.tasks.tasks)
                            .checked_mul(u64::from(limits.tasks.message_bytes).checked_add(32)?)?,
                    )?
                    .checked_add(u64::from(limits.tasks.saved_resources).checked_mul(8)?)?
                    .checked_add(u64::from(limits.journal.run_bytes))?
                    .checked_add(u64::from(limits.journal.transcript_bytes))?
                    .checked_add(List::<Box<[u8]>>::worst_case(limits.journal.transcript_bytes)?)?
                    .checked_add(
                        u64::from(limits.call_records).checked_mul(
                            u64::try_from(size_of::<crate::CallRecord>())
                                .ok()?
                                .checked_add(u64::from(limits.journal.transcript_bytes))?,
                        )?,
                    )?
                    .checked_add(
                        u64::from(limits.tasks.inbox_messages.checked_add(limits.tasks.tasks.checked_mul(2)?)?)
                            .checked_mul(u64::try_from(size_of::<tasks::Word>()).ok()?)?,
                    )?,
            )?,
        )?
        .checked_add(role_scratch_bytes(limits)?)?
        .checked_add(Queue::<Request>::worst_case(max_out(limits))?.checked_mul(2)?)?
        .checked_add(u64::try_from(size_of::<forge::Event>()).ok()?)?
        .checked_add(Queue::<Output>::worst_case(1)?)?
        .checked_add(Queue::<loads::Request>::worst_case(1)?)?;
    Some(bytes)
}
/// Conservative heap envelope for one input, output or restored page. The root's
/// complete state is an upper bound, but its independently retained model-name
/// contexts cannot also be owned by that message. People holds one per completed
/// ask and two per pending ask; core prices two synchronous transit contexts
/// (jig's people limits.rs and core memory.rs). Keep the root's one incoming
/// context: it covers an Ask's names, and restored names are bounded by page bytes.
#[must_use]
pub fn payload_worst_case(limits: &Limits) -> Option<u64> {
    let retained = u64::from(limits.people.requests)
        .checked_add(u64::from(limits.people.pending).checked_mul(2)?)?
        .checked_add(2)?;
    worst_case(limits)?.checked_sub(retained.checked_mul(people::model_names_worst_case()?)?)
}

pub(crate) fn role_scratch_bytes(limits: &Limits) -> Option<u64> {
    // Role administration owns one incoming/routed candidate and application
    // scratch copies independently of people's pending/completed asks. Semantic
    // inspection/recheck arrays contain Waiting contexts only (no reason bytes).
    let roster_bytes =
        u64::from(limits.people.holdings).checked_mul(u64::try_from(size_of::<people::Holding>()).ok()?)?;
    roster_bytes
        .checked_mul(4)?
        .checked_add(List::<tasks::EscalationContext>::worst_case(limits.tasks.tasks)?.checked_mul(2)?)?
        .checked_add(u64::try_from(size_of::<tasks::EscalationContext>()).ok()?)
}

pub(crate) fn run_policy_bytes(policy: &RunPolicy) -> Option<u64> {
    if policy.turns == 0
        || policy.waiting == skein_lib::Duration::ZERO
        || policy.time == skein_lib::Duration::ZERO
        || policy.call_timeout == skein_lib::Duration::ZERO
        || policy.model.name.is_empty()
        || policy.model.window == 0
        || policy.model.output == 0
        || policy.model.price_unit == 0
    {
        return None;
    }
    let mut bytes = u64::try_from(size_of::<RunCharter>())
        .ok()?
        .checked_add(u64::try_from(policy.instructions.len()).ok()?)?
        .checked_add(u64::try_from(policy.model.name.len()).ok()?)?
        .checked_add(
            u64::try_from(policy.alternatives.len()).ok()?.checked_mul(u64::try_from(size_of::<Model>()).ok()?)?,
        )?;
    for model in &policy.alternatives {
        if model.name.is_empty() || model.window == 0 || model.output == 0 || model.price_unit == 0 {
            return None;
        }
        bytes = bytes.checked_add(u64::try_from(model.name.len()).ok()?)?;
    }
    Some(bytes)
}

pub(crate) fn run_carriers_bound(limits: &Limits) -> Option<u64> {
    u64::from(limits.tasks.contract_choices)
        .checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?
        .checked_add(
            u64::from(limits.tasks.authority_grants).checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?)?,
        )?
        .checked_add(
            u64::from(limits.tasks.authority_grants)
                .checked_mul(u64::try_from(size_of::<tasks::ResourceScope>()).ok()?)?,
        )?
        .checked_add(
            u64::from(limits.tasks.authority_grants)
                .checked_mul(2)?
                .checked_mul(u64::from(limits.tasks.authority_segments))?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?
        .checked_add(u64::from(limits.tasks.authority_bytes))?
        .checked_add(
            u64::from(limits.tasks.executor_kinds)
                .checked_mul(u64::try_from(size_of::<tasks::AuthorityExecutor>()).ok()?)?,
        )
}

pub(crate) fn run_policy_bound(policy: &RunPolicy, limits: &Limits) -> Option<u64> {
    run_policy_bytes(policy)?.checked_add(run_carriers_bound(limits)?)?.checked_add(
        u64::from(tasks::MODELS).checked_mul(
            u64::try_from(size_of::<Option<u32>>())
                .ok()?
                .checked_add(u64::try_from(size_of::<jig_domain_authority::AccountBinding>()).ok()?)?,
        )?,
    )
}

pub(crate) fn run_charter_bytes(charter: &RunCharter) -> Option<u64> {
    let mut bytes = run_policy_bytes(&charter.policy)?
        .checked_add(
            u64::try_from(charter.sources.len()).ok()?.checked_mul(u64::try_from(size_of::<Option<u32>>()).ok()?)?,
        )?
        .checked_add(
            u64::try_from(charter.accounts.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<jig_domain_authority::AccountBinding>()).ok()?)?,
        )?;
    match &charter.contract {
        tasks::Contract::Report { .. } | tasks::Contract::Change { .. } => {}
        tasks::Contract::Approval { state, .. } => bytes = bytes.checked_add(u64::try_from(state.len()).ok()?)?,
        tasks::Contract::Verdict { choices } => {
            bytes = bytes.checked_add(
                u64::try_from(choices.len()).ok()?.checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?,
            )?;
        }
    }
    bytes = bytes
        .checked_add(
            u64::try_from(charter.authority.grants.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?)?,
        )?
        .checked_add(
            u64::try_from(charter.authority.note_resources.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<tasks::ResourceScope>()).ok()?)?,
        )?
        .checked_add(
            u64::try_from(charter.authority.delegation.kinds.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<tasks::AuthorityExecutor>()).ok()?)?,
        )?;
    for grant in &charter.authority.grants {
        bytes = bytes.checked_add(
            u64::try_from(grant.pattern.segments.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?;
        for segment in &grant.pattern.segments {
            bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
        }
        let terminal = match &grant.pattern.last {
            tasks::Last::Exact(bytes) | tasks::Last::Open(bytes) => bytes.len(),
        };
        bytes = bytes.checked_add(u64::try_from(terminal).ok()?)?;
    }
    for scope in &charter.authority.note_resources {
        bytes = bytes.checked_add(
            u64::try_from(scope.pattern.segments.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?;
        for segment in &scope.pattern.segments {
            bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
        }
        let terminal = match &scope.pattern.last {
            tasks::Last::Exact(bytes) | tasks::Last::Open(bytes) => bytes.len(),
        };
        bytes = bytes.checked_add(u64::try_from(terminal).ok()?)?;
    }
    Some(bytes)
}

pub(crate) fn row_bound(limits: &Limits) -> Option<u64> {
    let tasks = limits.tasks;
    let mut bytes = u64::try_from(size_of::<tasks::TaskRecord>()).ok()?;
    for retained in [
        u64::from(tasks.spec_bytes),
        u64::from(tasks.result_bytes).checked_mul(3)?,
        u64::from(tasks.inbox_bytes),
        u64::from(tasks.inbox_messages).checked_mul(u64::try_from(size_of::<tasks::Word>()).ok()?)?,
        // Saved names own their path segments and bytes, including the forge/repository pair.
        u64::from(tasks.saved_resources).checked_mul(
            u64::try_from(size_of::<tasks::SavedResource>())
                .ok()?
                .checked_add(
                    u64::from(tasks.authority_segments).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
                )?
                .checked_add(u64::from(tasks.authority_bytes))?,
        )?,
        u64::from(tasks.parameters).checked_mul(u64::try_from(size_of::<tasks::Parameter>()).ok()?)?,
        u64::from(tasks.inputs)
            .checked_add(u64::from(tasks.dependencies).checked_mul(2)?)?
            .checked_add(u64::from(tasks.delegates))?
            .checked_mul(8)?,
        u64::from(tasks.contract_choices).checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?,
        u64::from(tasks.authority_grants).checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?.checked_add(
            u64::from(tasks.authority_segments).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?)?,
        u64::from(tasks.authority_grants).checked_mul(
            u64::try_from(size_of::<tasks::ResourceScope>()).ok()?.checked_add(
                u64::from(tasks.authority_segments).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
            )?,
        )?,
        u64::from(tasks.authority_bytes),
        u64::from(tasks.executor_kinds).checked_mul(u64::try_from(size_of::<tasks::AuthorityExecutor>()).ok()?)?,
    ] {
        bytes = bytes.checked_add(retained)?;
    }
    let people = limits.people;
    Some(
        bytes
            .max(u64::from(people.identity_bytes))
            .max(u64::from(people.words))
            .max(u64::from(limits.forge.client.answer_bytes))
            .max(u64::from(people.holdings).checked_mul(u64::try_from(size_of::<people::Holding>()).ok()?)?),
    )
}
