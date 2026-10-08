//! Top-level forge routing and commit fence (domain/connectors.md, sections
//! 2–5). Durable rows are held here and mirrored by root store writes.
use alloc::boxed::Box;
use skein_lib::{Env, List, Map, Queue, Token};
use temper_engine_domain_forge_change as change;
use temper_engine_domain_forge_client as client;
use temper_engine_domain_forge_issues as issues;

use crate::{
    Adopted, Adoption, BranchHead, ChangeRow, CiState, Class, Event, Hold, IssueRow, Key, Kinds, Limits, Name, News,
    Protection, PullState, ReleaseEnding, ReleaseRow, Repository, Request, RestartStage, Role, Stored, Subscriber,
    Topic, What, Writer,
};
use crate::{brief, held};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AdoptionStage {
    Permission,
    Branches,
    Settings,
    Protection,
    Collaborators(u32),
}

/// One admission read chain; only rows proven from current provider facts
/// can become durable adopted repositories.
#[derive(Debug)]
pub(crate) struct PendingAdoption {
    request: Adoption,
    stage: AdoptionStage,
    permission: Option<client::api::Permission>,
    settings: Option<client::api::Settings>,
    protection: Protection,
    collaborators: List<client::api::Collaborator>,
}

/// One comparison in flight for a branch's observed tip move.
#[derive(Debug)]
pub(crate) struct PendingLanding {
    topic: Topic,
    before: client::api::Commit,
    after: client::api::Commit,
}
#[derive(Debug)]
pub(crate) struct PendingLost {
    task: u64,
    attempt: u64,
    name: Name,
}
/// One fresh verdict read for a commit with live CI subscribers.
#[derive(Debug)]
pub(crate) struct PendingCi {
    repository: client::api::Repository,
    head: client::api::Commit,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StepStage {
    Branch,
    Base,
    Pull,
    Ci,
    BaseCi,
    Compare,
    Reviews(u32),
}

/// One sequence of fresh reads before a level-triggered change step.
#[derive(Debug)]
pub(crate) struct PendingStep {
    task: u64,
    entry: u64,
    stage: StepStage,
    heard: change::Heard,
    gates: Box<[change::GateReport]>,
    queue_repair_active: bool,
    branch: Option<client::api::Commit>,
    base_tip: Option<client::api::Commit>,
    pull: Option<client::api::Pull>,
    ci: change::Status,
    base_ci: change::Status,
    contains_base: change::Status,
    reviews: Box<[client::api::Review]>,
    reviews_complete: bool,
}
struct StepInput {
    task: u64,
    entry: u64,
    heard: change::Heard,
    gates: Box<[change::GateReport]>,
    queue_repair_active: bool,
}

/// The bounded forge connector subtree.
#[derive(Debug)]
pub struct Domain {
    repositories: Map<client::api::Repository, Repository>,
    names: Map<u64, Box<[Name]>>,
    holds: Map<Name, Hold>,
    subscriptions: Map<(u64, Topic), Subscriber>,
    heads: Map<Name, BranchHead>,
    pulls: Map<Name, PullState>,
    ci: Map<(client::api::Repository, client::api::Commit), CiState>,
    entries: Map<u64, client::Entry>,
    landed: Map<client::api::Commit, u64>,
    landings: Map<Token, PendingLanding>,
    lost: Map<Token, PendingLost>,
    pending_ci: Map<Token, PendingCi>,
    sequence: u64,
    changes: Map<u64, ChangeRow>,
    steps: Map<Token, PendingStep>,
    issues: Map<u64, IssueRow>,
    releases: Map<u64, ReleaseRow>,
    namespace: Box<[u8]>,
    adoptions: Map<Token, PendingAdoption>,
    writers: Map<u16, u64>,
    client: client::Domain,
    client_out: Queue<client::Request>,
    pub(crate) brief_fetches: Map<Token, brief::BriefFetch>,
    brief_planned: Map<Token, crate::BriefSource>,
    pub(crate) brief_pending: Map<Token, held::Pending>,
    pub(crate) brief_held: Map<Token, held::Held>,
    judges: crate::Judges,
    judge_criteria: u32,
}

impl Domain {
    /// Bind agent and projection keys to the durable deployment ID before
    /// connector records are restored.
    pub fn bind_deployment(&mut self, id: [u8; 16], limits: &Limits) -> bool {
        if !self.client.bind_deployment(id, &limits.client) {
            return false;
        }
        self.namespace = Box::from(id);
        true
    }
    /// Construct a connector with a configured client writer and deployment
    /// namespace; invalid limits or writer configuration are refused.
    pub fn new(l: &Limits, seed: u64, config: client::Config) -> Result<Domain, client::api::Error> {
        if crate::worst_case(l).is_none() {
            return Err(client::api::Error::TooLarge);
        }
        let mut writers = Map::with_capacity(l.repositories);
        for writer in &config.writers {
            if writers.insert(writer.forge, writer.author).is_err() {
                return Err(client::api::Error::TooLarge);
            }
        }
        Ok(Domain {
            repositories: Map::with_capacity(l.repositories),
            names: Map::with_capacity(l.tasks),
            holds: Map::with_capacity(l.holds),
            subscriptions: Map::with_capacity(l.subscriptions),
            heads: Map::with_capacity(l.client.resources),
            pulls: Map::with_capacity(l.client.resources),
            ci: Map::with_capacity(l.subscriptions),
            entries: Map::with_capacity(l.entries),
            landed: Map::with_capacity(l.landings),
            landings: Map::with_capacity(l.landings),
            lost: Map::with_capacity(l.holds),
            pending_ci: Map::with_capacity(l.subscriptions),
            sequence: 0,
            changes: Map::with_capacity(l.changes),
            steps: Map::with_capacity(l.changes),
            issues: Map::with_capacity(l.issues),
            releases: Map::with_capacity(l.tasks),
            namespace: config.namespace.clone(),
            adoptions: Map::with_capacity(l.adoptions),
            writers,
            client: client::Domain::configured(&l.client, seed, config)?,
            client_out: Queue::with_capacity(client::max_out(&l.client)),
            brief_fetches: Map::with_capacity(l.brief_sections),
            brief_planned: Map::with_capacity(l.brief_sections),
            brief_pending: Map::with_capacity(l.brief_sections),
            brief_held: Map::with_capacity(l.brief_sections),
            judges: crate::Judges::empty(l.judge_projects),
            judge_criteria: l.judge_criteria,
        })
    }

    /// Install deployment criteria before decisions begin.
    pub fn deployment_judges(&mut self, criteria: Box<[crate::Criterion]>) -> bool {
        if !self.judge_count_fits(criteria.len()) {
            return false;
        }
        self.judges.deployment = criteria;
        true
    }

    /// Replace one project's judge parameters with its committed policy.
    pub fn project_judges(&mut self, project: u32, criteria: Box<[crate::Criterion]>) -> bool {
        self.judge_count_fits(criteria.len()) && self.judges.projects.insert(project, criteria).is_ok()
    }

    fn judge_count_fits(&self, count: usize) -> bool {
        match u32::try_from(count) {
            Ok(count) => count <= self.judge_criteria,
            Err(_) => false,
        }
    }

    /// Whether the landing effect's exact-head condition guards this judge.
    #[must_use]
    #[expect(clippy::match_like_matches_macro, reason = "the foundation forbids matches in domain steps")]
    pub fn guards_landing(&self, project: u32, requirement: u16, parameters: u32) -> bool {
        match (requirement, crate::judge::criterion(&self.judges, project, parameters)) {
            (1, Some(crate::Criterion::Ci))
            | (3, Some(crate::Criterion::Gate { .. }))
            | (4, Some(crate::Criterion::Approval { .. })) => true,
            _ => false,
        }
    }

    /// Judge one forge requirement for the exact head of a change effect.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "a judgement names its project, criterion and coherent change evidence"
    )]
    pub fn judge_landing(
        &self,
        project: u32,
        requirement: u16,
        parameters: u32,
        task: u64,
        head: client::api::Commit,
        evidence: &crate::ChangeEvidence,
        reviewers: &[crate::Reviewer],
    ) -> Option<crate::JudgeVerdict> {
        let criterion = crate::judge::criterion(&self.judges, project, parameters)?;
        let matching = match criterion {
            crate::Criterion::Ci => requirement == 1,
            crate::Criterion::UpToDate => requirement == 2,
            crate::Criterion::Gate { .. } => requirement == 3,
            crate::Criterion::Approval { .. } => requirement == 4,
        };
        if !matching {
            return None;
        }
        let row = self.changes.get(&task)?;
        let repository = self.repositories.get(&row.repository)?;
        Some(crate::judge::judge(criterion, repository, head, &row.change.clean, evidence, reviewers))
    }

    /// The next child timer, if one is armed.
    #[must_use]
    pub fn next_deadline(&self) -> Option<skein_lib::Time> {
        self.client.next_deadline()
    }

    /// Whether a child call or a newly committed entry may be sent.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.client.is_ready()
    }

    /// Whether no brief section is being gathered by the connector.
    #[must_use]
    pub fn briefs_idle(&self) -> bool {
        self.brief_fetches.is_empty()
            && self.brief_planned.is_empty()
            && self.brief_pending.is_empty()
            && self.brief_held.is_empty()
    }

    /// Transfer one completed section to the parent assembling an assignment.
    pub fn take_brief(&mut self, section: Token) -> Option<Box<[u8]>> {
        match self.brief_held.remove(&section) {
            Some(held) => Some(held.words),
            None => None,
        }
    }

    /// Reclaim transient child buffers after the current decision.
    pub fn reclaim(&mut self) {
        self.client.reclaim();
    }

    /// Check one typed task interest before the root adds its matching durable task row.
    #[must_use]
    pub fn can_subscribe(&self, limits: &Limits, subscription: &Subscriber) -> bool {
        if subscription.number == 0
            || subscription.paths.len() > usize::try_from(limits.paths_per_subscription).expect("u32 fits usize")
        {
            return false;
        }
        match self.subscriptions.get(&(subscription.task, subscription.topic.clone())) {
            Some(existing) if existing.number != subscription.number => return false,
            None if self.subscriptions.len() == limits.subscriptions => return false,
            Some(_) | None => {}
        }
        for path in &subscription.paths {
            if path.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize") {
                return false;
            }
        }
        let repository = match &subscription.topic {
            Topic::Landings { repository, branch } => {
                if branch.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize") {
                    return false;
                }
                *repository
            }
            Topic::Ci { repository, .. } | Topic::Pull { repository, .. } | Topic::Participation { repository, .. } => {
                *repository
            }
        };
        self.repositories.contains_key(&repository)
    }

    /// Find the connector half of one durable task subscription.
    #[must_use]
    pub fn subscription(&self, task: u64, number: u64) -> Option<Topic> {
        for (_, row) in &self.subscriptions {
            if row.task == task && row.number == number {
                return Some(row.topic.clone());
            }
        }
        None
    }

    /// Current live resource names of one task, for root-side name additions.
    #[must_use]
    pub fn names(&self, task: u64) -> Option<&[Name]> {
        match self.names.get(&task) {
            Some(names) => Some(names),
            None => None,
        }
    }

    /// Current holder and writer of one named resource for root claim checks.
    #[must_use]
    pub fn hold(&self, name: &Name) -> Option<&Hold> {
        self.holds.get(name)
    }

    /// The last durable head observed for a named branch.
    #[must_use]
    #[expect(clippy::manual_map, reason = "the strict subset uses a match without a closure")]
    pub fn branch_head(&self, name: &Name) -> Option<client::api::Commit> {
        match self.heads.get(name) {
            Some(row) => Some(row.commit),
            None => None,
        }
    }

    /// Adopted repository facts used by root authority and workspace translation.
    #[must_use]
    pub fn repository(&self, provider: client::api::Repository) -> Option<&Repository> {
        self.repositories.get(&provider)
    }

    /// Resolve a deployment-wide saved repository tag within its project.
    #[must_use]
    pub fn repository_tag(&self, project: u32, tag: u32) -> Option<&Repository> {
        let mut found = None;
        for (_, repository) in &self.repositories {
            if repository.project == project && repository.provider.repository == tag {
                if found.is_some() {
                    return None;
                }
                found = Some(repository);
            }
        }
        found
    }

    /// This project's explicitly selected home repository, when adopted.
    #[must_use]
    #[expect(clippy::manual_find, reason = "the strict subset uses explicit iteration instead of iterator closures")]
    pub fn home(&self, project: u32) -> Option<&Repository> {
        for (_, repository) in &self.repositories {
            if repository.project == project && repository.home {
                return Some(repository);
            }
        }
        None
    }

    /// Current durable change state used by the root's procedure route.
    #[must_use]
    pub fn change(&self, task: u64) -> Option<&ChangeRow> {
        self.changes.get(&task)
    }

    /// A change's committed pull identity for bounded root brief sections.
    #[must_use]
    #[expect(clippy::manual_find, reason = "the strict subset uses explicit iteration instead of iterator closures")]
    pub fn change_for_pull(&self, repository: u32, number: u64) -> Option<&ChangeRow> {
        for (_, row) in &self.changes {
            if row.repository.repository == repository && row.pull == Some(number) {
                return Some(row);
            }
        }
        None
    }

    /// Repair owner and task for one landing queue, retained through restart.
    #[must_use]
    pub fn queue_repair(&self, repository: client::api::Repository, base: &[u8]) -> Option<(u64, u64)> {
        let mut newest = None;
        for (_, row) in &self.changes {
            if row.repository == repository
                && row.base.as_ref() == base
                && let Some(repair) = row.queue_repair
                && match newest {
                    Some((_, prior)) => repair > prior,
                    None => true,
                }
            {
                newest = Some((row.task, repair));
            }
        }
        newest
    }

    #[must_use]
    pub fn queue_repairs(&self, repository: client::api::Repository, base: &[u8]) -> u32 {
        let mut count = 0_u32;
        for (_, row) in &self.changes {
            if row.repository == repository && row.base.as_ref() == base {
                count = count.saturating_add(row.queue_repairs);
            }
        }
        count
    }

    #[must_use]
    pub fn queue_repair_owner(&self, repair: u64) -> Option<u64> {
        for (_, row) in &self.changes {
            if row.queue_repair == Some(repair) {
                return Some(row.task);
            }
        }
        None
    }

    /// Committed goal projection for root subscription and retry routing.
    #[must_use]
    pub fn issue(&self, goal: u64) -> Option<&IssueRow> {
        self.issues.get(&goal)
    }

    /// Live change procedures in one repository, bounded by the connector's change table.
    #[must_use]
    pub fn changes_for(&self, repository: client::api::Repository, limit: u32) -> Box<[u64]> {
        let mut tasks = List::with_capacity(limit);
        for (task, row) in &self.changes {
            if row.repository == repository {
                tasks.push(*task).expect("change table bounds this list");
            }
        }
        tasks.into_boxed()
    }
}

