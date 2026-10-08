//! The admitted step and ordered journal release (jig's domain/root.md, 3).
use crate::boundary::{
    Assignment, BriefConnector, BriefSection, Event, Payload, PreparedWorkspace, Read, Released, Request, ResultPage,
    Work,
};
use crate::limits::{
    Limits, authority_within, core_limits, environment_core, payload_slots, root_journal_limits, route_bound,
    route_decision, route_takes, run_policy_bound, worst_case,
};
use crate::restart::{connector_restarting, request_load};
use crate::route::{
    LandingRule, account_fire, escalation, forge_route, host_route, inbox, landing, policy_translate, results,
    route_core_requests, route_into, step_routed,
};
use crate::translate::{Config, RootConfig, environment_forge, split_config, view_into, view_outputs, view_requests};
use crate::{CallKey, Decision, Delivery, Journal, JournalOutput as Output, Range, Write, loads};
use alloc::boxed::Box;
use jig_core::{Core, PendingRelay};
use jig_core_authority as authority;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{Env, Id, List, Map, Queue, ReplyTo, Slab, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_change as forge_change;
use temper_engine_domain_forge_client as forge_client;

/// Core, forge, journal and bounded child-requested continuation bindings.
/// The core owns policy and current proofs; the forge owns connector payloads
/// and timer decisions. Every outbound envelope passes through the journal
/// (jig's domain/root.md, sections 2–4).
#[derive(Debug)]
pub struct Domain {
    pub(crate) limits: Limits,
    pub(crate) config: RootConfig,
    pub(crate) core: Core,
    pub(crate) journal: Journal,
    pub(crate) stop_pending: bool,
    pub(crate) door_pass: bool,
    pub(crate) door_count: u32,
    pub(crate) startup_range: Option<Range>,
    pub(crate) core_header_loaded: bool,
    pub(crate) restored_commits: Option<u64>,
    pub(crate) brief_connectors: Slab<BriefConnector>,
    pub(crate) brief_sections: Map<u64, Box<[BriefSection]>>,
    pub(crate) run_workspaces: Map<u64, PreparedWorkspace>,
    pub(crate) forge: forge::Domain,
    pub(crate) adoption_restore: Map<Token, Option<forge::Repository>>,
    pub(crate) forge_subscribing: Map<Token, forge::Subscriber>,
    pub(crate) forge_unsubscribing: Map<Token, (u64, forge::Topic)>,
    pub(crate) forge_reading: Map<Token, (ReplyTo, CallKey)>,
    pub(crate) forge_left: Map<(u64, u64), Box<[forge::Pushed]>>,
    pub(crate) forge_effecting: Map<u64, (ReplyTo, CallKey)>,
    pub(crate) forge_effects: Map<Token, forge_route::EffectFlight>,
    pub(crate) next_effect_owner: u64,
    pub(crate) forge_delegating: Map<(u64, u64), forge_change::Delegate>,
    pub(crate) loads: loads::Loads,
    pub(crate) assignments: Map<u64, Assignment>,
    pub(crate) payloads: Slab<Option<Payload>>,
    pub(crate) result_reads: Slab<Option<Read>>,
    pub(crate) result_pages: Queue<ResultPage>,
    pub(crate) host_calls: Map<Token, host_route::Flight>,
    pub(crate) work: Queue<Work>,
}

impl Domain {
    /// Allocate the root and fixed child room from shell-supplied configuration and validated
    /// cross-child limits. Checks bounded authority/bootstrap configuration; issues no request.
    /// Startup pages/account setup begin only on `Event::Restart`.
    #[must_use]
    #[expect(clippy::too_many_lines, reason = "the root allocates every child and bounded handoff table together")]
    pub fn new(mut config: Config, limits: &Limits) -> Domain {
        assert!(worst_case(limits).is_some(), "root limits are valid");
        assert!(config.deployment_provider != 0, "deployment service provider differs from forge sign-in provider 0");
        assert!(
            match run_policy_bound(&config.run, limits) {
                Some(bytes) => bytes <= u64::from(limits.journal.run_bytes),
                None => false,
            },
            "run policy fits assignment bound"
        );
        assert!(
            config.resume_bytes > 0 && config.resume_bytes <= limits.journal.transcript_bytes,
            "configured task transcript bound fits the root's owned-byte limit"
        );
        assert!(*config.authority.limits() == limits.authority, "root prices its exact authority limits");
        assert!(
            limits.forge.judge_projects >= limits.authority.projects
                && limits.forge.judge_criteria >= limits.authority.requirements,
            "forge judge table covers authority policy bounds"
        );
        assert!(
            config.landing.projects.capacity() <= limits.authority.projects,
            "typed landing project table fits the policy bound"
        );

        assert!(
            landing_within(&config.landing.deployment, limits.journal.transcript_bytes),
            "deployment landing policy fits owned-byte bound"
        );
        for (_, rules) in &config.landing.projects {
            assert!(
                landing_within(rules, limits.journal.transcript_bytes),
                "project landing policy fits owned-byte bound"
            );
        }
        assert!(config.permission_roles.capacity() == limits.authority.projects, "permission policy project bound");
        for (_, mappings) in &config.permission_roles {
            let count = u64::try_from(mappings.len()).expect("usize fits u64");
            let unit = u64::try_from(size_of::<people::PermissionRole>()).expect("type size fits u64");
            assert!(
                match count.checked_mul(unit) {
                    Some(bytes) => bytes <= u64::from(limits.journal.transcript_bytes),
                    None => false,
                },
                "permission policy bound"
            );
        }
        assert!(authority_within(&config.chat_authority, limits), "chat authority shape bounded before copying");
        let mut projects = List::with_capacity(limits.people.projects);
        for owner in &config.owners {
            assert!(config.authority.policy(owner.project).is_some(), "bootstrap owner names configured policy");
            assert!(
                match config.authority.policy(owner.project).expect("known policy").escalation_role {
                    Some(role) => role <= 3,
                    None => false,
                },
                "current root requires final escalation role"
            );
            let mut found = false;
            for project in &projects {
                if *project == owner.project {
                    found = true;
                }
            }
            if !found {
                projects.push(owner.project).expect("configured projects bounded");
            }
        }
        let mut forge_config = core::mem::replace(
            &mut config.forge,
            forge_client::Config { namespace: Box::new([]), writers: Box::new([]) },
        );
        forge_config.namespace = Box::from(config.deployment);
        let mut forge =
            forge::Domain::new(&limits.forge, config.seed, forge_config).expect("valid forge configuration");
        let built = policy_translate::build_landing(
            &config.landing.deployment,
            false,
            limits.authority.requirements,
            config.forge_connector,
        )
        .expect("deployment landing requirements bounded");
        assert!(
            config.authority.add_configured_requirements(&built.requirements),
            "deployment landing requirements fit authority configuration"
        );
        assert!(forge.deployment_judges(built.criteria), "deployment judge table bounded");
        for (&project, rules) in &config.landing.projects {
            let built =
                policy_translate::build_landing(rules, true, limits.authority.requirements, config.forge_connector)
                    .expect("project landing requirements bounded");
            let mut policy = config.authority.policy(project).expect("landing project has a policy").clone();
            assert!(
                policy_translate::landing_roles(&policy, &config.landing.deployment),
                "deployment landing roles configured"
            );
            assert!(policy_translate::landing_roles(&policy, rules), "project landing roles configured");
            let combined = policy_translate::combine_requirements(
                &policy.requirements,
                &built.requirements,
                limits.authority.requirements,
            )
            .expect("project landing requirements fit authority policy");
            policy.requirements = combined;
            let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
            authority::step(&mut config.authority, authority::Event::Policy { project, policy }, &mut facts);
            assert_eq!(facts.pop(), Some(authority::PolicyFact::Changed { project }), "landing policy configured");
            assert!(forge.project_judges(project, built.criteria), "project judge table bounded");
        }
        let gate_policy = policy_translate::gate_policy(
            &config.landing,
            limits.authority.requirements,
            limits.forge.change_policy.gates,
        )
        .expect("landing gate templates bounded");
        assert!(forge.gate_policy(gate_policy, &limits.forge), "forge owns bounded landing templates");
        let (core_config, root_config) = split_config(config, projects);
        let mut core = Core::new(core_config, &core_limits(limits));
        let core_env = Env { now: skein_lib::Time::ZERO, wall: skein_lib::Wall::EPOCH, limits: core_limits(limits) };
        let configured = jig_core::step(
            &mut core,
            &core_env,
            jig_core::Event::Tasks(tasks::Event::Kinds {
                connector: root_config.forge_connector,
                kinds: Box::new([tasks::Kind {
                    connector: root_config.forge_connector,
                    kind: 1,
                    hold: match forge::resources::BRANCH_HOLD {
                        forge::resources::HoldKind::Exclusive { wait } => tasks::HoldKind::Exclusive {
                            taken: if wait { tasks::Taken::Waits } else { tasks::Taken::Refuses },
                        },
                        forge::resources::HoldKind::Shared => unreachable!("private branch kind takes exclusive holds"),
                    },
                }]),
            }),
        );
        match configured {
            jig_core::Requests::Out(mut output) => {
                match output.pop() {
                    Some(jig_core::Request::Decided) => {}
                    Some(
                        jig_core::Request::Write(_)
                        | jig_core::Request::Ask { .. }
                        | jig_core::Request::Held(_)
                        | jig_core::Request::Now(_),
                    )
                    | None => unreachable!("configuration decides nothing"),
                }
                assert!(output.is_empty(), "configuration emits no request");
            }
        }
        Domain {
            journal: Journal::new(&root_journal_limits(limits)),
            stop_pending: false,
            door_pass: false,
            door_count: 0,
            startup_range: None,
            core_header_loaded: false,
            restored_commits: None,
            core,
            brief_connectors: Slab::with_capacity(
                limits
                    .brief
                    .briefs
                    .checked_mul(limits.brief.sections)
                    .expect("validated brief count")
                    .checked_mul(2)
                    .expect("validated brief connector room"),
            ),
            brief_sections: Map::with_capacity(limits.tasks.tasks),
            run_workspaces: Map::with_capacity(limits.tasks.tasks),
            forge,
            adoption_restore: Map::with_capacity(limits.forge.adoptions),
            forge_subscribing: Map::with_capacity(limits.fleet.calls),
            forge_unsubscribing: Map::with_capacity(limits.fleet.calls),
            forge_reading: Map::with_capacity(limits.fleet.calls),
            forge_left: Map::with_capacity(limits.tasks.tasks),
            forge_effecting: Map::with_capacity(limits.fleet.calls),
            forge_effects: Map::with_capacity(
                limits.call_records.checked_add(limits.tasks.tasks).expect("effect handoff capacity"),
            ),
            next_effect_owner: 1,
            forge_delegating: Map::with_capacity(limits.forge.changes),
            loads: loads::Loads::new(&limits.loads),
            assignments: Map::with_capacity(limits.tasks.tasks),
            payloads: Slab::with_capacity(payload_slots(limits).expect("valid payload room")),
            result_reads: Slab::with_capacity(limits.loads.loads),
            result_pages: Queue::with_capacity(limits.loads.loads),
            host_calls: Map::with_capacity(limits.fleet.calls),
            work: Queue::with_capacity(route_bound(limits).expect("valid routes")),
            config: root_config,
            limits: *limits,
        }
    }

    /// Identify the core's refused restart step, including a live range that
    /// no longer fits the configured limits.
    #[must_use]
    pub fn restart_failure(&self) -> Option<jig_core::RestartStep> {
        self.core.restart_failure()
    }

    /// Decisions open only after the core's restart script completes.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.core.restart_ready() && !self.journal.stopped()
    }

    /// Pure shell idle query: no immediate internal handoff or issued store operation
    /// remains unfinished. Call after the iteration's reclaim; sessions, idle
    /// workers, assigned workers awaiting external answers and future task/account
    /// timers are permitted. Story completion also requires the shell's independent
    /// final result condition. This query never cancels work.
    #[must_use]
    pub fn quiescent(&self) -> bool {
        self.ready()
            && self.core.quiescent(self.journal.idle())
            && self.loads.quiescent()
            && self.work.is_empty()
            && self.assignments.is_empty()
            && self.payloads.is_empty()
            && self.result_reads.is_empty()
            && self.result_pages.is_empty()
            && self.adoption_restore.is_empty()
            && self.forge_subscribing.is_empty()
            && self.forge_unsubscribing.is_empty()
            && self.forge_reading.is_empty()
            && self.forge.briefs_idle()
            && self.host_calls.is_empty()
            && self.forge_effecting.is_empty()
            && self.forge_effects.is_empty()
            && self.brief_connectors.is_empty()
            && !self.forge.is_ready()
    }

    /// Pure snapshot query for the latest allocated deployment counters; these may run ahead of
    /// store durability. Exposes neither children nor owned handoff bodies and emits no effect.
    #[must_use]
    pub fn deployment(&self) -> crate::Deployment {
        self.core.counters.deployment()
    }

    /// Retired IO/body/child slots are reclaimed at iteration end, after every event and ready
    /// pass. Bounded child/slab bookkeeping releases only retired entries; issued loads still
    /// awaiting a terminal remain owned. Emits no request or terminal.
    pub fn reclaim(&mut self) {
        self.core.reclaim();
        self.brief_connectors.reclaim();
        self.forge.reclaim();
        self.payloads.reclaim();
        self.result_reads.reclaim();
        loads::reclaim(&mut self.loads);
    }

    /// Shell/root caller discards every currently queued child observation, scanning at most each
    /// child's configured fact capacity. Emits no effect or terminal and changes no decision state.
    pub fn drain_facts(&mut self) {
        self.core.drain_facts(&core_limits(&self.limits));
    }
}

