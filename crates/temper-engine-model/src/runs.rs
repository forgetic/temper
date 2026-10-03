//! Runs (engine-model.md, sections 4.2, 8 and 9): starting the run the hub
//! claimed, the fleet's placements and answers, the inbox events relayed to
//! a live run, its calls served, and its reports to the views.
//!
//! - **Starting:** the rules check the run (its budget against what was
//!   spent, its grants); the brief renders the sections the plan selected;
//!   the store gives the snapshot to resume if the plan says so and there
//!   is one; the charter and the workspace are composed, and the fleet is
//!   asked to place the attempt. Until the cold start is done, a run
//!   prepared waits: nothing new starts before every claim is adopted. A run
//!   the rules refuse, or whose brief cannot be rendered, is answered at
//!   once, as failed; one stopped while it is prepared, as refused.
//! - **Answers:** the fleet hands each answer on once; the inbox position
//!   moves with what the run took, then the hub hears it, and the fleet's
//!   acknowledgement goes once the hub has it durably or calls it stale.
//! - **Calls:** a forge read (if the run's grants allow), a `recall`, a
//!   `note` (if the rules allow), a comment and an escalation, each
//!   answered once, through the fleet.

use alloc::boxed::Box;
use core::mem;

use temper_engine_model_brief as brief;
use temper_engine_model_fleet as fleet;
use temper_engine_model_forge::{self as forge, api};
use temper_engine_model_plan as plan;
use temper_engine_model_rules as rules;
use temper_engine_model_views as views;
use temper_engine_model_work as work;
use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Queue, ReplyTo, Token};

use crate::boundary::{
    Answer, Assignment, Call, Charter, Checkout, Inbound, Item, Posted, Request, Served, Start, Unserved, Workspace,
};
use crate::facts::Fact;
use crate::items::{self, Entry, Job, Live, Seen, Starting};
use crate::jobs::{get, get_mut};
use crate::limits::Limits;
use crate::model::{self, Model};
use crate::route;
use crate::translate;
use crate::waits::{Carried, Relayed, Wait};

/// The hub starts the item's attempt `attempt`, on the run it was told is
/// due, its claim written.
pub(crate) fn start(model: &mut Model, env: &Env<Limits>, item: Item, attempt: u64, run: Token) {
    let Some(id) = items::find(model, item) else { unreachable!("the hub starts only items the top level holds") };
    assert!(run == id.token(), "a run is the one decided for its item");
    let entry = get_mut(model, id);
    let Some(due) = entry.due.take() else { unreachable!("the hub starts the run it was told is due") };
    let start = entry.next.saturating_sub(1);
    entry.live = Some(Live { attempt, start, started: false, bounced: false, comments: None });
    entry.grants = Some(due.grants);
    entry.resumed = None;
    // The rules were asked as the run was decided; what was spent since may
    // change their answer: then nothing runs, and the hub claims again,
    // which asks them again.
    match rule(model, env, id, &due) {
        rules::Decision::Allow => {}
        rules::Decision::Wait | rules::Decision::Accept { .. } | rules::Decision::Refuse => {
            return end(model, env, id, attempt, work::Answer::Refused);
        }
    }
    let sections = sections(model, env, id, &due);
    let Ok(rendering) = model.waits.insert(Wait::Job { entry: id }) else {
        unreachable!("the waits have room for every item's job")
    };
    let fetching = if due.resume && get(model, id).relations.snapshot {
        let Ok(fetching) = model.waits.insert(Wait::Job { entry: id }) else {
            unreachable!("the waits have room for every item's job")
        };
        model.requests.push(Request::Store { owner: fetching.token(), op: crate::boundary::Store::Get { item } });
        Some(fetching)
    } else {
        None
    };
    // A change made again after an earlier attempt starts from that
    // attempt's push, if one landed whose answer never came (its worker lost
    // holding it): the item's branch is read on the forge first.
    let produces = match due.why {
        plan::Why::Produce => true,
        plan::Why::Work | plan::Why::Repair(_) | plan::Why::Review { .. } | plan::Why::Turn => false,
    };
    let branching = if produces && attempt > 1 && get(model, id).relations.branch.is_none() {
        let Ok(branching) = model.waits.insert(Wait::Job { entry: id }) else {
            unreachable!("the waits have room for every item's job")
        };
        Some(branching)
    } else {
        None
    };
    let starting =
        Starting { attempt, run: due, brief: None, rendering: Some(rendering), fetching, branching, snapshot: None };
    get_mut(model, id).job = Job::Starting(Box::new(starting));
    if let Some(branching) = branching {
        let branch = translate::branch(&model.config.branches, item);
        let read = forge::Read::Branch { repository: item.repository, branch };
        route::forge_step(model, env, forge::Event::Read { owner: branching.token(), read });
    }
    let reply_to = ReplyTo::new(rendering.token());
    route::brief_step(model, env, brief::Event::Render { reply_to, sections });
}