/// Maximum requests from one top step, including one child burst.
#[must_use]
pub const fn max_out(l: &Limits) -> u32 {
    l.output
}

/// Decide one parent event and collect its durable records and outputs.
pub fn step(d: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::PlanBrief { section, source } => {
            assert!(d.brief_planned.insert(section, source) == Ok(None), "one bounded planned section");
        }
        Event::GatherPlanned { section, parts, bytes, ci_budget } => {
            let Some(source) = d.brief_planned.remove(&section) else {
                out.push(Request::BriefSized { section, size: None });
                return;
            };
            let max_job_bytes = match source {
                crate::BriefSource::Ci { .. } => env.limits.client.answer_bytes.min(ci_budget / 5).max(1),
                crate::BriefSource::Reviews { .. } | crate::BriefSource::Pull { .. } => 0,
            };
            held::gather(d, section, source, parts, bytes, max_job_bytes, env.limits.brief_bytes, out);
        }
        Event::GatherBrief { owner, source, parts, bytes, max_job_bytes } => {
            brief::gather(d, owner, source, parts, bytes, max_job_bytes, env.limits.brief_bytes, out);
        }
        Event::GatherBriefHeld { section, source, parts, bytes, max_job_bytes } => {
            held::gather(d, section, source, parts, bytes, max_job_bytes, env.limits.brief_bytes, out);
        }
        Event::CutBrief { section, bytes } => held::cut(d, section, bytes, out),
        Event::TakeBrief { section } => held::take(d, section, out),
        Event::DropBrief { section } => {
            d.brief_planned.remove(&section);
            held::drop_section(d, section);
        }
        Event::Adopt { reply_to, adoption } => adopt(d, env, reply_to, adoption, out),
        Event::ForgetAdoption { repository, restore } => match restore {
            Some(previous) => {
                d.repositories.insert(repository, previous.clone()).expect("restores an admitted repository");
                emit(out, Request::Save { record: Stored::Repository(previous) });
            }
            None => {
                if d.repositories.remove(&repository).is_some() {
                    emit(out, Request::Erase { key: Key::Repository(repository) });
                }
            }
        },
        Event::Names { task, resources } => names(d, env, task, resources, out),
        Event::Unnamed { task } => unnamed(d, env, task, out),
        Event::Hold { task, resource, from } => hold(d, env, task, resource, from, out),
        Event::Claim { task, attempt, writes, holders } => claim(d, env, task, attempt, &writes, &holders, out),
        Event::Answered { task, attempt, pushed } => answered(d, env, task, attempt, &pushed, out),
        Event::Lost { task, attempt } => lost(d, env, task, attempt, out),
        Event::Enqueue { entry } => enqueue(d, env, entry, out),
        Event::Project { entry, repository, view } => project(d, env, entry, repository, view, out),
        Event::ProjectDesired { entry, goal } => {
            if let Some(row) = d.issues.get(&goal)
                && let Some(view) = &row.desired
            {
                project(d, env, entry, row.repository, view.clone(), out);
            }
        }
        Event::Change { row } => register_change(d, env, row, out),
        Event::QueueRepairStarted { owner, repair } => {
            if let Some(row) = d.changes.get_mut(&owner)
                && !row.base_repair
            {
                row.queue_repair = Some(repair);
                row.queue_repairs = row.queue_repairs.saturating_add(1);
                emit(out, Request::Save { record: Stored::Change(row.clone()) });
            }
        }
        Event::Delegated { task, child, kind } => delegated(d, task, child, kind, out),
        Event::DelegateRefused { task, child } => delegate_refused(d, task, child, out),
        Event::DelegateResult { task, child, status, words } => {
            delegate_result(d, env, task, child, status, &words, out);
        }
        Event::StepChange { task, entry, heard, gates, queue_repair_active } => {
            step_change(d, env, StepInput { task, entry, heard, gates, queue_repair_active }, out);
        }
        Event::ReleaseChange { task } => release_change(d, task, out),
        Event::SettleEffects { task, root, ending } => settle_effects(d, env, task, root, ending, out),
        Event::Release { task, root, ending, entry } => release(d, env, task, root, ending, entry, out),
        Event::ContinueRelease { task, entry } => continue_release(d, env, task, entry, out),
        Event::ReleaseProjection { goal } => release_projection(d, goal, out),
        Event::Committed { entry } => committed(d, env, entry, out),
        Event::Withdraw { entry } => withdraw(d, env, entry, out),
        Event::VetoChange { task, entry, prior } => veto_change(d, task, entry, prior, out),
        Event::Subscribe { subscription } => subscribe(d, env, subscription, out),
        Event::Unsubscribe { task, topic } => unsubscribe(d, task, topic, out),
        Event::Hint { hint } => {
            ci_hint(d, env, &hint, out);
            child(d, env, client::Event::Hint { hint }, out);
        }
        Event::Client(event) => child(d, env, event, out),
        Event::Restore { record } => restore(d, env, record),
        Event::Restored { clock } => restored(d, env, clock, out),
        Event::ReadAfresh => child(d, env, client::Event::ReadAfresh, out),
        Event::SettleOutbox => child(d, env, client::Event::SettleOutbox, out),
    }
}

/// Drive one child call after prior saves have committed.
pub fn resume(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let child_env = child_env(env);
    client::resume(&mut d.client, &child_env, &mut d.client_out);
    drain(d, env, out);
}

/// Expire one client timer.
pub fn fire(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let child_env = child_env(env);
    client::fire(&mut d.client, &child_env, &mut d.client_out);
    drain(d, env, out);
}

fn child_env(env: &Env<Limits>) -> Env<client::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.client }
}

fn child(d: &mut Domain, env: &Env<Limits>, event: client::Event, out: &mut Queue<Request>) {
    client::step(&mut d.client, &child_env(env), event, &mut d.client_out);
    drain(d, env, out);
}

fn drain(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    for _ in 0..d.client_out.len() {
        let request = d.client_out.pop().expect("counted child output");
        match request {
            client::Request::Save { record } => emit(out, Request::Save { record: Stored::Client(record) }),
            client::Request::Erase { key } => emit(out, Request::Erase { key: Key::Client(key) }),
            client::Request::Progress { entry } => {
                d.entries.insert(entry.number, entry.clone()).expect("progress replaces a known entry");
                emit(out, Request::Save { record: Stored::Entry(entry) });
            }
            client::Request::Outcome { entry, task, outcome } => {
                projection_outcome(d, entry, outcome, out);
                change_outcome(d, env, entry, outcome, out);
                release_outcome(d, entry, outcome, out);
                match outcome {
                    client::Outcome::Made { made: client::Made::Merged(commit), .. } => {
                        if d.landed.len() < env.limits.landings || d.landed.contains_key(&commit) {
                            d.landed.insert(commit, task).expect("preflighted landed capacity");
                            emit(out, Request::Save { record: Stored::Landed { commit, task } });
                        }
                    }
                    client::Outcome::Made {
                        made:
                            client::Made::Created(_)
                            | client::Made::Commented(_)
                            | client::Made::Reviewed(_)
                            | client::Made::Updated(_)
                            | client::Made::Branch(_)
                            | client::Made::Set,
                        ..
                    }
                    | client::Outcome::Failed(_)
                    | client::Outcome::Raced { .. }
                    | client::Outcome::Uncertain
                    | client::Outcome::Held
                    | client::Outcome::Withdrawn => {}
                }
                match outcome {
                    client::Outcome::Uncertain => {}
                    client::Outcome::Made { .. }
                    | client::Outcome::Failed(_)
                    | client::Outcome::Raced { .. }
                    | client::Outcome::Held
                    | client::Outcome::Withdrawn => {
                        d.entries.remove(&entry);
                        emit(out, Request::Erase { key: Key::Entry(entry) });
                    }
                }
                emit(out, Request::Outcome { entry, task, outcome });
                if outcome != client::Outcome::Uncertain
                    && match d.releases.get(&task) {
                        Some(row) => !row.failed,
                        None => false,
                    }
                {
                    emit(out, Request::ContinueRelease { task });
                }
            }
            client::Request::Changed { resource, result } => changed(d, env, resource, result, out),
            client::Request::Drift { resource, expected: _, observed: _ } => drift(d, env, resource, out),
            client::Request::Call { call, repository, op } => emit(out, Request::Call { call, repository, op }),
            client::Request::Kept { owner, result } => {
                if result.is_err() {
                    emit(out, Request::Refused { task: owner.raw() });
                }
            }
            client::Request::Read { owner, result } => {
                if d.brief_fetches.contains_key(&owner) {
                    brief::brief_answer(d, owner, result, out);
                } else if d.adoptions.contains_key(&owner) {
                    adoption_read(d, env, owner, result, out);
                } else if d.landings.contains_key(&owner) {
                    landing_read(d, owner, result, out);
                } else if d.steps.contains_key(&owner) {
                    step_read(d, env, owner, result, out);
                } else if d.pending_ci.contains_key(&owner) {
                    ci_read(d, owner, result, out);
                } else if owner.raw() & (1_u64 << 61_u32) != 0 && owner.raw() & (1_u64 << 62_u32) == 0 {
                    emit(out, Request::Read { owner, result });
                } else {
                    lost_read(d, env, owner, result, out);
                }
            }
            client::Request::ReadAfreshDone => emit(out, Request::RestartDone { stage: RestartStage::ReadAfresh }),
            client::Request::OutboxDone => emit(out, Request::RestartDone { stage: RestartStage::Settled }),
        }
    }
}

fn emit(out: &mut Queue<Request>, request: Request) {
    out.push(request);
}

fn valid_name(d: &Domain, l: &Limits, name: &Name) -> bool {
    if !d.repositories.contains_key(&client::api::Repository { forge: name.forge, repository: name.repository }) {
        return false;
    }
    match &name.what {
        What::Repository | What::Pull(_) | What::Issue(_) => true,
        What::Branch(parts) => {
            if parts.is_empty() {
                return false;
            }
            let mut bytes = 0usize;
            for part in parts {
                if part.is_empty() || part.contains(&b'/') {
                    return false;
                }
                let Some(size) = bytes.checked_add(part.len()) else { return false };
                let Some(next) = size.checked_add(1) else {
                    return false;
                };
                bytes = next;
            }
            bytes <= usize::try_from(l.name_bytes).expect("u32 fits usize")
        }
    }
}