/// Output room for a commit and one delivery, or one bounded immediate cohort.
/// Reserve before release; step and fire allocate the same bounded scratch room.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    let viewed = views::max_out(&limits.views).saturating_add(4);
    if viewed > 4 { viewed } else { 4 }
}

pub(crate) fn internal(number: u64) -> ReplyTo {
    ReplyTo::new(Token::new(number))
}

pub(crate) fn emit(decision: &mut Decision, limits: &Limits, delivery: Delivery) {
    decision.carry_delivery(&limits.journal, delivery).expect("whole root decision delivery reserved");
}

pub(crate) fn save(decision: &mut Decision, limits: &Limits, write: Write) {
    decision.carry_write(&limits.journal, write).expect("validated bounded root decision record");
}

/// Route one admitted input and all synchronous child callbacks in one decision; store terminals
/// are accepted even under pressure. The shell supplies
/// unchanged configured limits with injected time. Refused web/worker calls get terminal/busy
/// notices before child mutation; admitted effects may wait in the journal until store durability.
/// Account operations keep their own secret-free terminal contract.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event) {
    let mut out = Queue::with_capacity(max_out(&env.limits));
    step_routed(domain, env, event, &mut out);
    stage_now(domain, &mut out);
}

pub(crate) fn admits(domain: &Domain, limits: &Limits) -> bool {
    route_takes(domain, limits)
        && domain.core.counters.deployment().messages
            <= u64::MAX.checked_sub(u64::from(limits.tasks.tasks)).expect("task count fits u64")
        && domain.work.is_empty()
        && domain.journal.takes(&skein_lib::JournalRoom {
            writes: 0,
            held: limits.journal.deliveries.checked_mul(3).expect("root held reserve bounded"),
        })
}