/// The forge answered the read of the item's branch, before its change is
/// made again: a push found there is the item's branch, and the run starts
/// from it.
pub(crate) fn branched(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    wait: Id<Wait>,
    result: Result<api::Answer, forge::Failure>,
) {
    let Some(entry) = model.items.get_mut(id) else { return };
    let Some(starting) = items::starting_mut(&mut entry.job) else { return };
    if starting.branching != Some(wait) {
        return;
    }
    starting.branching = None;
    let pushed = match result {
        Ok(api::Answer::Commit(commit)) => Some(commit),
        Ok(_) | Err(_) => None,
    };
    if let Some(commit) = pushed
        && entry.relations.branch.is_none()
    {
        entry.relations.branch = Some(commit);
        items::aside(model, env, id);
    }
    ready(model, env, id);
}

/// The rules on a run: its budget against what was spent, by the item's
/// goal and by the deployment, and what it may read and push.
pub(crate) fn rule(model: &Model, env: &Env<Limits>, id: Id<Entry>, run: &plan::Run) -> rules::Decision {
    let entry = get(model, id);
    let goal = match entry.relations.goal {
        Some(goal) => match items::find(model, goal) {
            Some(goal) => rules::Goal::Spent(get(model, goal).relations.spent),
            None => rules::Goal::Spent(0),
        },
        None => rules::Goal::Outside,
    };
    let branch = translate::branch(&model.config.branches, entry.item);
    let checked = rules::Run {
        budget: run.budget.tokens,
        goal,
        deployment_spent: model.spent,
        grants: translate::grants(run.grants, entry.item.repository, &branch),
    };
    let gates = match entry.step.as_ref() {
        Some(record) => translate::gates(&record.step.gates),
        None => List::with_capacity(0),
    };
    let mut findings = Queue::with_capacity(rules::max_out(&env.limits.rules));
    rules::check_run(
        &model.config.rules,
        &env.limits.rules,
        &checked,
        items::accepted(entry, None),
        gates.as_slice(),
        &mut findings,
    )
}

/// The sections a run's brief carries, in the brief's terms: the item is
/// required; a repair's failing CI and a rebase's pull request too, since a
/// run without them would spend itself on nothing.
fn sections(model: &Model, env: &Env<Limits>, id: Id<Entry>, run: &plan::Run) -> Box<[brief::Wanted]> {
    let entry = get(model, id);
    let item = translate::brief_item(entry.item);
    let chosen = run.sections;
    let head = head_of(model, entry.item);
    let repair = match run.why {
        plan::Why::Repair(repair) => Some(repair),
        plan::Why::Work | plan::Why::Produce | plan::Why::Review { .. } | plan::Why::Turn => None,
    };
    let mut wanted = List::with_capacity(env.limits.brief.sections);
    want(&mut wanted, brief::Source::Item(item), true);
    // People's messages waiting are what the run is for, and what its
    // answer takes: without them, it does not run.
    if chosen.comments {
        let messages = waiting_messages(entry);
        want(&mut wanted, brief::Source::Comments { item, since: entry.since }, messages);
    }
    if chosen.dependencies && !entry.relations.dependencies.is_empty() {
        let mut items = List::with_capacity(env.limits.brief.items);
        for dependency in &entry.relations.dependencies {
            if items.push(translate::brief_item(dependency.item)).is_err() {
                break;
            }
        }
        want(&mut wanted, brief::Source::Dependencies(items.into_boxed()), false);
    }
    if let Some(head) = head {
        if chosen.ci {
            let required = repair == Some(plan::Repair::CiFailed);
            want(&mut wanted, brief::Source::Ci { item, head }, required);
        }
        if chosen.reviews {
            let required = repair == Some(plan::Repair::ChangesRequested);
            want(&mut wanted, brief::Source::Reviews { item, head }, required);
        }
        if chosen.pull {
            let required = matches_rebase(repair);
            want(&mut wanted, brief::Source::Pull { item, head }, required);
        }
    }
    if chosen.attempts {
        want(&mut wanted, brief::Source::Attempts(item), false);
    }
    if chosen.plan {
        let goal = match entry.relations.goal {
            Some(goal) => Some(goal),
            None => match entry.step.as_ref() {
                Some(record) if record.goal.is_some() => Some(entry.item),
                Some(_) | None => None,
            },
        };
        if let Some(goal) = goal {
            want(&mut wanted, brief::Source::Plan { goal: translate::brief_item(goal) }, false);
        }
    }
    if chosen.notes {
        let goal = goal_item(entry.relations.goal);
        want(&mut wanted, brief::Source::Notes { repository: entry.item.repository, goal }, false);
    }
    if chosen.template
        && let Some(template) = run.template
    {
        want(&mut wanted, brief::Source::Template(template), false);
    }
    wanted.into_boxed()
}