fn adopt(d: &mut Domain, env: &Env<Limits>, reply_to: Token, request: Adoption, out: &mut Queue<Request>) {
    let invalid = request.host.is_empty()
        || request.owner.is_empty()
        || request.name.is_empty()
        || request.landing.is_empty()
        || request.prefix.is_empty()
        || request.host.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || request.owner.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || request.name.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || request.landing.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || request.prefix.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || d.adoptions.contains_key(&reply_to)
        || d.adoptions.len() == env.limits.adoptions
        || (!d.repositories.contains_key(&request.provider) && d.repositories.len() == env.limits.repositories);
    if invalid {
        emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::TooLarge) });
        return;
    }
    if let Some(existing) = d.repositories.get(&request.provider)
        && existing.project != request.project
        && request.role != Role::Context
    {
        emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Refused) });
        return;
    }
    if request.home && request.role == Role::Context {
        emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Refused) });
        return;
    }
    if !request.ci && request.checks.is_empty() {
        emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Refused) });
        return;
    }
    if request.checks.len() > usize::try_from(env.limits.change_policy.gates).expect("u32 fits usize")
        || request.checks.contains(&0)
    {
        emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::TooLarge) });
        return;
    }
    for (index, check) in request.checks.iter().enumerate() {
        if request.checks.get(..index).expect("index in checks").contains(check) {
            emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Refused) });
            return;
        }
    }
    if request.home {
        for (_, existing) in &d.repositories {
            if existing.project == request.project && existing.home && existing.provider != request.provider {
                emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Refused) });
                return;
            }
        }
    }
    for (_, pending) in &d.adoptions {
        if pending.request.provider == request.provider {
            emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Busy) });
            return;
        }
    }
    let Some(author) = d.writers.get(&request.provider.forge) else {
        emit(out, Request::Adopted { reply_to, result: Err(client::api::Error::Forbidden) });
        return;
    };
    let repository = request.provider;
    let pending = PendingAdoption {
        request,
        stage: AdoptionStage::Permission,
        permission: None,
        settings: None,
        protection: Protection::Unknown,
        collaborators: List::with_capacity(env.limits.collaborators),
    };
    d.adoptions.insert(reply_to, pending).expect("adoption capacity checked");
    child(
        d,
        env,
        client::Event::Read { owner: reply_to, repository, read: client::api::Read::Permission { user: *author } },
        out,
    );
}

fn adoption_read(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(mut pending) = d.adoptions.remove(&owner) else { return };
    let repository = pending.request.provider;
    match pending.stage {
        AdoptionStage::Permission => match result {
            Ok(client::api::Answer::Permission(permission)) => {
                if permission == client::api::Permission::None {
                    emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::Forbidden) });
                    return;
                }
                pending.permission = Some(permission);
                let check_prefix = pending.request.role != Role::Context && !d.repositories.contains_key(&repository);
                pending.stage = if check_prefix { AdoptionStage::Branches } else { AdoptionStage::Settings };
                d.adoptions.insert(owner, pending).expect("replaces read chain");
                let read = if check_prefix { client::api::Read::Branches } else { client::api::Read::Settings };
                child(d, env, client::Event::Read { owner, repository, read }, out);
            }
            Ok(_) => emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::InvalidAnswer) }),
            Err(error) => emit(out, Request::Adopted { reply_to: owner, result: Err(error) }),
        },
        AdoptionStage::Branches => match result {
            Ok(client::api::Answer::Branches(branches)) => {
                for branch in branches {
                    if branch.starts_with(&pending.request.prefix) {
                        emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::Refused) });
                        return;
                    }
                }
                pending.stage = AdoptionStage::Settings;
                d.adoptions.insert(owner, pending).expect("replaces read chain");
                child(d, env, client::Event::Read { owner, repository, read: client::api::Read::Settings }, out);
            }
            Ok(_) => emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::InvalidAnswer) }),
            Err(error) => emit(out, Request::Adopted { reply_to: owner, result: Err(error) }),
        },
        AdoptionStage::Settings => match result {
            Ok(client::api::Answer::Settings(settings)) => {
                pending.settings = Some(settings);
                pending.stage = AdoptionStage::Protection;
                let branch = pending.request.landing.clone();
                d.adoptions.insert(owner, pending).expect("replaces read chain");
                child(
                    d,
                    env,
                    client::Event::Read { owner, repository, read: client::api::Read::Protection { branch } },
                    out,
                );
            }
            Ok(_) => emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::InvalidAnswer) }),
            Err(error) => emit(out, Request::Adopted { reply_to: owner, result: Err(error) }),
        },
        AdoptionStage::Protection => {
            match result {
                Ok(client::api::Answer::Protection(Some(protection))) => {
                    pending.protection = Protection::Rule(protection);
                }
                Ok(client::api::Answer::Protection(None)) => pending.protection = Protection::Absent,
                Err(client::api::Error::Forbidden) => pending.protection = Protection::Unknown,
                Err(error) => {
                    emit(out, Request::Adopted { reply_to: owner, result: Err(error) });
                    return;
                }
                Ok(_) => {
                    emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::InvalidAnswer) });
                    return;
                }
            }
            pending.stage = AdoptionStage::Collaborators(1);
            d.adoptions.insert(owner, pending).expect("replaces read chain");
            child(
                d,
                env,
                client::Event::Read { owner, repository, read: client::api::Read::Collaborators { page: 1 } },
                out,
            );
        }
        AdoptionStage::Collaborators(page) => adoption_collaborators(d, env, owner, pending, page, result, out),
    }
}

fn adoption_collaborators(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    mut pending: PendingAdoption,
    page: u32,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let repository = pending.request.provider;
    match result {
        Ok(client::api::Answer::Collaborators { collaborators, more }) => {
            for person in collaborators {
                if pending.collaborators.push(person).is_err() {
                    emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::TooLarge) });
                    return;
                }
            }
            if more {
                let Some(next) = page.checked_add(1) else {
                    emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::TooLarge) });
                    return;
                };
                pending.stage = AdoptionStage::Collaborators(next);
                d.adoptions.insert(owner, pending).expect("replaces read chain");
                child(
                    d,
                    env,
                    client::Event::Read { owner, repository, read: client::api::Read::Collaborators { page: next } },
                    out,
                );
            } else {
                finish_adoption(d, owner, pending, out);
            }
        }
        Ok(_) => emit(out, Request::Adopted { reply_to: owner, result: Err(client::api::Error::InvalidAnswer) }),
        Err(error) => emit(out, Request::Adopted { reply_to: owner, result: Err(error) }),
    }
}

fn finish_adoption(d: &mut Domain, owner: Token, pending: PendingAdoption, out: &mut Queue<Request>) {
    let Some(permission) = pending.permission else { unreachable!("permission read before collaborators") };
    let Some(settings) = pending.settings else { unreachable!("settings read before collaborators") };
    let writable = match permission {
        client::api::Permission::Write | client::api::Permission::Admin => true,
        client::api::Permission::None | client::api::Permission::Read => false,
    } && pending.request.role != Role::Context;
    let kinds = Kinds {
        read: true,
        push: writable,
        open: writable,
        land: writable && (settings.merge || settings.squash || settings.rebase),
        review: writable,
        status: writable,
        comment: writable,
        issue: writable,
        branch: writable,
    };
    let request = pending.request;
    let row = Repository {
        project: request.project,
        home: request.home,
        provider: request.provider,
        host: request.host,
        owner: request.owner,
        name: request.name,
        prefix: request.prefix,
        role: request.role,
        ci: request.ci,
        checks: request.checks,
        kinds,
        protection: pending.protection,
        settings,
    };
    d.repositories.insert(row.provider, row.clone()).expect("adoption capacity checked");
    emit(out, Request::Save { record: Stored::Repository(row.clone()) });
    emit(
        out,
        Request::Adopted {
            reply_to: owner,
            result: Ok(Adopted { repository: row, collaborators: pending.collaborators.into_boxed() }),
        },
    );
}

fn to_client(name: &Name, l: &Limits) -> Option<client::Resource> {
    let repository = client::api::Repository { forge: name.forge, repository: name.repository };
    let what = match &name.what {
        What::Repository => client::What::Repository,
        What::Pull(number) => client::What::Pull(*number),
        What::Issue(number) => client::What::Issue(*number),
        What::Branch(parts) => {
            let mut bytes = List::with_capacity(l.name_bytes);
            for (index, part) in parts.iter().enumerate() {
                if index > 0 && bytes.push(b'/').is_err() {
                    return None;
                }
                for byte in part {
                    if bytes.push(*byte).is_err() {
                        return None;
                    }
                }
            }
            client::What::Branch(bytes.into_boxed())
        }
    };
    Some(client::Resource { repository, what })
}

fn participating(d: &Domain, resource: &client::Resource) -> bool {
    let (client::What::Issue(number) | client::What::Pull(number)) = &resource.what else { return false };
    for (_, subscriber) in &d.subscriptions {
        if subscriber.topic == (Topic::Participation { repository: resource.repository, number: *number }) {
            return true;
        }
    }
    false
}

fn names(d: &mut Domain, env: &Env<Limits>, task: u64, resources: Box<[Name]>, out: &mut Queue<Request>) {
    if resources.len() > usize::try_from(env.limits.resources_per_task).expect("u32 fits usize")
        || (!d.names.contains_key(&task) && d.names.len() == env.limits.tasks)
    {
        emit(out, Request::Refused { task });
        return;
    }
    let mut watches = Map::with_capacity(env.limits.client.resources);
    for (other_task, row) in &d.names {
        if *other_task != task {
            for name in row {
                let Some(resource) = to_client(name, &env.limits) else {
                    emit(out, Request::Refused { task });
                    return;
                };
                let participating = participating(d, &resource);
                if watches.insert(resource.clone(), client::Watch { resource, participating }).is_err() {
                    emit(out, Request::Refused { task });
                    return;
                }
            }
        }
    }
    for name in &resources {
        if !valid_name(d, &env.limits, name) {
            emit(out, Request::Refused { task });
            return;
        }
        let Some(resource) = to_client(name, &env.limits) else {
            emit(out, Request::Refused { task });
            return;
        };
        let participating = participating(d, &resource);
        if watches.insert(resource.clone(), client::Watch { resource, participating }).is_err() {
            emit(out, Request::Refused { task });
            return;
        }
    }
    let mut list: List<client::Watch> = List::with_capacity(env.limits.client.resources);
    for (_, watch) in &watches {
        list.push(watch.clone()).expect("map capped to watch list");
    }
    client::step(
        &mut d.client,
        &child_env(env),
        client::Event::Keep { owner: Token::new(task), watches: list.into_boxed() },
        &mut d.client_out,
    );
    if kept(&d.client_out) {
        d.names.insert(task, resources.clone()).expect("preflighted task capacity");
        emit(out, Request::Save { record: Stored::Names { task, resources } });
    }
    drain(d, env, out);
}

fn unnamed(d: &mut Domain, env: &Env<Limits>, task: u64, out: &mut Queue<Request>) {
    let mut list: List<client::Watch> = List::with_capacity(env.limits.client.resources);
    for (owner, row) in &d.names {
        if *owner == task {
            continue;
        }
        for name in row {
            let Some(resource) = to_client(name, &env.limits) else { continue };
            let mut present = false;
            for watch in &list {
                if watch.resource == resource {
                    present = true;
                }
            }
            let participating = participating(d, &resource);
            if !present && list.push(client::Watch { resource, participating }).is_err() {
                emit(out, Request::Refused { task });
                return;
            }
        }
    }
    client::step(
        &mut d.client,
        &child_env(env),
        client::Event::Keep { owner: Token::new(task), watches: list.into_boxed() },
        &mut d.client_out,
    );
    if kept(&d.client_out) && d.names.remove(&task).is_some() {
        emit(out, Request::Erase { key: Key::Names(task) });
    }
    drain(d, env, out);
}

fn kept(out: &Queue<client::Request>) -> bool {
    for request in out {
        match request {
            client::Request::Kept { result, .. } => return result.is_ok(),
            client::Request::Changed { .. }
            | client::Request::Drift { .. }
            | client::Request::Save { .. }
            | client::Request::Progress { .. }
            | client::Request::Erase { .. }
            | client::Request::Outcome { .. }
            | client::Request::Call { .. }
            | client::Request::Read { .. }
            | client::Request::ReadAfreshDone
            | client::Request::OutboxDone => {}
        }
    }
    false
}