pub(crate) fn close(domain: &mut Domain, env: &Env<Limits>, mut decision: Decision, out: &mut Queue<Request>) {
    if domain.core.restart_failure().is_some() {
        out.push(Request::Stop);
        return;
    }
    let requests = jig_core::finish_decision(&mut domain.core, &environment_core(env));
    route_core_requests(domain, env, &mut decision, requests);
    route_into(domain, env, &mut decision);
    let persist_header =
        domain.core_header_loaded && domain.core.restart_step() != Some(jig_core::RestartStep::LoadCore);
    crate::decision::accept_routed(
        &mut domain.journal,
        &mut domain.core.counters,
        &env.limits.journal,
        decision,
        persist_header,
    )
    .expect("root pressure reserved before child mutation");
    if let Some(commits) = domain.restored_commits.take() {
        assert!(domain.journal.idle(), "deployment is the first cold-start row");
        domain.journal = Journal::from_durable(&root_journal_limits(&env.limits), commits);
    }
    if domain.journal.stopped() {
        out.push(Request::Stop);
    }
}

pub(crate) fn journal_outputs(journal_out: &mut Queue<Output>, out: &mut Queue<Request>) {
    for _ in 0..journal_out.len() {
        match journal_out.pop().expect("journal output count") {
            Output::Now(request) => out.push(request),
            Output::Commit { number, writes } => {
                out.push(Request::Store(crate::StoreRequest::Commit { number, writes }));
            }
            Output::Stop => out.push(Request::Stop),
            Output::Deliver(delivery) => out.push(crate::boundary::delivery_output(delivery)),
        }
    }
}