/// Whether people's messages wait in the item's inbox.
fn waiting_messages(entry: &Entry) -> bool {
    for (_, noted) in &entry.inbox {
        if noted.source == plan::Source::Message {
            return true;
        }
    }
    false
}

/// The head of the item's pull request, as the working set holds it.
fn head_of(model: &Model, item: Item) -> Option<brief::Commit> {
    let level = model.forge.pull(translate::forge_item(item))?;
    Some(brief::Commit(level.commit))
}

fn goal_item(goal: Option<Item>) -> Option<brief::Item> {
    let goal = goal?;
    Some(translate::brief_item(goal))
}

fn want(wanted: &mut List<brief::Wanted>, source: brief::Source, required: bool) {
    // A brief has room for every section the plan may select.
    wanted.push(brief::Wanted { source, required }).expect("room for each of them");
}

const fn matches_rebase(repair: Option<plan::Repair>) -> bool {
    match repair {
        Some(repair) => match repair {
            plan::Repair::BaseMoved | plan::Repair::Conflicts => true,
            plan::Repair::CiFailed | plan::Repair::ChangesRequested => false,
        },
        None => false,
    }
}

/// The brief answered: its sections, or `None` if it failed or was refused.
pub(crate) fn rendered(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    sections: Result<Box<[brief::Section]>, work::Answer>,
) {
    let token = reply_to.into_token();
    let Some(answered) = crate::serve::take(model, token) else { return };
    let Some(id) = answered.job() else { return };
    let Some(entry) = model.items.get_mut(id) else { return };
    let Some(starting) = items::starting_mut(&mut entry.job) else { return };
    if starting.rendering != Some(Id::from_token(token)) {
        return;
    }
    starting.rendering = None;
    let attempt = starting.attempt;
    match sections {
        Ok(sections) => {
            starting.brief = Some(sections);
            ready(model, env, id);
        }
        Err(answer) => {
            entry.job = Job::Idle;
            end(model, env, id, attempt, answer);
        }
    }
}

/// The store answered the get of the snapshot to resume.
pub(crate) fn fetched(
    model: &mut Model,
    env: &Env<Limits>,
    id: Id<Entry>,
    wait: Id<Wait>,
    snapshot: Option<Box<[u8]>>,
) {
    let Some(entry) = model.items.get_mut(id) else { return };
    let Some(starting) = items::starting_mut(&mut entry.job) else { return };
    if starting.fetching != Some(wait) {
        return;
    }
    starting.fetching = None;
    starting.snapshot = snapshot;
    ready(model, env, id);
}

/// A run prepared, once its brief and its snapshot are in: its assignment
/// is composed, and the fleet places it, unless the cold start is not done.
fn ready(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = get_mut(model, id);
    let Some(starting) = items::starting_mut(&mut entry.job) else { return };
    if starting.rendering.is_some() || starting.fetching.is_some() || starting.branching.is_some() {
        return;
    }
    let starting = match mem::replace(&mut entry.job, Job::Idle) {
        Job::Starting(starting) => starting,
        Job::Idle | Job::Asking { .. } | Job::Writing { .. } | Job::Recording { .. } | Job::Applying(_) => {
            unreachable!("an item starting is ready")
        }
    };
    let Starting { attempt, run, brief, snapshot, .. } = *starting;
    let brief = match brief {
        Some(brief) => brief,
        None => Box::new([]),
    };
    let assignment = assignment(model, id, attempt, &run, brief, snapshot);
    get_mut(model, id).assignment = Some(Box::new(assignment));
    if model.loaded.is_some() {
        place(model, env, id);
    } else if !get(model, id).waiting {
        get_mut(model, id).waiting = true;
        if model.held.try_push(id).is_err() {
            unreachable!("the held runs have room for every item");
        }
    }
}

/// The assignment of the item's attempt (worker-model.md, 4.1).
fn assignment(
    model: &Model,
    id: Id<Entry>,
    attempt: u64,
    run: &plan::Run,
    brief: Box<[brief::Section]>,
    snapshot: Option<Box<[u8]>>,
) -> Assignment {
    let entry = get(model, id);
    let item = entry.item;
    let branch = translate::branch(&model.config.branches, item);
    let start = match entry.relations.branch {
        Some(_) => Start::Branch { branch: copy_of(&branch) },
        None => Start::Base { branch: base(model, entry) },
    };
    let push = if run.grants.modify { Some(copy_of(&branch)) } else { None };
    let save = if run.grants.modify { Some(translate::branch(&model.config.saved, item)) } else { None };
    let checkout = Checkout { repository: item.repository, start, push };
    let charter = Charter {
        why: run.why,
        brief,
        instructions: copy_of(&run.instructions),
        grants: run.grants,
        finish: run.finish,
        budget: run.budget,
        models: copy_of(&model.config.models),
        policy: model.config.policy,
    };
    Assignment {
        item,
        attempt,
        workspace: Workspace { key: translate::workstream(item), repositories: Box::new([checkout]) },
        save,
        charter,
        snapshot,
    }
}