fn hold(d: &mut Domain, env: &Env<Limits>, task: u64, resource: Name, from: Option<u64>, out: &mut Queue<Request>) {
    if !valid_name(d, &env.limits, &resource) || (!d.holds.contains_key(&resource) && d.holds.len() == env.limits.holds)
    {
        emit(out, Request::Refused { task });
        return;
    }
    if let Some(existing) = d.holds.get(&resource) {
        if existing.task != task && Some(existing.task) != from {
            emit(out, Request::Taken { task, resource, by: existing.task });
            return;
        }
    } else if from.is_some() {
        emit(out, Request::Refused { task });
        return;
    }
    let writer = match d.holds.get(&resource) {
        Some(row) => row.writer,
        None => None,
    };
    let row = Hold { name: resource.clone(), task, writer };
    d.holds.insert(resource, row.clone()).expect("preflighted hold capacity");
    emit(out, Request::Save { record: Stored::Hold(row) });
}

fn claim(
    d: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    attempt: u64,
    writes: &[Name],
    holders: &[u64],
    out: &mut Queue<Request>,
) {
    if writes.len() > usize::try_from(env.limits.resources_per_task).expect("u32 fits usize") {
        emit(out, Request::Refused { task });
        return;
    }
    for name in writes {
        let Some(row) = d.holds.get(name) else {
            emit(out, Request::Refused { task });
            return;
        };
        if row.task != task && !holders.contains(&row.task) {
            emit(out, Request::Taken { task, resource: name.clone(), by: row.task });
            return;
        }
        match row.writer {
            Some(Writer::Run { task: owner, attempt: old }) if owner == task && old == attempt => {}
            Some(Writer::Run { task: owner, .. }) => {
                emit(out, Request::Taken { task, resource: name.clone(), by: owner });
                return;
            }
            Some(Writer::Entry(entry)) => {
                emit(out, Request::Taken { task, resource: name.clone(), by: entry });
                return;
            }
            None => {}
        }
    }
    for name in writes {
        let Some(row) = d.holds.get_mut(name) else { unreachable!("preflighted hold") };
        row.writer = Some(Writer::Run { task, attempt });
        let saved = row.clone();
        emit(out, Request::Save { record: Stored::Hold(saved) });
        if let Some(resource) = to_client(name, &env.limits) {
            child(d, env, client::Event::Writer { resource, taken: true }, out);
        }
    }
}

fn answered(
    d: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    attempt: u64,
    pushed: &[(Name, client::api::Commit)],
    out: &mut Queue<Request>,
) {
    let mut freed = List::with_capacity(env.limits.holds);
    for (name, row) in &d.holds {
        if row.writer == Some(Writer::Run { task, attempt }) {
            freed.push(name.clone()).expect("list sized to holds");
        }
    }
    for name in &freed {
        let Some(row) = d.holds.get_mut(name) else { unreachable!("collected hold") };
        row.writer = None;
        let saved = row.clone();
        emit(out, Request::Save { record: Stored::Hold(saved) });
        if let Some(resource) = to_client(name, &env.limits) {
            for (pushed_name, commit) in pushed {
                if pushed_name == name {
                    child(d, env, client::Event::Pushed { resource: resource.clone(), commit: *commit }, out);
                }
            }
            child(d, env, client::Event::Writer { resource, taken: false }, out);
        }
    }
}

fn lost(d: &mut Domain, env: &Env<Limits>, task: u64, attempt: u64, out: &mut Queue<Request>) {
    let mut names = List::with_capacity(env.limits.holds);
    for (name, row) in &d.holds {
        if row.writer == Some(Writer::Run { task, attempt }) {
            names.push(name.clone()).expect("list sized to holds");
        }
    }
    for name in &names {
        let Some(resource) = to_client(name, &env.limits) else { continue };
        let read = match resource.what {
            client::What::Branch(branch) => client::api::Read::Branch { branch },
            client::What::Repository | client::What::Pull(_) | client::What::Issue(_) => {
                emit(out, Request::Refused { task });
                continue;
            }
        };
        let Some(sequence) = d.sequence.checked_add(1) else {
            emit(out, Request::Refused { task });
            return;
        };
        d.sequence = sequence;
        let owner = Token::new(sequence | (1_u64 << 62));
        if d.lost.insert(owner, PendingLost { task, attempt, name: name.clone() }).is_err() {
            emit(out, Request::Refused { task });
            return;
        }
        child(d, env, client::Event::Read { owner, repository: resource.repository, read }, out);
    }
}

fn lost_read(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(pending) = d.lost.remove(&owner) else { return };
    let observed = match result {
        Ok(client::api::Answer::Commit(commit)) => Some(commit),
        Err(client::api::Error::Missing) => None,
        Ok(_) | Err(_) => {
            emit(out, Request::Refused { task: pending.task });
            return;
        }
    };
    let Some(row) = d.holds.get_mut(&pending.name) else { return };
    if row.writer != Some(Writer::Run { task: pending.task, attempt: pending.attempt }) {
        return;
    }
    row.writer = None;
    let saved = row.clone();
    emit(out, Request::Save { record: Stored::Hold(saved) });
    if let Some(commit) = observed {
        let head = BranchHead { name: pending.name.clone(), commit };
        if d.heads.insert(pending.name.clone(), head.clone()).is_err() {
            emit(out, Request::Refused { task: pending.task });
            return;
        }
        emit(out, Request::Save { record: Stored::BranchHead(head) });
    }
    if let Some(resource) = to_client(&pending.name, &env.limits) {
        match observed {
            Some(commit) => child(d, env, client::Event::Pushed { resource: resource.clone(), commit }, out),
            None => emit(out, Request::Drift { task: pending.task, resource: pending.name.clone() }),
        }
        child(d, env, client::Event::Writer { resource, taken: false }, out);
    }
}

fn enqueue(d: &mut Domain, env: &Env<Limits>, entry: client::Entry, out: &mut Queue<Request>) {
    let task = entry.task;
    let Some(repository) = d.repositories.get(&entry.repository) else {
        emit(out, Request::Refused { task });
        return;
    };
    if repository.role == Role::Context
        || !effect_permitted(repository.kinds, &entry.effect.write)
        || (!d.entries.contains_key(&entry.number) && d.entries.len() == env.limits.entries)
    {
        emit(out, Request::Refused { task });
        return;
    }
    if d.entries.contains_key(&entry.number) {
        emit(out, Request::Refused { task });
        return;
    }
    d.entries.insert(entry.number, entry.clone()).expect("preflighted entry capacity");
    emit(out, Request::Save { record: Stored::Entry(entry) });
}

fn effect_permitted(kinds: Kinds, write: &client::api::Write) -> bool {
    match write {
        client::api::Write::CreateIssue { .. } => kinds.issue,
        client::api::Write::OpenPull { .. } => kinds.open,
        client::api::Write::Post { .. } => kinds.comment,
        client::api::Write::Review { .. } | client::api::Write::SetReviewers { .. } => kinds.review,
        client::api::Write::Edit { .. } | client::api::Write::Close { .. } | client::api::Write::Reopen { .. } => {
            kinds.issue || kinds.open
        }
        client::api::Write::Merge { .. } => kinds.land,
        client::api::Write::Update { .. }
        | client::api::Write::CreateBranch { .. }
        | client::api::Write::DeleteBranch { .. } => kinds.branch,
        client::api::Write::Status { .. } => kinds.status,
    }
}

fn register_change(d: &mut Domain, env: &Env<Limits>, row: ChangeRow, out: &mut Queue<Request>) {
    let task = row.task;
    if !d.repositories.contains_key(&row.repository)
        || row.change.task != task
        || row.branch.is_empty()
        || row.base.is_empty()
        || row.branch.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || row.base.len() > usize::try_from(env.limits.name_bytes).expect("u32 fits usize")
        || row.title.len() > usize::try_from(env.limits.client.op_bytes).expect("u32 fits usize")
        || row.body.len() > usize::try_from(env.limits.client.op_bytes).expect("u32 fits usize")
        || (!d.changes.contains_key(&task) && d.changes.len() == env.limits.changes)
        || d.changes.contains_key(&task)
    {
        emit(out, Request::Refused { task });
        return;
    }
    d.changes.insert(task, row.clone()).expect("change capacity checked");
    emit(out, Request::Save { record: Stored::Change(row) });
}

fn delegated(d: &mut Domain, task: u64, child: u64, kind: change::Delegate, out: &mut Queue<Request>) {
    let Some(row) = d.changes.get_mut(&task) else {
        emit(out, Request::Refused { task });
        return;
    };
    row.delegate = Some((child, kind));
    row.delegate_status = change::Status::Unknown;
    emit(out, Request::Save { record: Stored::Change(row.clone()) });
}

fn delegate_refused(d: &mut Domain, task: u64, child: u64, out: &mut Queue<Request>) {
    let Some(row) = d.changes.get_mut(&task) else { return };
    let Some((expected, kind)) = row.delegate else { return };
    if expected != child {
        return;
    }
    row.delegate = None;
    row.delegate_status = change::Status::Unknown;
    let state = match (kind, &row.change.state) {
        (change::Delegate::Produce, change::State::Producing { .. }) => change::State::Producing { requested: false },
        (change::Delegate::Repair(why), change::State::Repairing { .. }) => {
            row.change.repairs = row.change.repairs.saturating_sub(1);
            change::State::Repairing { requested: false, why }
        }
        (change::Delegate::Resolve { base }, change::State::Resolving { .. }) => {
            row.change.resolutions = row.change.resolutions.saturating_sub(1);
            change::State::Resolving { requested: false, base }
        }
        (change::Delegate::Gate { number, .. }, change::State::Gating { head, asked }) => {
            let mut remaining = List::with_capacity(u32::try_from(asked.len()).expect("bounded gate list"));
            for prior in asked {
                if *prior != number {
                    remaining.push(*prior).expect("same gate room");
                }
            }
            change::State::Gating { head: *head, asked: remaining.into_boxed() }
        }
        _ => row.change.state.clone(),
    };
    row.change.state = change::State::Held { was: Box::new(state), why: change::Hold::Failed };
    emit(out, Request::Save { record: Stored::Change(row.clone()) });
}

fn delegate_result(
    d: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    child: u64,
    status: change::Status,
    words: &[u8],
    out: &mut Queue<Request>,
) {
    let Some(row) = d.changes.get_mut(&task) else { return };
    let Some((expected, kind)) = row.delegate else { return };
    if expected != child {
        return;
    }
    row.delegate = None;
    row.delegate_status = status;
    match kind {
        change::Delegate::Gate { number, head } => {
            let report = change::GateReport { number, head, status };
            let mut verdicts = List::with_capacity(env.limits.change_policy.gates);
            for old in &row.verdicts {
                if old.number != number {
                    verdicts.push(*old).expect("bounded prior gate reports");
                }
            }
            if verdicts.push(report).is_err() {
                emit(out, Request::Refused { task });
                return;
            }
            row.verdicts = verdicts.into_boxed();
            let mut remarks = List::with_capacity(env.limits.change_policy.gates);
            for old in &row.gate_remarks {
                if old.number != number {
                    remarks.push(old.clone()).expect("bounded prior gate remarks");
                }
            }
            let kept = words.len().min(usize::try_from(env.limits.client.answer_bytes).expect("u32 fits usize"));
            remarks
                .push(crate::GateRemark {
                    number,
                    head,
                    words: Box::from(words.get(..kept).expect("kept is in bounds")),
                })
                .expect("gate room checked");
            row.gate_remarks = remarks.into_boxed();
        }
        change::Delegate::Produce | change::Delegate::Repair(_) | change::Delegate::Resolve { .. } => {}
    }
    emit(out, Request::Save { record: Stored::Change(row.clone()) });
}