/// The only admission call for outputs that have no durability dependency.
pub(crate) fn now(domain: &mut Domain, request: Request) -> bool {
    domain.journal.now(request).is_ok()
}

/// Admit immediate requests through the journal's door. The caller takes
/// them on a later release pass; no step returns an outbound queue.
pub(crate) fn stage_now(domain: &mut Domain, out: &mut Queue<Request>) {
    domain.door_pass = true;
    for _ in 0..out.len() {
        let request = out.pop().expect("original output count");
        match request {
            Request::Stop => domain.stop_pending = true,
            request @ (Request::Worker(_)
            | Request::Party(_)
            | Request::Forge { .. }
            | Request::Store(crate::StoreRequest::Load { .. })
            | Request::ToChild(_)
            | Request::Account(_)) => {
                assert!(now(domain, request), "root door reserves one route's immediate outputs");
                domain.door_count = domain.door_count.checked_add(1).expect("bounded immediate route");
            }
            Request::Store(crate::StoreRequest::Commit { .. }) => {
                unreachable!("release takes commits from the journal")
            }
        }
    }
}

/// Drain the journal only; child continuations enter through `Event::Released`
/// and owner progress through `Event::Resume`, each with its own admission.
pub fn release(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    assert!(out.room() >= max_out(&env.limits), "root output room reserved");
    if domain.stop_pending {
        domain.stop_pending = false;
        domain.door_pass = false;
        domain.door_count = 0;
        out.push(Request::Stop);
        return;
    }
    let mut released = Queue::with_capacity(1);
    if domain.door_pass {
        domain.door_pass = false;
        send_commit(domain, out);
        for _ in 0..domain.door_count {
            let _status: skein_lib::Released = domain.journal.release(&mut released);
            out.push(released.pop().expect("one admitted immediate output"));
        }
        domain.door_count = 0;
        return;
    }
    let _status: skein_lib::Released = domain.journal.release(&mut released);
    if let Some(request) = released.pop() {
        out.push(request);
    }
    send_commit(domain, out);
}