/// Where a run's checkout starts that has no branch of its own yet: its
/// change's base, or its repository's first base.
fn base(model: &Model, entry: &Entry) -> Box<[u8]> {
    if let Some(record) = entry.step.as_ref() {
        match &record.step.work {
            plan::Work::Change(change) => return copy_of(&change.base),
            plan::Work::Agent(_) | plan::Work::Wait(_) | plan::Work::Session(_) => {}
        }
    }
    let repo = model.config.plan.repo(plan::Repository(entry.item.repository));
    match repo {
        Some(repo) => match repo.bases.first() {
            Some(base) => copy_of(base),
            None => copy_of(b"main"),
        },
        None => copy_of(b"main"),
    }
}

/// Asks the fleet to place the item's attempt, and the views to follow it.
fn place(model: &mut Model, env: &Env<Limits>, id: Id<Entry>) {
    let entry = get_mut(model, id);
    let item = entry.item;
    let Some(live) = entry.live.as_mut() else { return };
    live.started = true;
    let attempt = live.attempt;
    let run = translate::run_of(item);
    let reply_to = ReplyTo::new(run);
    let workstream = translate::workstream(item);
    let started = views::Event::Started { run, attempt: Token::new(attempt), item: run, policy: model.config.policy };
    route::views_step(model, env, started);
    route::fleet_step(model, env, fleet::Event::Start { reply_to, run, attempt: Token::new(attempt), workstream });
}

/// The forge sub-model's cold read is done: every item it found is
/// announced, and taken into the hub, or did not fit.
pub(crate) fn read(model: &mut Model) {
    model.read = true;
}

/// Whether the cold start is done, and not yet told: the cold read is, and
/// every item it announced is in the hub, which asked the fleet to adopt
/// each claim it read, and the fleet has heard it.
pub(crate) fn is_loaded(model: &Model) -> bool {
    if model.loaded.is_some() || !model.read || !model.work_out.is_empty() {
        return false;
    }
    for (_, id) in &model.names {
        if let Some(entry) = model.items.get(*id)
            && entry.taking == items::Taking::Asked
        {
            return false;
        }
    }
    true
}

/// The cold start is done: every claim the records hold is adopted. The
/// fleet starts the strays' graces, and the runs prepared meanwhile start.
pub(crate) fn loaded(model: &mut Model, env: &Env<Limits>) {
    model.loaded = Some(env.now);
    model::keep(model, Fact::Loaded);
    route::fleet_step(model, env, fleet::Event::Loaded);
    for _ in 0..model.held.len() {
        let Some(id) = model.held.pop() else { break };
        let Some(entry) = model.items.get_mut(id) else { continue };
        entry.waiting = false;
        if entry.assignment.is_some() && entry.live.is_some() {
            place(model, env, id);
        }
    }
}

/// The hub adopts the item's attempt `attempt`, which its record claims.
pub(crate) fn adopt(model: &mut Model, env: &Env<Limits>, item: Item, attempt: u64) {
    let Some(id) = items::find(model, item) else { unreachable!("the hub adopts only items the top level holds") };
    let entry = get_mut(model, id);
    let start = entry.next.saturating_sub(1);
    // What it took before the restart is what its claim's record says: of
    // what this life's inbox holds, it takes nothing, and the next run has
    // it all again.
    entry.live = Some(Live { attempt, start, started: true, bounced: true, comments: None });
    // Its grants are what its claim gave it, which the record's step says;
    // what an earlier life made for it comes after its claim's position.
    entry.grants = match entry.step.as_ref() {
        Some(record) => translate::grants_of(record),
        None => None,
    };
    entry.resumed = Some(items::Resumed { attempt, since: entry.since });
    let run = translate::run_of(item);
    let reply_to = ReplyTo::new(run);
    route::fleet_step(model, env, fleet::Event::Adopt { reply_to, run, attempt: Token::new(attempt) });
}

/// A worker's hello: a run it lists as ending or answered takes no inbox
/// event passed to it from now on, whatever crosses its answer.
pub(crate) fn hello(model: &mut Model, hello: &crate::boundary::Hello) {
    for hosted in &hello.hosting {
        let ending = match hosted.phase {
            fleet::Phase::Ending | fleet::Phase::Answered => true,
            fleet::Phase::Preparing | fleet::Phase::Starting | fleet::Phase::Active | fleet::Phase::Waiting => false,
        };
        let Some(id) = items::find(model, hosted.item) else { continue };
        if let Some(live) = get_mut(model, id).live.as_mut()
            && live.attempt == hosted.attempt
            && ending
        {
            live.bounced = true;
        }
    }
}