fn step_change(d: &mut Domain, env: &Env<Limits>, input: StepInput, out: &mut Queue<Request>) {
    let StepInput { task, entry, heard, gates, queue_repair_active } = input;
    let Some(row) = d.changes.get(&task) else {
        emit(out, Request::Refused { task });
        return;
    };
    for (_, pending) in &d.steps {
        if pending.task == task {
            // A timer or a delegate result can wake this procedure while its
            // prior fresh-read chain is still in flight.
            return;
        }
    }
    if gates.len() > usize::try_from(env.limits.change_policy.gates).expect("u32 fits usize")
        || d.steps.len() == env.limits.changes
    {
        emit(out, Request::Refused { task });
        return;
    }
    let repository = row.repository;
    let branch = row.branch.clone();
    let Some(sequence) = d.sequence.checked_add(1) else {
        emit(out, Request::Refused { task });
        return;
    };
    d.sequence = sequence;
    let owner = Token::new(sequence | (1_u64 << 61));
    let pending = PendingStep {
        task,
        entry,
        stage: StepStage::Branch,
        heard: change::Heard {
            delegate: if row.delegate_status == change::Status::Unknown { heard.delegate } else { row.delegate_status },
            ..heard
        },
        gates: if gates.is_empty() { row.verdicts.clone() } else { gates },
        queue_repair_active,
        branch: None,
        base_tip: None,
        pull: None,
        ci: change::Status::Unknown,
        base_ci: change::Status::Unknown,
        contains_base: change::Status::Unknown,
        reviews: Box::new([]),
        reviews_complete: true,
    };
    d.steps.insert(owner, pending).expect("step capacity checked");
    child(d, env, client::Event::Read { owner, repository, read: client::api::Read::Branch { branch } }, out);
}

fn status(ci: client::api::Ci) -> change::Status {
    match ci {
        client::api::Ci::None => change::Status::Unknown,
        client::api::Ci::Pending => change::Status::Pending,
        client::api::Ci::Passed => change::Status::Passed,
        client::api::Ci::Failed => change::Status::Failed,
    }
}

#[expect(clippy::too_many_lines, reason = "one bounded fresh-read chain preserves its stage and facts")]
fn step_read(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(mut pending) = d.steps.remove(&owner) else { return };
    if result == Err(client::api::Error::Busy) {
        emit(
            out,
            Request::ChangeDecision {
                task: pending.task,
                decision: change::Decision::Wait {
                    until: Some(skein_lib::Wall::from_nanos(env.wall.as_nanos().saturating_add(1_000_000_000))),
                },
                entry: None,
                evidence: None,
            },
        );
        return;
    }
    let Some(row) = d.changes.get(&pending.task) else { return };
    let repository = row.repository;
    let branch = row.branch.clone();
    let base = row.base.clone();
    let next = match pending.stage {
        StepStage::Branch => {
            match result {
                Ok(client::api::Answer::Commit(commit)) => pending.branch = Some(commit),
                Err(client::api::Error::Missing) => pending.branch = None,
                Ok(_) | Err(_) => {
                    emit(out, Request::Refused { task: pending.task });
                    return;
                }
            }
            pending.stage = StepStage::Base;
            client::api::Read::Branch { branch: base }
        }
        StepStage::Base => {
            match result {
                Ok(client::api::Answer::Commit(commit)) => pending.base_tip = Some(commit),
                Ok(_) | Err(_) => {
                    emit(out, Request::Refused { task: pending.task });
                    return;
                }
            }
            pending.stage = StepStage::Pull;
            client::api::Read::PullFor { head: branch, base }
        }
        StepStage::Pull => {
            match result {
                Ok(client::api::Answer::Pull(pull)) => pending.pull = Some(pull),
                Err(client::api::Error::Missing) => pending.pull = None,
                Ok(_) | Err(_) => {
                    emit(out, Request::Refused { task: pending.task });
                    return;
                }
            }
            pending.stage = StepStage::Ci;
            let Some(commit) = pending.branch else {
                pending.stage = StepStage::BaseCi;
                let Some(base_tip) = pending.base_tip else { unreachable!("base read") };
                d.steps.insert(owner, pending).expect("replaces step");
                child(
                    d,
                    env,
                    client::Event::Read {
                        owner,
                        repository,
                        read: client::api::Read::Statuses { commit: base_tip, page: 1 },
                    },
                    out,
                );
                return;
            };
            client::api::Read::Statuses { commit, page: 1 }
        }
        StepStage::Ci => {
            match result {
                Ok(client::api::Answer::Statuses { ci, .. }) => pending.ci = status(ci),
                Ok(_) | Err(_) => pending.ci = change::Status::Unknown,
            }
            pending.stage = StepStage::BaseCi;
            let Some(base_tip) = pending.base_tip else { unreachable!("base read") };
            client::api::Read::Statuses { commit: base_tip, page: 1 }
        }
        StepStage::BaseCi => {
            match result {
                Ok(client::api::Answer::Statuses { ci, .. }) => pending.base_ci = status(ci),
                Ok(_) | Err(_) => pending.base_ci = change::Status::Unknown,
            }
            let Some(head) = pending.branch else {
                finish_step(d, env, pending, out);
                return;
            };
            let Some(base_tip) = pending.base_tip else { unreachable!("base read") };
            pending.stage = StepStage::Compare;
            client::api::Read::Compare { before: base_tip, after: head }
        }
        StepStage::Compare => {
            match result {
                Ok(client::api::Answer::Compare { before, after, contains_before, .. })
                    if Some(before) == pending.base_tip && Some(after) == pending.branch =>
                {
                    pending.contains_base =
                        if contains_before { change::Status::Passed } else { change::Status::Failed };
                }
                Ok(_) | Err(_) => pending.contains_base = change::Status::Unknown,
            }
            if let Some(pull) = &pending.pull {
                pending.stage = StepStage::Reviews(1);
                client::api::Read::Reviews { number: pull.number, page: 1 }
            } else {
                finish_step(d, env, pending, out);
                return;
            }
        }
        StepStage::Reviews(page) => match result {
            Ok(client::api::Answer::Reviews { reviews, more }) => {
                let Some(total) = pending.reviews.len().checked_add(reviews.len()) else {
                    emit(out, Request::Refused { task: pending.task });
                    return;
                };
                if total > usize::try_from(env.limits.client.inbox).expect("u32 fits usize") {
                    pending.reviews_complete = false;
                    finish_step(d, env, pending, out);
                    return;
                }
                let mut gathered = List::with_capacity(env.limits.client.inbox);
                for review in &pending.reviews {
                    gathered.push(review.clone()).expect("review bound preflighted");
                }
                for review in reviews {
                    gathered.push(review).expect("review bound preflighted");
                }
                pending.reviews = gathered.into_boxed();
                if more {
                    let Some(next) = page.checked_add(1) else {
                        pending.reviews_complete = false;
                        finish_step(d, env, pending, out);
                        return;
                    };
                    pending.stage = StepStage::Reviews(next);
                    let Some(pull) = &pending.pull else { unreachable!("review read has a pull") };
                    client::api::Read::Reviews { number: pull.number, page: next }
                } else {
                    finish_step(d, env, pending, out);
                    return;
                }
            }
            Ok(_) | Err(_) => {
                pending.reviews_complete = false;
                finish_step(d, env, pending, out);
                return;
            }
        },
    };
    d.steps.insert(owner, pending).expect("replaces step");
    child(d, env, client::Event::Read { owner, repository, read: next }, out);
}

#[expect(
    clippy::match_like_matches_macro,
    clippy::wildcard_enum_match_arm,
    reason = "the queue tests its currently known ready states without the disallowed matches macro"
)]
fn queue_first(d: &Domain, row: &ChangeRow, now: skein_lib::Wall, l: &Limits) -> bool {
    for (_, repair) in &d.changes {
        if repair.repository == row.repository
            && repair.base == row.base
            && repair.base_repair
            && match repair.change.state {
                change::State::Queued { .. } | change::State::First { .. } => true,
                _ => false,
            }
        {
            return row.task == repair.task;
        }
    }
    let mut ready = List::with_capacity(l.changes);
    for (_, other) in &d.changes {
        if other.repository != row.repository || other.base != row.base {
            continue;
        }
        let since = match other.change.state {
            change::State::Queued { ready_since, .. } | change::State::First { ready_since, .. } => Some(ready_since),
            change::State::Producing { .. }
            | change::State::Opening { .. }
            | change::State::Recreating { .. }
            | change::State::Reopening { .. }
            | change::State::Checking { .. }
            | change::State::Gating { .. }
            | change::State::Updating { .. }
            | change::State::Resolving { .. }
            | change::State::Repairing { .. }
            | change::State::Landing { .. }
            | change::State::Landed { .. }
            | change::State::Held { .. } => None,
        };
        if let Some(since) = since {
            ready
                .push(change::Ready { task: other.task, priority: other.priority, since })
                .expect("changes bound ready");
        }
    }
    change::first(ready.as_slice(), l.queue_window, now) == Some(row.task)
}

fn finish_step(d: &mut Domain, env: &Env<Limits>, pending: PendingStep, out: &mut Queue<Request>) {
    let Some(mut row) = d.changes.get(&pending.task).cloned() else { return };
    let prior = row.change.clone();
    let Some(base_tip) = pending.base_tip else {
        emit(out, Request::Refused { task: pending.task });
        return;
    };
    row.pull = match &pending.pull {
        Some(pull) => Some(pull.number),
        None => row.pull,
    };
    let facts = change_facts(d, env, &row, &pending, base_tip);
    let heard = change::Heard {
        effect: if row.pending.is_some() { change::EffectResult::Pending } else { row.effect },
        ..pending.heard
    };
    for _ in 0_u32..16 {
        let stepped = change::step(&row.change, &facts, &heard, env.wall, &env.limits.change_policy);
        let advanced = stepped.change != row.change;
        row.change = stepped.change;
        match stepped.decision {
            change::Decision::None if advanced => {}
            change::Decision::Effect(effect) => {
                emit_change_effect(d, env, &mut row, &pending, prior, effect, out);
                return;
            }
            decision @ (change::Decision::None
            | change::Decision::Wait { .. }
            | change::Decision::Delegate(_)
            | change::Decision::Ready
            | change::Decision::QueueRepair
            | change::Decision::Finish { .. }
            | change::Decision::Cancel
            | change::Decision::Hold(_)) => {
                d.changes.insert(row.task, row.clone()).expect("replaces change");
                emit(out, Request::Save { record: Stored::Change(row.clone()) });
                emit(out, Request::ChangeDecision { task: row.task, decision, entry: None, evidence: None });
                return;
            }
        }
    }
    emit(out, Request::Refused { task: row.task });
}

fn change_facts(
    d: &Domain,
    env: &Env<Limits>,
    row: &ChangeRow,
    pending: &PendingStep,
    base_tip: client::api::Commit,
) -> change::Facts {
    let pull = match &pending.pull {
        Some(pull) if pull.merged.is_some() => change::Pull::Merged { commit: pull.merged.expect("checked merged") },
        Some(pull) if pull.state == client::api::State::Closed => change::Pull::Closed,
        Some(pull) => change::Pull::Open { head: pull.commit, base: pull.base_commit.unwrap_or(base_tip) },
        None => change::Pull::Missing,
    };
    let retargeted = match &pending.pull {
        Some(pull) => pull.base != row.base,
        None => false,
    };
    let branch_name = client::Resource { repository: row.repository, what: client::What::Branch(row.branch.clone()) };
    let writer_taken = match from_client(&branch_name, &env.limits) {
        Some(name) => match d.holds.get(&name) {
            Some(hold) => hold.writer.is_some(),
            None => false,
        },
        None => false,
    };
    let may_merge = match d.repositories.get(&row.repository) {
        Some(repository) => repository.kinds.land,
        None => false,
    };
    let has_ci = match d.repositories.get(&row.repository) {
        Some(repository) => repository.ci,
        None => true,
    };
    change::Facts {
        branch: pending.branch,
        expected_base: base_tip,
        pull,
        ci: change::Ci {
            head: pending.branch.unwrap_or([0; 32]),
            status: if has_ci { pending.ci } else { change::Status::Passed },
        },
        base_ci: change::Ci {
            head: base_tip,
            status: if !has_ci || row.base_repair { change::Status::Passed } else { pending.base_ci },
        },
        base_tip,
        contains_base: pending.contains_base,
        mergeable: match &pending.pull {
            Some(pull) if pull.mergeable => change::Status::Passed,
            Some(_) => change::Status::Failed,
            None => change::Status::Unknown,
        },
        gates: pending.gates.clone(),
        writer_taken,
        drift: if retargeted { Some(change::Hold::Retargeted) } else { row.drift },
        first: queue_first(d, row, env.wall, &env.limits),
        may_merge,
        queue_repair_active: pending.queue_repair_active,
    }
}