pub(crate) fn send_commit(domain: &mut Domain, out: &mut Queue<Request>) {
    match crate::commit(&mut domain.journal, &domain.limits.journal) {
        Some(Output::Commit { number, writes }) => {
            out.push(Request::Store(crate::StoreRequest::Commit { number, writes }));
        }
        Some(Output::Now(_) | Output::Deliver(_) | Output::Stop) => unreachable!("journal commit has writes only"),
        None => {}
    }
}

pub(crate) fn resume_routed(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    assert!(out.room() >= max_out(&env.limits), "root ready output room");
    if domain.journal.stopped() || domain.core.restart_failure().is_some() {
        return;
    }
    if !domain.result_pages.is_empty() && route_takes(domain, &env.limits) {
        let mut decision = route_decision(domain, &env.limits).expect("archive page admitted before owner changes");
        let page = domain.result_pages.pop().expect("pending result page");
        match domain.result_reads.get(Id::from_token(page.waiter)) {
            Some(Some(Read::Inbox(_))) => {
                inbox::page(domain, env, page.waiter, page.rows, page.next, &mut decision, out);
            }
            Some(Some(Read::Result(_))) => {
                results::page(domain, env, page.waiter, page.rows, page.next, &mut decision, out);
            }
            Some(
                Some(
                    Read::Escalation(_)
                    | Read::Proposal(_)
                    | Read::Transcript { .. }
                    | Read::Dependency(_)
                    | Read::InputCheck(_)
                    | Read::Notes { .. },
                )
                | None,
            )
            | None => unreachable!("queued person page has its live read"),
        }
        close(domain, env, decision, out);
        return;
    }
    if !domain.work.is_empty() {
        if route_takes(domain, &env.limits) {
            let decision = route(domain, env);
            close(domain, env, decision, out);
        }
        return;
    }
    if !route_takes(domain, &env.limits) {
        return;
    }
    if domain.ready() || connector_restarting(domain) {
        if domain.forge.is_ready() {
            let mut decision =
                route_decision(domain, &env.limits).expect("journal room checked before connector continuation");
            let mut child = Queue::with_capacity(forge::max_out(&env.limits.forge));
            forge::resume(&mut domain.forge, &environment_forge(env), &mut child);
            forge_route::outputs(domain, env, &mut decision, &mut child);
            route_into(domain, env, &mut decision);
            close(domain, env, decision, out);
            return;
        }
        if !domain.ready() {
            return;
        }
        if domain.core.accounts.usable(domain.core.settings.account) && !domain.core.due.is_empty() {
            for _ in 0..domain.core.due.len() {
                domain.work.push(Work::Activate(domain.core.due.pop().expect("waiting activation")));
            }
            let decision = route(domain, env);
            close(domain, env, decision, out);
            return;
        }
        let mut decision = route_decision(domain, &env.limits).expect("journal room checked before fleet continuation");
        let routed = jig_core::resume_fleet(&mut domain.core, &environment_core(env));
        route_core_requests(domain, env, &mut decision, routed);
        route_into(domain, env, &mut decision);
        close(domain, env, decision, out);
    }
}