/// The hub cancels the item's attempt `attempt`: the fleet cancels it, or, if
/// it is still being prepared, it ends at once, refused.
pub(crate) fn cancel(model: &mut Model, env: &Env<Limits>, item: Item, attempt: u64) {
    let Some(id) = items::find(model, item) else { return };
    let entry = get_mut(model, id);
    let started = match entry.live {
        Some(live) => live.attempt != attempt || live.started,
        None => true,
    };
    if started {
        let run = translate::run_of(item);
        return route::fleet_step(model, env, fleet::Event::Cancel { run, attempt: Token::new(attempt) });
    }
    entry.job = Job::Idle;
    entry.assignment = None;
    entry.grants = None;
    end(model, env, id, attempt, work::Answer::Refused);
}

/// Ends the item's attempt the fleet does not have: the hub hears it as
/// answered, with nothing to acknowledge.
fn end(model: &mut Model, env: &Env<Limits>, id: Id<Entry>, attempt: u64, answer: work::Answer) {
    let entry = get_mut(model, id);
    entry.live = None;
    entry.assignment = None;
    entry.grants = None;
    let item = entry.item;
    route::work_step(model, env, work::Event::Answered { item, attempt, answer });
}

/// The fleet assigns the item's attempt to a worker.
pub(crate) fn assign(model: &mut Model, channel: Token, run: Token, attempt: Token, out: &mut Queue<Request>) {
    let item = translate::item(run);
    let Some(id) = items::find(model, item) else { return };
    let Some(assignment) = get(model, id).assignment.as_ref() else { return };
    if assignment.attempt != attempt.raw() {
        return;
    }
    out.push(Request::Assign { channel, assignment: Assignment::clone(assignment) });
}

/// The fleet places the item's attempt on a worker.
pub(crate) fn placed(model: &mut Model, env: &Env<Limits>, run: Token, attempt: Token) {
    let item = translate::item(run);
    let phase =
        views::Event::Phase { item: run, repository: item.repository, phase: crate::boundary::Phase::Running.code() };
    route::views_step(model, env, phase);
    route::work_step(model, env, work::Event::Placed { item, attempt: attempt.raw() });
}

/// The hub relays the inbox event `event` to the item's attempt.
pub(crate) fn inbound(model: &mut Model, env: &Env<Limits>, item: Item, attempt: u64, event: Token) {
    let run = translate::run_of(item);
    route::fleet_step(model, env, fleet::Event::Inbound { run, attempt: Token::new(attempt), event });
}

/// The fleet passes the inbox event `event` to the worker hosting the
/// item's attempt.
pub(crate) fn deliver(
    model: &mut Model,
    channel: Token,
    run: Token,
    attempt: Token,
    event: Token,
    out: &mut Queue<Request>,
) {
    let item = translate::item(run);
    let Some(id) = items::find(model, item) else { return };
    let Some(noted) = get_mut(model, id).inbox.get_mut(&event.raw()) else { return };
    noted.delivered = true;
    let inbound = noted.inbound;
    out.push(Request::Inbound { channel, item, attempt: attempt.raw(), event: inbound });
}

/// News of the item, from the forge sub-model: kept in its inbox, and told
/// the hub.
pub(crate) fn news(model: &mut Model, env: &Env<Limits>, item: forge::Item, seq: u64, news: forge::News) {
    let Some(id) = items::find(model, translate::item_of(item)) else { return };
    let (source, pushed) = match news {
        forge::News::Comment { .. } => (plan::Source::Message, None),
        forge::News::Reviews { .. } => (plan::Source::Own, None),
        forge::News::Pull { commit, .. } => (plan::Source::Own, Some(commit)),
    };
    if let Some(commit) = pushed {
        let base = match model.forge.pull(item) {
            Some(level) => level.base,
            None => None,
        };
        let entry = get_mut(model, id);
        let moved = match entry.seen {
            Some(seen) => seen.head != commit,
            None => true,
        };
        if moved {
            entry.seen = Some(Seen { head: commit, at: env.now, base });
        }
    }
    items::inbox(model, env, id, Inbound::News(news), Some(seq), source);
}

/// The hub keeps the snapshot its attempt parked with: to the store.
pub(crate) fn keep(model: &mut Model, item: Item, snapshot: Token) {
    let Some(carried) = model.carried.get_mut(Id::from_token(snapshot)) else { return };
    let Some(answer) = carried.answer_mut() else { return };
    let kept = match answer {
        Answer::Parked { snapshot, .. } => snapshot.take(),
        Answer::Busy | Answer::Invalid | Answer::Ended { .. } | Answer::Failed { .. } => None,
    };
    let Some(bytes) = kept else { return };
    let Some(id) = items::find(model, item) else { return };
    get_mut(model, id).relations.snapshot = true;
    let Ok(wait) = model.waits.insert(Wait::Aside { entry: Some(id) }) else {
        unreachable!("the waits have room for every item's job")
    };
    let op = crate::boundary::Store::Put { item, snapshot: bytes };
    model.requests.push(Request::Store { owner: wait.token(), op });
}