fn emit_change_effect(
    d: &mut Domain,
    env: &Env<Limits>,
    row: &mut ChangeRow,
    pending: &PendingStep,
    prior: change::Change,
    effect: change::Effect,
    out: &mut Queue<Request>,
) {
    let Some(write) = change_write(row, effect) else {
        emit(
            out,
            Request::ChangeDecision {
                task: row.task,
                decision: change::Decision::Hold(change::Hold::Failed),
                entry: None,
                evidence: None,
            },
        );
        return;
    };
    if d.entries.contains_key(&pending.entry) || d.entries.len() == env.limits.entries {
        emit(out, Request::Refused { task: row.task });
        return;
    }
    let condition = match effect {
        change::Effect::Update { head, base } => client::Condition::Update { head, base },
        change::Effect::Merge { .. } => client::Condition::Merge { base: row.base.clone() },
        change::Effect::Open
        | change::Effect::CreateBranch { .. }
        | change::Effect::Reopen
        | change::Effect::Retarget => client::Condition::None,
    };
    let entry = client::Entry {
        number: pending.entry,
        task: row.task,
        repository: row.repository,
        effect: client::Effect { write, condition },
        start: None,
        attempt: None,
        failures: 0,
    };
    row.pending = Some(pending.entry);
    row.effect = change::EffectResult::Pending;
    d.changes.insert(row.task, row.clone()).expect("replaces change");
    emit(out, Request::Save { record: Stored::Change(row.clone()) });
    d.entries.insert(entry.number, entry.clone()).expect("entry capacity checked");
    emit(out, Request::Save { record: Stored::Entry(entry) });
    emit(
        out,
        Request::ChangeDecision {
            task: row.task,
            decision: change::Decision::Effect(effect),
            entry: Some(pending.entry),
            evidence: Some(crate::ChangeEvidence {
                prior,
                head: pending.branch,
                base_tip: pending.base_tip.expect("step read base tip"),
                contains_base: pending.contains_base,
                ci: if match d.repositories.get(&row.repository) {
                    Some(repository) => repository.ci,
                    None => true,
                } {
                    pending.ci
                } else {
                    change::Status::Passed
                },
                gates: pending.gates.clone(),
                reviews: pending.reviews.clone(),
                reviews_complete: pending.reviews_complete,
            }),
        },
    );
}

fn change_write(row: &ChangeRow, effect: change::Effect) -> Option<client::api::Write> {
    match effect {
        change::Effect::Open => Some(client::api::Write::OpenPull {
            title: row.title.clone(),
            body: row.body.clone(),
            head: row.branch.clone(),
            base: row.base.clone(),
        }),
        change::Effect::CreateBranch { head } => {
            Some(client::api::Write::CreateBranch { branch: row.branch.clone(), commit: head })
        }
        change::Effect::Reopen => Some(client::api::Write::Reopen { number: row.pull? }),
        change::Effect::Retarget => None,
        change::Effect::Update { .. } => Some(client::api::Write::Update { number: row.pull? }),
        change::Effect::Merge { head, .. } => Some(client::api::Write::Merge { number: row.pull?, head }),
    }
}

fn change_outcome(d: &mut Domain, env: &Env<Limits>, entry: u64, outcome: client::Outcome, out: &mut Queue<Request>) {
    let mut task = None;
    for (number, row) in &d.changes {
        if row.pending == Some(entry) {
            task = Some(*number);
        }
    }
    let Some(task) = task else { return };
    let Some(row) = d.changes.get_mut(&task) else { unreachable!("found change row") };
    let pull_closed = match outcome {
        client::Outcome::Raced { made: client::Made::Created(_), why: client::api::Error::Closed } => true,
        client::Outcome::Made { .. }
        | client::Outcome::Failed(_)
        | client::Outcome::Raced { .. }
        | client::Outcome::Uncertain
        | client::Outcome::Held
        | client::Outcome::Withdrawn => false,
    };
    row.effect = match outcome {
        client::Outcome::Made { .. } => change::EffectResult::Made,
        client::Outcome::Failed(client::api::Error::Conflict) => change::EffectResult::Conflict,
        client::Outcome::Failed(_)
        | client::Outcome::Raced { .. }
        | client::Outcome::Held
        | client::Outcome::Withdrawn => change::EffectResult::Failed,
        client::Outcome::Uncertain => return,
    };
    row.pending = None;
    if pull_closed {
        row.drift = Some(change::Hold::PullClosed);
    }
    let saved = row.clone();
    emit(out, Request::Save { record: Stored::Change(saved.clone()) });
    if pull_closed {
        let branch =
            client::Resource { repository: saved.repository, what: client::What::Branch(saved.branch.clone()) };
        if let Some(resource) = from_client(&branch, &env.limits) {
            emit(out, Request::Drift { task, resource });
        }
    }
    match outcome {
        client::Outcome::Made { made: client::Made::Updated(commit), .. } => {
            child(
                d,
                env,
                client::Event::Pushed {
                    resource: client::Resource {
                        repository: saved.repository,
                        what: client::What::Branch(saved.branch),
                    },
                    commit,
                },
                out,
            );
        }
        client::Outcome::Made {
            made:
                client::Made::Created(_)
                | client::Made::Commented(_)
                | client::Made::Reviewed(_)
                | client::Made::Merged(_)
                | client::Made::Branch(_)
                | client::Made::Set,
            ..
        }
        | client::Outcome::Failed(_)
        | client::Outcome::Raced { .. }
        | client::Outcome::Uncertain
        | client::Outcome::Held
        | client::Outcome::Withdrawn => {}
    }
    emit(out, Request::ChangeDecision { task, decision: change::Decision::None, entry: None, evidence: None });
}

fn release_change(d: &mut Domain, task: u64, out: &mut Queue<Request>) {
    let Some(row) = d.changes.get_mut(&task) else {
        emit(out, Request::Refused { task });
        return;
    };
    row.drift = None;
    let saved = row.clone();
    emit(out, Request::Save { record: Stored::Change(saved) });
}

fn settle_effects(
    d: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    root: u64,
    ending: ReleaseEnding,
    out: &mut Queue<Request>,
) {
    if let Some(row) = d.releases.get_mut(&task) {
        if row.root != root || (row.ending != ending && ending != ReleaseEnding::Cancelled) {
            emit(out, Request::ReleaseFailed { task });
            return;
        }
        if row.ending != ending {
            row.ending = ending;
            row.close_pull = match d.changes.get(&task) {
                Some(change) => change.pull,
                None => None,
            };
            emit(out, Request::Save { record: Stored::Release(row.clone()) });
        }
        if row.effects_settled {
            emit(out, Request::EffectsSettled { task });
            return;
        }
    } else {
        if d.releases.len() == env.limits.tasks {
            emit(out, Request::ReleaseFailed { task });
            return;
        }
        let row = ReleaseRow {
            task,
            root,
            ending,
            close_pull: if ending == ReleaseEnding::Cancelled {
                match d.changes.get(&task) {
                    Some(change) => change.pull,
                    None => None,
                }
            } else {
                None
            },
            pending: None,
            failed: false,
            effects_settled: false,
            releasing: false,
        };
        d.releases.insert(task, row.clone()).expect("release capacity checked");
        emit(out, Request::Save { record: Stored::Release(row) });
    }
    if ending == ReleaseEnding::Cancelled {
        let mut unsent = List::with_capacity(env.limits.entries);
        for (number, entry) in &d.entries {
            if entry.task == task && entry.attempt.is_none() {
                unsent.push(*number).expect("entry table bounded");
            }
        }
        for number in unsent.into_boxed() {
            withdraw(d, env, number, out);
            if let Some(entry) = d.entries.get(&number)
                && entry.start.is_none()
            {
                d.entries.remove(&number);
                emit(out, Request::Erase { key: Key::Entry(number) });
                emit(out, Request::Outcome { entry: number, task, outcome: client::Outcome::Withdrawn });
            }
        }
    }
    settle_prior_effects(d, task, out);
}

fn settle_prior_effects(d: &mut Domain, task: u64, out: &mut Queue<Request>) {
    for (_, pending) in &d.entries {
        if pending.task == task {
            return;
        }
    }
    let row = d.releases.get_mut(&task).expect("effect settlement row");
    if !row.effects_settled {
        row.effects_settled = true;
        emit(out, Request::Save { record: Stored::Release(row.clone()) });
    }
    emit(out, Request::EffectsSettled { task });
}

fn release(
    d: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    root: u64,
    ending: ReleaseEnding,
    entry: u64,
    out: &mut Queue<Request>,
) {
    if let Some(row) = d.releases.get_mut(&task) {
        if row.root != root || (row.ending != ending && ending != ReleaseEnding::Cancelled) {
            emit(out, Request::ReleaseFailed { task });
            return;
        }
        if row.ending != ending {
            row.ending = ending;
            row.close_pull = match d.changes.get(&task) {
                Some(change) => change.pull,
                None => None,
            };
        }
        if row.failed {
            row.failed = false;
        }
        row.releasing = true;
        emit(out, Request::Save { record: Stored::Release(row.clone()) });
    } else {
        if d.releases.len() == env.limits.tasks {
            emit(out, Request::ReleaseFailed { task });
            return;
        }
        let row = ReleaseRow {
            task,
            root,
            ending,
            close_pull: if ending == ReleaseEnding::Cancelled {
                match d.changes.get(&task) {
                    Some(change) => change.pull,
                    None => None,
                }
            } else {
                None
            },
            pending: None,
            failed: false,
            effects_settled: true,
            releasing: true,
        };
        d.releases.insert(task, row.clone()).expect("release capacity checked");
        emit(out, Request::Save { record: Stored::Release(row) });
    }
    continue_release(d, env, task, entry, out);
}

fn continue_release(d: &mut Domain, env: &Env<Limits>, task: u64, entry: u64, out: &mut Queue<Request>) {
    for _ in 0..=env.limits.holds {
        let Some(mut row) = d.releases.get(&task).cloned() else { return };
        if row.failed || row.pending.is_some() {
            return;
        }
        if !row.effects_settled {
            settle_prior_effects(d, task, out);
            return;
        }
        if !row.releasing {
            return;
        }
        for (_, pending) in &d.entries {
            if pending.task == task {
                return;
            }
        }
        if let Some(number) = row.close_pull {
            let Some(change) = d.changes.get(&task) else {
                emit(out, Request::ReleaseFailed { task });
                return;
            };
            let repository = change.repository;
            let write = client::api::Write::Close { number };
            enqueue_release(d, env, &mut row, entry, repository, write, None, out);
            return;
        }
        let mut selected = None;
        for (name, hold) in &d.holds {
            if hold.task == task {
                selected = Some((name.clone(), hold.writer));
                break;
            }
        }
        if let Some((name, writer)) = selected {
            if writer.is_some() {
                emit(out, Request::ReleaseFailed { task });
                return;
            }
            if row.ending == ReleaseEnding::Failed && row.root != task {
                let hold = d.holds.get_mut(&name).expect("selected hold");
                hold.task = row.root;
                emit(out, Request::Save { record: Stored::Hold(hold.clone()) });
                emit(out, Request::Retained { task, root: row.root, resource: name });
                continue;
            }
            if let Some(resource) = to_client(&name, &env.limits) {
                match resource.what {
                    client::What::Branch(branch) => {
                        enqueue_release(
                            d,
                            env,
                            &mut row,
                            entry,
                            resource.repository,
                            client::api::Write::DeleteBranch { branch },
                            Some(name),
                            out,
                        );
                        return;
                    }
                    client::What::Repository | client::What::Pull(_) | client::What::Issue(_) => {}
                }
            }
            d.holds.remove(&name);
            emit(out, Request::Erase { key: Key::Hold(name) });
            continue;
        }
        let mut topics = List::with_capacity(env.limits.subscriptions);
        for ((owner, topic), _) in &d.subscriptions {
            if *owner == task {
                topics.push(topic.clone()).expect("subscription capacity");
            }
        }
        for topic in &topics {
            unsubscribe(d, task, topic.clone(), out);
        }
        unnamed(d, env, task, out);
        d.releases.remove(&task);
        emit(out, Request::Erase { key: Key::Release(task) });
        emit(out, Request::Released { task });
        return;
    }
    unreachable!("every release pass removes a hold or completes");
}

#[expect(clippy::too_many_arguments, reason = "one cleanup effect carries its durable entry and held resource")]
fn enqueue_release(
    d: &mut Domain,
    env: &Env<Limits>,
    row: &mut ReleaseRow,
    number: u64,
    repository: client::api::Repository,
    write: client::api::Write,
    resource: Option<Name>,
    out: &mut Queue<Request>,
) {
    if d.entries.len() == env.limits.entries || d.entries.contains_key(&number) {
        emit(out, Request::ReleaseFailed { task: row.task });
        return;
    }
    let entry = client::Entry {
        number,
        task: row.task,
        repository,
        effect: client::Effect { write, condition: client::Condition::None },
        start: None,
        attempt: None,
        failures: 0,
    };
    row.pending = Some((number, resource));
    d.releases.insert(row.task, row.clone()).expect("replaces release");
    emit(out, Request::Save { record: Stored::Release(row.clone()) });
    d.entries.insert(number, entry.clone()).expect("entry capacity checked");
    emit(out, Request::Save { record: Stored::Entry(entry) });
}