/// Fire participating child timers through the same barrier; store durability and inputs run before
/// this pass. The shell supplies injected monotonic/wall time. Account
/// timers may emit bounded protocol actions independently; root routes that mutate tasks/fleet wait
/// for whole-decision admission. Their store and account outcomes enter later through `step`.
pub fn fire(domain: &mut Domain, env: &Env<Limits>) {
    let mut out = Queue::with_capacity(max_out(&env.limits));
    fire_routed(domain, env, &mut out);
    stage_now(domain, &mut out);
}

pub(crate) fn fire_routed(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if domain.journal.stopped() || domain.core.restart_failure().is_some() {
        return;
    }
    if !admits(domain, &env.limits) || (!domain.ready() && !connector_restarting(domain)) {
        let room = skein_lib::JournalRoom { writes: 0, held: 0 };
        let Some(decision) = domain.journal.decision(&room) else { return };
        readonly_timers(domain, env, out);
        domain.journal.accept(decision);
        return;
    }
    let mut decision = route_decision(domain, &env.limits).expect("timer route admitted before child mutation");
    readonly_timers(domain, env, out);
    if connector_restarting(domain) {
        let mut forge_out = Queue::with_capacity(forge::max_out(&env.limits.forge));
        forge::fire(&mut domain.forge, &environment_forge(env), &mut forge_out);
        forge_route::outputs(domain, env, &mut decision, &mut forge_out);
        route_into(domain, env, &mut decision);
        close(domain, env, decision, out);
        return;
    }
    if !domain.ready() {
        return;
    }
    let mut forge_out = Queue::with_capacity(forge::max_out(&env.limits.forge));
    forge::fire(&mut domain.forge, &environment_forge(env), &mut forge_out);
    forge_route::outputs(domain, env, &mut decision, &mut forge_out);
    route_into(domain, env, &mut decision);
    let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::EffectDeadline);
    route_core_requests(domain, env, &mut decision, routed);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Tasks);
    route_core_requests(domain, env, &mut decision, routed);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Fleet);
    route_core_requests(domain, env, &mut decision, routed);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Brief);
    route_core_requests(domain, env, &mut decision, routed);
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

fn readonly_timers(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    account_fire(domain, env, out);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::View);
    let mut view_out = view_requests(routed, views::max_out(&env.limits.views));
    view_outputs(domain, env, &mut view_out, out);
}

pub(crate) fn route(domain: &mut Domain, env: &Env<Limits>) -> Decision {
    let mut decision = route_decision(domain, &env.limits).expect("journal room checked before routing children");
    route_into(domain, env, &mut decision);
    decision
}