/// The hub has the item's attempt's answer durably, or does not want it:
/// the fleet acknowledges it, if it handed it, and its payload goes.
pub(crate) fn acknowledge(model: &mut Model, env: &Env<Limits>, item: Item, attempt: u64) {
    let Some(payload) = model.handed.remove(&(item, attempt)) else { return };
    forget(model, payload);
    let run = translate::run_of(item);
    route::fleet_step(model, env, fleet::Event::Acknowledge { run, attempt: Token::new(attempt) });
    if let Some(id) = items::find(model, item)
        && get(model, id).resave
    {
        items::aside(model, env, id);
    }
}

/// The hub is done with the item: the forge sub-model lets it go.
pub(crate) fn left(model: &mut Model, env: &Env<Limits>, item: Item) {
    items::finished(model, env, item);
    let Some(id) = items::find(model, item) else { return };
    let closed = get(model, id).closed;
    items::drop_entry(model, id);
    if !closed {
        route::forge_step(model, env, forge::Event::Untrack { item: translate::forge_item(item) });
    }
}

/// A worker's answer for the item's attempt, which the fleet takes on.
pub(crate) fn answer(model: &mut Model, env: &Env<Limits>, channel: Token, item: Item, attempt: u64, answer: Answer) {
    let Some(run) = translate::run(item) else { return };
    let acted = translate::answer(&answer);
    let carried = Carried::Answer { item, attempt, answer: Box::new(answer) };
    let Ok(payload) = model.carried.insert(carried) else {
        // No room: its channel is closed rather than the answer dropped,
        // and the worker sends it again after its next hello.
        model.requests.push(Request::Refuse { channel });
        return;
    };
    let event =
        fleet::Event::Answer { channel, run, attempt: Token::new(attempt), answer: acted, payload: payload.token() };
    route::fleet_step(model, env, event);
}

/// The fleet hands on the answer of the item's attempt: what the run took
/// leaves the inbox, then the hub hears it.
pub(crate) fn answered(model: &mut Model, env: &Env<Limits>, to: ReplyTo, run: Token, attempt: Token, payload: Token) {
    assert!(to.into_token() == run, "an attempt's answer ends its run's call");
    let item = translate::item(run);
    let attempt = attempt.raw();
    let answer = match model.carried.get(Id::from_token(payload)) {
        Some(Carried::Answer { answer, .. }) => answer,
        Some(Carried::Call { .. } | Carried::Served { .. } | Carried::Report { .. } | Carried::Done) | None => {
            unreachable!("an answer handed on is the one carried")
        }
    };
    let (answer, work) = match answer.as_ref() {
        Answer::Ended { work, .. } => (work::Answer::Ended { outcome: payload }, Some(work)),
        Answer::Parked { snapshot, work } => {
            let snapshot = if snapshot.is_some() { Some(payload) } else { None };
            (work::Answer::Parked { snapshot }, Some(work))
        }
        Answer::Failed { failure, work } => (work::Answer::Failed(translate::class(*failure)), Some(work)),
        Answer::Invalid => (work::Answer::Failed(work::Class::Permanent), None),
        Answer::Busy => unreachable!("the fleet places a busy attempt again"),
    };
    let landed = match work {
        Some(work) => {
            let mut head = None;
            for landed in &work.landed {
                if landed.repository == item.repository {
                    head = Some(landed.commit);
                }
            }
            head
        }
        None => None,
    };
    if model.handed.insert((item, attempt), payload).is_err() {
        unreachable!("the answers handed on have room for every attempt");
    }
    route::views_step(model, env, views::Event::Finished { run });
    if let Some(id) = items::find(model, item) {
        let live = get(model, id).live;
        if let Some(live) = live
            && live.attempt == attempt
        {
            {
                let took = match answer {
                    work::Answer::Ended { .. } | work::Answer::Parked { .. } => true,
                    work::Answer::Failed(_) | work::Answer::Lost | work::Answer::Refused => false,
                };
                if took {
                    items::took(model, env, id);
                }
                let entry = get_mut(model, id);
                if let Some(head) = landed {
                    entry.relations.branch = Some(head);
                }
                // A turn that parked is over, as one whose outcome is applied
                // is: the next waits for its wake, not retried at once.
                let parked = match answer {
                    work::Answer::Parked { .. } => true,
                    work::Answer::Ended { .. }
                    | work::Answer::Failed(_)
                    | work::Answer::Lost
                    | work::Answer::Refused => false,
                };
                if parked && let Some(record) = entry.step.as_mut() {
                    record.progress.running = None;
                }
                entry.live = None;
                entry.assignment = None;
                entry.grants = None;
            }
        }
    }
    route::work_step(model, env, work::Event::Answered { item, attempt, answer });
}