fn release_outcome(d: &mut Domain, entry: u64, outcome: client::Outcome, out: &mut Queue<Request>) {
    if outcome == client::Outcome::Uncertain {
        return;
    }
    let mut owner = None;
    for (task, row) in &d.releases {
        if match &row.pending {
            Some((number, _)) => *number == entry,
            None => false,
        } {
            owner = Some(*task);
            break;
        }
    }
    let Some(task) = owner else { return };
    let mut row = d.releases.get(&task).expect("selected release").clone();
    let (_, resource) = row.pending.take().expect("selected pending release");
    match outcome {
        client::Outcome::Made { .. } => match resource {
            Some(name) => {
                d.holds.remove(&name);
                emit(out, Request::Erase { key: Key::Hold(name) });
            }
            None => row.close_pull = None,
        },
        client::Outcome::Failed(_)
        | client::Outcome::Raced { .. }
        | client::Outcome::Held
        | client::Outcome::Withdrawn => {
            row.failed = true;
            emit(out, Request::ReleaseFailed { task });
        }
        client::Outcome::Uncertain => unreachable!("uncertain returned above"),
    }
    d.releases.insert(task, row.clone()).expect("replaces release");
    emit(out, Request::Save { record: Stored::Release(row) });
}

fn project(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    repository: client::api::Repository,
    view: issues::GoalView,
    out: &mut Queue<Request>,
) {
    let goal = view.goal;
    let Some(adopted) = d.repositories.get(&repository) else {
        emit(out, Request::ProjectionFailed { goal });
        return;
    };
    if view.repository != u64::from(repository.repository)
        || adopted.role == Role::Context
        || !adopted.kinds.issue
        || (!d.issues.contains_key(&goal) && d.issues.len() == env.limits.issues)
    {
        emit(out, Request::ProjectionFailed { goal });
        return;
    }
    let prior = match d.issues.get(&goal) {
        Some(row) if row.repository != repository || row.failed => {
            emit(out, Request::ProjectionFailed { goal });
            return;
        }
        Some(row) if row.pending.is_some() => {
            let mut row = row.clone();
            row.desired = Some(view);
            d.issues.insert(goal, row.clone()).expect("replaces issue");
            emit(out, Request::Save { record: Stored::Issue(row) });
            emit(out, Request::ProjectAfter { goal, when: None });
            return;
        }
        Some(row) => row.clone(),
        None => IssueRow {
            goal,
            repository,
            state: issues::Projected::default(),
            number: None,
            pending: None,
            before: None,
            failed: false,
            desired: None,
        },
    };
    let projected = issues::project(&view, &prior.state, env.wall, &env.limits.issue_policy);
    match projected.decision {
        issues::Decision::None | issues::Decision::Wait(_) => {
            if prior.desired.as_ref() != Some(&view) {
                let mut row = prior;
                row.desired = Some(view);
                d.issues.insert(goal, row.clone()).expect("replaces issue");
                emit(out, Request::Save { record: Stored::Issue(row) });
            }
            match projected.decision {
                issues::Decision::Wait(when) => emit(out, Request::ProjectAfter { goal, when: Some(when) }),
                issues::Decision::None => {}
                issues::Decision::Hold | issues::Decision::Effect(_) => unreachable!("matched above"),
            }
        }
        issues::Decision::Hold => emit(out, Request::ProjectionFailed { goal }),
        issues::Decision::Effect(effect) => {
            if d.entries.len() == env.limits.entries || d.entries.contains_key(&number) {
                emit(out, Request::ProjectAfter { goal, when: None });
                return;
            }
            let Some(write) = project_write(d, &prior, effect, &env.limits) else {
                emit(out, Request::ProjectionFailed { goal });
                return;
            };
            let entry = client::Entry {
                number,
                task: goal,
                repository,
                effect: client::Effect { write, condition: client::Condition::None },
                start: None,
                attempt: None,
                failures: 0,
            };
            let row = IssueRow {
                goal,
                repository,
                state: projected.projected,
                number: prior.number,
                pending: Some(number),
                before: Some(prior.state),
                failed: false,
                desired: Some(view),
            };
            d.issues.insert(goal, row.clone()).expect("projection capacity checked");
            emit(out, Request::Save { record: Stored::Issue(row) });
            d.entries.insert(number, entry.clone()).expect("outbox capacity checked");
            emit(out, Request::Save { record: Stored::Entry(entry) });
        }
    }
}

fn project_write(d: &Domain, row: &IssueRow, effect: issues::Effect, l: &Limits) -> Option<client::api::Write> {
    match effect {
        issues::Effect::Open { goal, key, title, body, .. } => {
            let key = projection_key(&d.namespace, goal, key, l)?;
            Some(client::api::Write::CreateIssue { key, title: Box::from(title.as_bytes()), body })
        }
        issues::Effect::Body { body, .. } => {
            Some(client::api::Write::Edit { number: row.number?, title: None, body: Some(body) })
        }
        issues::Effect::Comment { goal, key, body, .. } => {
            let key = projection_key(&d.namespace, goal, key, l)?;
            Some(client::api::Write::Post { number: row.number?, key, body: Box::from(body.as_bytes()) })
        }
        issues::Effect::Close { .. } => Some(client::api::Write::Close { number: row.number? }),
    }
}

fn projection_key(namespace: &[u8], goal: u64, key: issues::Key, l: &Limits) -> Option<Box<[u8]>> {
    let (part, number) = match key {
        issues::Key::Open => (1, 0),
        issues::Key::Body(revision) => (2, revision),
        issues::Key::Milestone(milestone) => match milestone {
            issues::MilestoneKey::PlanAccepted => (3, 0),
            issues::MilestoneKey::Revision(number) => (4, number),
            issues::MilestoneKey::ChangeLanded(number) => (5, number),
            issues::MilestoneKey::ChangeHeld(number) => (6, number),
            issues::MilestoneKey::Report(number) => (7, number),
            issues::MilestoneKey::Finished => (8, 0),
        },
        issues::Key::Close => (9, 0),
    };
    client::effect_key(namespace, &client::EffectPurpose::Projection { goal, part, number }, l.client.op_bytes)
}

fn projection_outcome(d: &mut Domain, entry: u64, outcome: client::Outcome, out: &mut Queue<Request>) {
    let mut goal = None;
    for (number, row) in &d.issues {
        if row.pending == Some(entry) {
            goal = Some(*number);
        }
    }
    let Some(goal) = goal else { return };
    let Some(row) = d.issues.get_mut(&goal) else { unreachable!("found issue row") };
    match outcome {
        client::Outcome::Uncertain => return,
        client::Outcome::Made { made, .. } => {
            if row.number.is_none() {
                match made {
                    client::Made::Created(number) => row.number = Some(number),
                    client::Made::Commented(_)
                    | client::Made::Reviewed(_)
                    | client::Made::Merged(_)
                    | client::Made::Updated(_)
                    | client::Made::Branch(_)
                    | client::Made::Set => row.failed = true,
                }
            }
        }
        client::Outcome::Failed(_)
        | client::Outcome::Raced { .. }
        | client::Outcome::Held
        | client::Outcome::Withdrawn => {
            if let Some(before) = row.before.take() {
                row.state = before;
            }
            row.failed = true;
        }
    }
    row.pending = None;
    row.before = None;
    let saved = row.clone();
    emit(out, Request::Save { record: Stored::Issue(saved) });
    if row.failed {
        emit(out, Request::ProjectionFailed { goal });
    } else {
        emit(out, Request::ProjectAfter { goal, when: None });
    }
}

fn release_projection(d: &mut Domain, goal: u64, out: &mut Queue<Request>) {
    let Some(row) = d.issues.get_mut(&goal) else { return };
    if row.pending.is_some() {
        return;
    }
    row.failed = false;
    let saved = row.clone();
    emit(out, Request::Save { record: Stored::Issue(saved) });
    emit(out, Request::ProjectAfter { goal, when: None });
}

fn committed(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    if let Some(entry) = d.entries.get(&number) {
        child(d, env, client::Event::Make { entry: entry.clone() }, out);
    }
}

fn withdraw(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    if d.entries.contains_key(&number) {
        child(d, env, client::Event::Withdraw { entry: number }, out);
    }
}

fn veto_change(d: &mut Domain, task: u64, entry: u64, prior: change::Change, out: &mut Queue<Request>) {
    let Some(row) = d.changes.get_mut(&task) else { return };
    if row.pending != Some(entry) || !d.entries.contains_key(&entry) {
        return;
    }
    row.change = prior;
    row.pending = None;
    row.effect = change::EffectResult::None;
    emit(out, Request::Save { record: Stored::Change(row.clone()) });
    d.entries.remove(&entry);
    emit(out, Request::Erase { key: Key::Entry(entry) });
}

fn subscribe(d: &mut Domain, env: &Env<Limits>, subscription: Subscriber, out: &mut Queue<Request>) {
    let task = subscription.task;
    let ci = match &subscription.topic {
        Topic::Ci { repository, head } => Some((*repository, *head)),
        Topic::Landings { .. } | Topic::Pull { .. } | Topic::Participation { .. } => None,
    };
    if !d.can_subscribe(&env.limits, &subscription) {
        emit(out, Request::Refused { task });
        return;
    }
    let key = (task, subscription.topic.clone());
    d.subscriptions.insert(key, subscription.clone()).expect("preflighted subscription capacity");
    emit(out, Request::Save { record: Stored::Subscription(subscription) });
    if let Some((repository, head)) = ci {
        ci_refresh(d, env, repository, head, out);
    }
}

fn unsubscribe(d: &mut Domain, task: u64, topic: Topic, out: &mut Queue<Request>) {
    if d.subscriptions.remove(&(task, topic.clone())).is_some() {
        emit(out, Request::Erase { key: Key::Subscription { task, topic: topic.clone() } });
        match topic {
            Topic::Ci { repository, head } => {
                let mut named = false;
                for (_, subscriber) in &d.subscriptions {
                    if subscriber.topic == (Topic::Ci { repository, head }) {
                        named = true;
                    }
                }
                if !named && d.ci.remove(&(repository, head)).is_some() {
                    emit(out, Request::Erase { key: Key::Ci { repository, head } });
                }
            }
            Topic::Landings { .. } | Topic::Pull { .. } | Topic::Participation { .. } => {}
        }
    }
}

fn ci_refresh(
    d: &mut Domain,
    env: &Env<Limits>,
    repository: client::api::Repository,
    head: client::api::Commit,
    out: &mut Queue<Request>,
) {
    for (_, pending) in &d.pending_ci {
        if pending.repository == repository && pending.head == head {
            return;
        }
    }
    if d.pending_ci.len() == env.limits.subscriptions {
        return;
    }
    let Some(sequence) = d.sequence.checked_add(1) else { return };
    d.sequence = sequence;
    let owner = Token::new(sequence | (1_u64 << 60));
    d.pending_ci.insert(owner, PendingCi { repository, head }).expect("CI read capacity checked");
    child(
        d,
        env,
        client::Event::Read { owner, repository, read: client::api::Read::Statuses { commit: head, page: 1 } },
        out,
    );
}

fn ci_hint(d: &mut Domain, env: &Env<Limits>, hint: &client::api::Hint, out: &mut Queue<Request>) {
    let head = match &hint.change {
        client::api::Change::Commit(head) => *head,
        client::api::Change::Item(_) | client::api::Change::Branch(_) => return,
    };
    let mut subscribed = false;
    for (_, subscriber) in &d.subscriptions {
        match &subscriber.topic {
            Topic::Ci { repository, head: watched } if *repository == hint.repository && *watched == head => {
                subscribed = true;
                break;
            }
            Topic::Ci { .. } | Topic::Landings { .. } | Topic::Pull { .. } | Topic::Participation { .. } => {}
        }
    }
    if subscribed {
        ci_refresh(d, env, hint.repository, head, out);
    }
}

fn ci_read(
    d: &mut Domain,
    owner: Token,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(pending) = d.pending_ci.remove(&owner) else { return };
    if let Ok(client::api::Answer::Statuses { ci, .. }) = result {
        ci_observed(d, pending.repository, pending.head, ci, out);
    }
}