pub(crate) fn forge_release_ending(ending: tasks::Ending) -> forge::ReleaseEnding {
    match ending {
        tasks::Ending::Done(_) => forge::ReleaseEnding::Done,
        tasks::Ending::Failed { .. } => forge::ReleaseEnding::Failed,
        tasks::Ending::Cancelled { .. } => forge::ReleaseEnding::Cancelled,
    }
}
/// A released continuation is one newly admitted step, just like an input.
/// When the route is full the journal returns it to iterate without changing a child.
pub(crate) fn step_released(domain: &mut Domain, env: &Env<Limits>, released: Released, out: &mut Queue<Request>) {
    let Some(mut decision) = route_decision(domain, &env.limits) else {
        out.push(Request::ToChild(released));
        return;
    };
    released_into(domain, env, released, out);
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

#[expect(clippy::too_many_lines, reason = "exhaustive released continuation routing")]
fn released_into(domain: &mut Domain, env: &Env<Limits>, released: Released, out: &mut Queue<Request>) {
    match released.0 {
        Delivery::ForgeCommitted { entry } => {
            domain.work.push(Work::Forge(forge::Event::Committed { entry }));
        }
        Delivery::Fleet(event) => domain.work.push(Work::Fleet(event)),
        Delivery::View(event) => {
            view_into(domain, env, *event, out);
        }
        Delivery::Relay { task, attempt, previous, word } => {
            let name = Token::new(word.number);
            let payload = domain.payloads.insert(Some(Payload::InboxWord(word))).expect("relay payload reserved");
            assert!(
                domain.core.relaying.replace(PendingRelay { previous, message: name.raw() }).is_none(),
                "one relay at a time"
            );
            domain.work.push(Work::Fleet(fleet::Event::Inbound {
                run: Token::new(task),
                attempt: Token::new(attempt),
                message: fleet::Message { name, sender: payload.token(), words: payload.token() },
            }));
        }
        Delivery::Load { waiter, range, after } => {
            request_load(domain, waiter, range, after, out);
        }
        Delivery::ReadEscalationDecision { waiter } => {
            let Some(Some(read)) = domain.result_reads.get(Id::from_token(waiter)) else { return };
            let (task, revision) = match read {
                Read::Escalation(escalation::Query::Historical { task, revision, .. }) => (*task, *revision),
                Read::Result(_)
                | Read::Inbox(_)
                | Read::Transcript { .. }
                | Read::Dependency(_)
                | Read::InputCheck(_)
                | Read::Notes { .. }
                | Read::Escalation(escalation::Query::Read { .. })
                | Read::Proposal(_) => {
                    unreachable!("decision archive waiter")
                }
            };
            request_load(
                domain,
                waiter,
                Range::Core(crate::CoreRange::EscalationDecision { task, revision }),
                None,
                out,
            );
        }
        Delivery::ReadResult { waiter } => {
            let Some(read) = domain.result_reads.get(Id::from_token(waiter)) else {
                return;
            };
            let Some(read) = read else {
                return;
            };
            match read {
                Read::Result(_) => request_load(domain, waiter, Range::Core(crate::CoreRange::EndedResults), None, out),
                Read::Inbox(_)
                | Read::Escalation(_)
                | Read::Proposal(_)
                | Read::Transcript { .. }
                | Read::Dependency(_)
                | Read::InputCheck(_)
                | Read::Notes { .. } => {
                    unreachable!("result waiter")
                }
            }
        }
        Delivery::BeginInboxView { waiter } => {
            match domain.result_reads.get(Id::from_token(waiter)) {
                Some(Some(Read::Inbox(_))) => {}
                Some(
                    Some(
                        Read::Result(_)
                        | Read::Escalation(_)
                        | Read::Proposal(_)
                        | Read::Transcript { .. }
                        | Read::Dependency(_)
                        | Read::InputCheck(_)
                        | Read::Notes { .. },
                    )
                    | None,
                )
                | None => unreachable!("inbox start has its live waiter"),
            }
            request_load(domain, waiter, Range::Core(crate::CoreRange::Tasks), None, out);
        }
        delivery @ (Delivery::Host(_)
        | Delivery::ForgeCall { .. }
        | Delivery::Procedure { .. }
        | Delivery::CallAnswer { .. }
        | Delivery::Inbound { .. }
        | Delivery::InboxPage { .. }
        | Delivery::InboxView { .. }
        | Delivery::EscalationReply { .. }
        | Delivery::Reply { .. }
        | Delivery::AcknowledgeTurn { .. }
        | Delivery::Acknowledge { .. }
        | Delivery::Cancel { .. }
        | Delivery::Assigned { .. }
        | Delivery::WebReply { .. }
        | Delivery::Refuse { .. }
        | Delivery::TurnBusy { .. }
        | Delivery::ResultReply { .. }
        | Delivery::Result { .. }) => out.push(crate::boundary::delivery_output(delivery)),
    }
}

fn landing_within(rules: &[LandingRule], bytes: u32) -> bool {
    match landing::landing_rules_bytes(rules) {
        Some(held) => held <= u64::from(bytes),
        None => false,
    }
}