/// The fleet ends the item's attempt with no worker's answer: lost,
/// withdrawn or refused.
pub(crate) fn ended(
    model: &mut Model,
    env: &Env<Limits>,
    to: ReplyTo,
    run: Token,
    attempt: Token,
    answer: work::Answer,
) {
    assert!(to.into_token() == run, "an attempt's ending ends its run's call");
    let item = translate::item(run);
    let attempt = attempt.raw();
    route::views_step(model, env, views::Event::Finished { run });
    if let Some(id) = items::find(model, item) {
        let entry = get_mut(model, id);
        if let Some(live) = entry.live
            && live.attempt == attempt
        {
            entry.live = None;
            entry.assignment = None;
            entry.grants = None;
        }
    }
    route::work_step(model, env, work::Event::Answered { item, attempt, answer });
}

/// The attempt the answer `outcome` carries is of.
pub(crate) fn attempt_of(model: &Model, outcome: Token) -> u64 {
    match model.carried.get(Id::from_token(outcome)) {
        Some(Carried::Answer { attempt, .. }) => *attempt,
        Some(Carried::Call { .. } | Carried::Served { .. } | Carried::Report { .. } | Carried::Done) | None => 0,
    }
}

/// The outcome the answer `outcome` carries, as it is posted.
pub(crate) fn posted(model: &Model, outcome: Token) -> Option<Box<Posted>> {
    let (item, attempt, answer) = model.carried.get(Id::from_token(outcome))?.answer()?;
    let (outcome, work) = match answer {
        Answer::Ended { outcome, work } => (outcome, work),
        Answer::Busy | Answer::Invalid | Answer::Parked { .. } | Answer::Failed { .. } => return None,
    };
    let mut head = None;
    for landed in &work.landed {
        if landed.repository == item.repository {
            head = Some(landed.commit);
        }
    }
    Some(Box::new(Posted { attempt, outcome: outcome.clone(), head }))
}

/// Forgets what the top level carried for the fleet as `payload`.
pub(crate) fn forget(model: &mut Model, payload: Token) {
    take_carried(model, Id::from_token(payload));
}

/// The fleet drops what the top level carried as `payload`. A run's call it
/// could not pass up is answered at once, unserved, on the channel it came
/// on (never silently): busy if its attempt may yet be the live claim (the
/// cold start is not done, or the fleet had no room for it), failed if it is
/// fenced off. A run's answer that comes too late (its attempt presumed lost
/// and fenced off, or kept as a stray past the grace) still says where it
/// pushed: the item's branch is there on the forge, and its next run starts
/// from it, unless the item records a branch already.
pub(crate) fn dropped(model: &mut Model, env: &Env<Limits>, payload: Token) {
    let Some(taken) = take_carried(model, Id::from_token(payload)) else { return };
    match taken {
        Carried::Call { channel, item, attempt, call, body } => {
            let relayed = Relayed { channel, item, attempt, call, body };
            let live = match items::find(model, item) {
                Some(id) => match get(model, id).live {
                    Some(live) => live.attempt == attempt,
                    None => false,
                },
                None => false,
            };
            let why = if live || model.loaded.is_none() { Unserved::Busy } else { Unserved::Failed };
            unrouted(model, &relayed, why);
        }
        Carried::Answer { item, attempt: _, answer } => late(model, env, item, &answer),
        Carried::Served { .. } | Carried::Report { .. } | Carried::Done => {}
    }
}

/// A run's answer the fleet drops: the push it made, if the item records
/// none. One it records is a later answer's, or what was read on the forge,
/// either newer than this push: only the record says so after a restart.
fn late(model: &mut Model, env: &Env<Limits>, item: Item, answer: &Answer) {
    let work = match answer {
        Answer::Ended { work, .. } | Answer::Parked { work, .. } | Answer::Failed { work, .. } => work,
        Answer::Busy | Answer::Invalid => return,
    };
    let mut head = None;
    for landed in &work.landed {
        if landed.repository == item.repository {
            head = Some(landed.commit);
        }
    }
    let Some(head) = head else { return };
    let Some(id) = items::find(model, item) else { return };
    let entry = get_mut(model, id);
    if entry.relations.branch.is_some() {
        return;
    }
    entry.relations.branch = Some(head);
    items::aside(model, env, id);
}

/// Answers a run's call at once, on the channel it came on.
fn unrouted(model: &mut Model, relayed: &Relayed, why: Unserved) {
    let Relayed { channel, item, attempt, call, .. } = *relayed;
    model.requests.push(Request::Relayed { channel, item, attempt, call, served: Served::Unserved(why) });
}