fn ci_observed(
    d: &mut Domain,
    repository: client::api::Repository,
    head: client::api::Commit,
    status: client::api::Ci,
    out: &mut Queue<Request>,
) {
    let topic = Topic::Ci { repository, head };
    let mut named = false;
    for (_, subscriber) in &d.subscriptions {
        if subscriber.topic == topic {
            named = true;
        }
    }
    let same = match d.ci.get(&(repository, head)) {
        Some(prior) => prior.status == status,
        None => false,
    };
    if !named || same {
        return;
    }
    if d.ci.len() == d.ci.capacity() && !d.ci.contains_key(&(repository, head)) {
        return;
    }
    let row = CiState { repository, head, status };
    d.ci.insert((repository, head), row.clone()).expect("CI subscription capacity");
    emit(out, Request::Save { record: Stored::Ci(row) });
    publish(d, topic, News::Ci { head, status }, out);
}

fn changed(
    d: &mut Domain,
    env: &Env<Limits>,
    resource: client::Resource,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    match result {
        Ok(client::api::Answer::Commit(commit)) => branch_changed(d, env, resource, commit, out),
        Ok(client::api::Answer::Pull(pull)) => pull_changed(d, env, resource, pull, out),
        Ok(client::api::Answer::Item { item, comments, more: _ }) => {
            let topic = Topic::Participation { repository: resource.repository, number: item.number };
            if !comments.is_empty() {
                publish(d, topic, News::Changed { number: item.number }, out);
            }
        }
        Ok(client::api::Answer::Reviews { reviews, more: _ }) => match resource.what {
            client::What::Pull(number) if !reviews.is_empty() => {
                let topic = Topic::Participation { repository: resource.repository, number };
                publish(d, topic, News::Changed { number }, out);
            }
            client::What::Pull(_) | client::What::Repository | client::What::Branch(_) | client::What::Issue(_) => {}
        },
        Ok(
            client::api::Answer::Items { .. }
            | client::api::Answer::Branches(_)
            | client::api::Answer::Statuses { .. }
            | client::api::Answer::Remarks { .. }
            | client::api::Answer::PullFiles { .. }
            | client::api::Answer::Compare { .. }
            | client::api::Answer::Checks(_)
            | client::api::Answer::Job { .. }
            | client::api::Answer::Protection(_)
            | client::api::Answer::Settings(_)
            | client::api::Answer::Collaborators { .. }
            | client::api::Answer::Permission(_)
            | client::api::Answer::Created(_)
            | client::api::Answer::Commented(_)
            | client::api::Answer::Reviewed(_)
            | client::api::Answer::Merged(_)
            | client::api::Answer::Branch(_)
            | client::api::Answer::Done,
        )
        | Err(_) => {}
    }
}

fn from_client(resource: &client::Resource, l: &Limits) -> Option<Name> {
    let what = match &resource.what {
        client::What::Repository => What::Repository,
        client::What::Pull(number) => What::Pull(*number),
        client::What::Issue(number) => What::Issue(*number),
        client::What::Branch(branch) => {
            if branch.is_empty() || branch.len() > usize::try_from(l.name_bytes).ok()? {
                return None;
            }
            let mut parts = List::with_capacity(l.name_bytes);
            let mut start = 0usize;
            for (index, byte) in branch.iter().enumerate() {
                if *byte == b'/' {
                    if index == start {
                        return None;
                    }
                    parts.push(Box::from(branch.get(start..index)?)).ok()?;
                    start = index.checked_add(1)?;
                }
            }
            if start == branch.len() {
                return None;
            }
            parts.push(Box::from(branch.get(start..)?)).ok()?;
            What::Branch(parts.into_boxed())
        }
    };
    Some(Name { forge: resource.repository.forge, repository: resource.repository.repository, what })
}

#[expect(clippy::manual_map, reason = "step code has no closures")]
fn branch_changed(
    d: &mut Domain,
    env: &Env<Limits>,
    resource: client::Resource,
    commit: client::api::Commit,
    out: &mut Queue<Request>,
) {
    let Some(name) = from_client(&resource, &env.limits) else { return };
    let Some(branch) = (match &resource.what {
        client::What::Branch(branch) => Some(branch.clone()),
        client::What::Repository | client::What::Pull(_) | client::What::Issue(_) => None,
    }) else {
        return;
    };
    let previous = match d.heads.get(&name) {
        Some(row) => Some(row.commit),
        None => None,
    };
    let row = BranchHead { name: name.clone(), commit };
    if d.heads.insert(name, row.clone()).is_err() {
        return;
    }
    emit(out, Request::Save { record: Stored::BranchHead(row) });
    let Some(before) = previous else { return };
    if before == commit {
        return;
    }
    let topic = Topic::Landings { repository: resource.repository, branch };
    if d.landings.len() >= env.limits.landings {
        publish(d, topic, News::Landing { before, after: commit, files: None }, out);
        return;
    }
    let Some(sequence) = d.sequence.checked_add(1) else {
        publish(d, topic, News::Landing { before, after: commit, files: None }, out);
        return;
    };
    d.sequence = sequence;
    let owner = Token::new(sequence | (1_u64 << 63));
    d.landings.insert(owner, PendingLanding { topic, before, after: commit }).expect("landing capacity preflighted");
    child(
        d,
        env,
        client::Event::Read {
            owner,
            repository: resource.repository,
            read: client::api::Read::Compare { before, after: commit },
        },
        out,
    );
}

fn landing_read(
    d: &mut Domain,
    owner: Token,
    result: Result<client::api::Answer, client::api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(pending) = d.landings.remove(&owner) else { return };
    let files = match result {
        Ok(client::api::Answer::Compare { before, after, files, commits: _, contains_before: _ })
            if before == pending.before && after == pending.after =>
        {
            let mut names = List::with_capacity(u32::try_from(files.len()).unwrap_or(u32::MAX));
            for file in files {
                names.push(file.path).expect("sized from bounded comparison");
            }
            Some(names.into_boxed())
        }
        Ok(_) | Err(_) => None,
    };
    publish(d, pending.topic, News::Landing { before: pending.before, after: pending.after, files }, out);
}

fn pull_changed(
    d: &mut Domain,
    env: &Env<Limits>,
    resource: client::Resource,
    pull: client::api::Pull,
    out: &mut Queue<Request>,
) {
    let Some(name) = from_client(&resource, &env.limits) else { return };
    let prior = d.pulls.get(&name).cloned();
    let state = PullState { name: name.clone(), head: pull.commit, ci: pull.ci, state: pull.state };
    if d.pulls.insert(name, state.clone()).is_err() {
        return;
    }
    emit(out, Request::Save { record: Stored::PullState(state) });
    let topic = Topic::Pull { repository: resource.repository, number: pull.number };
    let changed_pull = match &prior {
        Some(row) => row.head != pull.commit || row.state != pull.state,
        None => true,
    };
    if changed_pull {
        publish(d, topic, News::Changed { number: pull.number }, out);
    }
    let changed_ci = match &prior {
        Some(row) => row.head != pull.commit || row.ci != pull.ci,
        None => true,
    };
    if changed_ci {
        ci_observed(d, resource.repository, pull.commit, pull.ci, out);
    }
}

fn overlap(files: &[Box<[u8]>], paths: &[Box<[u8]>]) -> bool {
    for file in files {
        for path in paths {
            if file.as_ref() == path.as_ref() {
                return true;
            }
            if file.starts_with(path) && file.get(path.len()) == Some(&b'/') {
                return true;
            }
        }
    }
    false
}

fn classify(d: &Domain, subscription: &Subscriber, news: &News) -> Class {
    match news {
        News::Landing { after, files, .. } => {
            if let Some(owner) = d.landed.get(after)
                && subscription.own_change == Some(*owner)
            {
                return Class::Kept;
            }
            match files {
                None => Class::Wakes,
                Some(files) if overlap(files, &subscription.paths) => Class::Wakes,
                Some(_) => Class::Kept,
            }
        }
        News::Ci { .. } | News::Changed { .. } => Class::Wakes,
    }
}

fn publish(d: &Domain, topic: Topic, news: News, out: &mut Queue<Request>) {
    for (_, subscriber) in &d.subscriptions {
        if subscriber.topic == topic {
            let class = classify(d, subscriber, &news);
            emit(
                out,
                Request::News {
                    task: subscriber.task,
                    subscription: subscriber.number,
                    topic: topic.clone(),
                    news: news.clone(),
                    class,
                },
            );
        }
    }
}

#[expect(
    clippy::match_like_matches_macro,
    clippy::wildcard_enum_match_arm,
    reason = "the outbox test uses explicit matches in this strict subset"
)]
fn updating_entry(d: &Domain, task: u64) -> bool {
    let Some(change) = d.changes.get(&task) else { return false };
    if !match change.change.state {
        change::State::Updating { .. } => true,
        _ => false,
    } {
        return false;
    }
    let Some(number) = change.pending else { return false };
    let Some(entry) = d.entries.get(&number) else { return false };
    match entry.effect.write {
        client::api::Write::Update { .. } => true,
        _ => false,
    }
}

fn drift(d: &mut Domain, env: &Env<Limits>, resource: client::Resource, out: &mut Queue<Request>) {
    let mut affected = List::with_capacity(env.limits.holds);
    for (name, row) in &d.holds {
        if to_client(name, &env.limits) == Some(resource.clone()) {
            if updating_entry(d, row.task) {
                continue;
            }
            emit(out, Request::Drift { task: row.task, resource: name.clone() });
            affected.push(row.task).expect("one affected task per hold");
        }
    }
    for task in &affected {
        if let Some(row) = d.changes.get_mut(task) {
            row.drift = Some(change::Hold::BranchMoved);
            emit(out, Request::Save { record: Stored::Change(row.clone()) });
        }
    }
}

fn restore(d: &mut Domain, env: &Env<Limits>, record: Stored) {
    match record {
        Stored::Repository(row) => {
            d.repositories.insert(row.provider, row).expect("restored repository within limits");
        }
        Stored::Hold(row) => {
            d.holds.insert(row.name.clone(), row).expect("restored hold within limits");
        }
        Stored::Names { task, resources } => {
            d.names.insert(task, resources).expect("restored names within limits");
        }
        Stored::Subscription(row) => {
            d.subscriptions.insert((row.task, row.topic.clone()), row).expect("restored subscription within limits");
        }
        Stored::BranchHead(row) => {
            d.heads.insert(row.name.clone(), row).expect("restored branch head within limits");
        }
        Stored::PullState(row) => {
            d.pulls.insert(row.name.clone(), row).expect("restored pull within limits");
        }
        Stored::Ci(row) => {
            d.ci.insert((row.repository, row.head), row).expect("restored CI within limits");
        }
        Stored::Landed { commit, task } => {
            d.landed.insert(commit, task).expect("restored landing within limits");
        }
        Stored::Entry(row) => {
            d.entries.insert(row.number, row).expect("restored entry within limits");
        }
        Stored::Client(row) => {
            let mut discarded = Queue::with_capacity(env.limits.output);
            child(d, env, client::Event::Restore { record: row }, &mut discarded);
            assert!(discarded.is_empty(), "restoration emits no output");
        }
        Stored::Change(row) => {
            d.changes.insert(row.task, row).expect("restored change within limits");
        }
        Stored::Issue(row) => {
            d.issues.insert(row.goal, row).expect("restored issue within limits");
        }
        Stored::Release(row) => {
            d.releases.insert(row.task, row).expect("restored release within limits");
        }
    }
}

fn restored(d: &mut Domain, env: &Env<Limits>, clock: client::RecoveryClock, out: &mut Queue<Request>) {
    child(d, env, client::Event::Restored { clock }, out);
    child(d, env, client::Event::PauseOutbox, out);
    let mut entries = List::with_capacity(env.limits.entries);
    for (_, entry) in &d.entries {
        entries.push(entry.clone()).expect("entry list capacity");
    }
    for entry in &entries {
        child(d, env, client::Event::Make { entry: entry.clone() }, out);
    }
    let mut ci = List::with_capacity(env.limits.subscriptions);
    for (_, subscriber) in &d.subscriptions {
        match &subscriber.topic {
            Topic::Ci { repository, head } if !ci.as_slice().contains(&(*repository, *head)) => {
                ci.push((*repository, *head)).expect("subscription capacity");
            }
            Topic::Ci { .. } | Topic::Landings { .. } | Topic::Pull { .. } | Topic::Participation { .. } => {}
        }
    }
    for (repository, head) in &ci {
        ci_refresh(d, env, *repository, *head, out);
    }
    emit(out, Request::RestartDone { stage: RestartStage::Restored });
}