/// A worker relays a call of the item's run: carried to the fleet, which
/// passes it up if the attempt is live.
pub(crate) fn relay(
    model: &mut Model,
    env: &Env<Limits>,
    channel: Token,
    item: Item,
    attempt: u64,
    call: Token,
    body: Call,
) {
    let Some(run) = translate::run(item) else {
        let relayed = Relayed { channel, item, attempt, call, body: Box::new(body) };
        return unrouted(model, &relayed, Unserved::Invalid);
    };
    let payload = match model.carried.insert(Carried::Call { channel, item, attempt, call, body: Box::new(body) }) {
        Ok(payload) => payload,
        Err(carried) => {
            if let Some(relayed) = carried.call() {
                unrouted(model, &relayed, Unserved::Busy);
            }
            return;
        }
    };
    let event = fleet::Event::Relay { run, attempt: Token::new(attempt), call, body: payload.token() };
    route::fleet_step(model, env, event);
}

/// The fleet passes a run's call up: served, and answered once.
pub(crate) fn call(model: &mut Model, env: &Env<Limits>, to: ReplyTo, run: Token, attempt: Token, body: Token) {
    let item = translate::item(run);
    let attempt = attempt.raw();
    let id = Id::from_token(body);
    let Some(taken) = take_carried(model, id) else { unreachable!("a call passed up is the one carried") };
    let Some(relayed) = taken.call() else { unreachable!("a call passed up is the one carried") };
    let Ok(wait) = model.waits.insert(Wait::Relay { to }) else {
        unreachable!("the waits have room for every run's call")
    };
    crate::serve::serve(model, env, wait, item, attempt, relayed.call, *relayed.body);
}

/// Takes what is carried as `id`, which goes at the reclaim point.
fn take_carried(model: &mut Model, id: Id<Carried>) -> Option<Carried> {
    let carried = model.carried.get_mut(id)?;
    let taken = mem::replace(carried, Carried::Done);
    match taken {
        Carried::Done => None,
        Carried::Answer { .. } | Carried::Call { .. } | Carried::Served { .. } | Carried::Report { .. } => {
            model.carried.retire(id);
            Some(taken)
        }
    }
}

/// The fleet passes the answer to a run's call down to its worker.
pub(crate) fn relayed(
    model: &mut Model,
    channel: Token,
    run: Token,
    attempt: Token,
    call: Token,
    answer: Token,
    out: &mut Queue<Request>,
) {
    let id = Id::from_token(answer);
    let Some(taken) = take_carried(model, id) else { return };
    let Some(served) = taken.served() else { return };
    let item = translate::item(run);
    out.push(Request::Relayed { channel, item, attempt: attempt.raw(), call, served: *served });
}

/// The answer to a run's call goes to the fleet, carried.
pub(crate) fn serve_answer(model: &mut Model, env: &Env<Limits>, to: ReplyTo, served: Served) {
    let Ok(payload) = model.carried.insert(Carried::Served { served: Box::new(served) }) else {
        unreachable!("the carried answers have room for every run's call")
    };
    route::fleet_step(model, env, fleet::Event::Relayed { to, answer: payload.token() });
}

/// A worker bounced an inbound event.
pub(crate) fn bounced(model: &mut Model, env: &Env<Limits>, item: Item, attempt: u64, bounce: fleet::Bounce) {
    let Some(run) = translate::run(item) else { return };
    route::fleet_step(model, env, fleet::Event::Bounced { run, attempt: Token::new(attempt), bounce });
}

/// The fleet passes a bounce up: what the attempt took is then only what its
/// brief had.
pub(crate) fn bounce(model: &mut Model, run: Token, attempt: Token) {
    let Some(id) = items::find(model, translate::item(run)) else { return };
    let entry = get_mut(model, id);
    if let Some(live) = entry.live.as_mut()
        && live.attempt == attempt.raw()
    {
        live.bounced = true;
    }
}

/// A worker passes a run's report: carried to the fleet, which drops it
/// unless its attempt is live.
pub(crate) fn told(
    model: &mut Model,
    env: &Env<Limits>,
    item: Item,
    attempt: u64,
    kind: views::Kind,
    content: Box<[u8]>,
) {
    let Some(run) = translate::run(item) else { return };
    let Ok(payload) = model.carried.insert(Carried::Report { kind, content }) else { return };
    route::fleet_step(model, env, fleet::Event::Told { run, attempt: Token::new(attempt), fact: payload.token() });
}

/// The fleet passes a run's report up: to the views.
pub(crate) fn report(model: &mut Model, env: &Env<Limits>, run: Token, fact: Token) {
    let id = Id::from_token(fact);
    let Some(taken) = take_carried(model, id) else { return };
    let Some((kind, content)) = taken.report() else { return };
    route::views_step(model, env, views::Event::Reported { run, kind, content });
}

/// A run's call that cannot be served: answered at once.
pub(crate) fn unserved(model: &mut Model, env: &Env<Limits>, wait: Id<Wait>, why: Unserved) {
    let Some(taken) = crate::serve::take(model, wait.token()) else { return };
    let Some(to) = taken.relay() else { return };
    serve_answer(model, env, to, Served::Unserved(why));
}
