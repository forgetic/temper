//! The fake forge's state and its entry points.
//!
//! A call is decided when it arrives: refused for its user's rate, failed by
//! chance, or made, its answer held until its latency is past, and then
//! answered by [`fire`]. CI's verdicts and webhook deliveries go out the same
//! way, as their timers fire.

use alloc::boxed::Box;
use core::mem;

use temper_lib::bytes::copy_of;
use temper_lib::{Deadlines, Duration, Env, Id, Map, Queue, ReplyTo, Rng, Set, Slab, Time};

use crate::boundary::{Event, Request};

use crate::api::{Answer, Error, Op, Permission, Read, What, Write};
use crate::faults::{self, Window};
use crate::git::{self, Object};
use crate::hooks::{self, Delivery, Hook};
use crate::limits::{self, Limits};
use crate::observe::{self, Observation, Observations, Operation};
use crate::store::Repository;
use crate::{ci, issues, pulls, reads, wiki};

/// The most requests an entry point emits per call.
pub const MAX_OUT: u32 = 1;

/// How the forge behaves, handed to every step read-only. Chances are per
/// mille.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    pub limits: Limits,
    /// The time to answer a call is drawn from `latency_min..=latency_max`,
    /// or, for the `late` chance of calls, from `late_min..=late_max`.
    pub latency_min: Duration,
    pub latency_max: Duration,
    pub late: u32,
    pub late_min: Duration,
    pub late_max: Duration,
    /// The chance that a call fails as unavailable, having done nothing.
    pub unavailable: u32,
    /// The chance that a call is made and then fails as timed out.
    pub timeouts: u32,
    /// The calls a user may make in a window of `rate_window`, which starts
    /// with their first call after the last one ended. Zero: no limit.
    pub rate_limit: u32,
    pub rate_window: Duration,
    /// The user CI reports as.
    pub ci: u64,
    /// The time to deliver a webhook is drawn from `hook_min..=hook_max`, or,
    /// for the `hooks_late` chance of them, from `late_min..=late_max`; and
    /// the `hooks_lost` chance of them is never delivered.
    pub hook_min: Duration,
    pub hook_max: Duration,
    pub hooks_late: u32,
    pub hooks_lost: u32,
    /// The resolution of the times the forge keeps and shows: an item's
    /// created and updated times, a comment's, a review's, a status's.
    /// Forgejo keeps seconds. Zero: the clock's own.
    pub resolution: Duration,
    /// Whether a status reported on an open pull request's head moves its
    /// updated time, and whether editing or deleting a comment moves its
    /// item's. Neither does on Forgejo.
    pub status_updates: bool,
    pub edit_updates: bool,
}

/// What the forge has done, for a world to check at settle.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tally {
    /// Calls answered, and those answered at once as unavailable for want of
    /// room.
    pub answered: u32,
    pub busy: u32,
    /// Calls failed by chance: unavailable, having done nothing; and timed
    /// out, having been made.
    pub unavailable: u32,
    pub timeouts: u32,
    /// Calls refused for their user's rate, and those refused because every
    /// rate window the limits keep was running.
    pub limited: u32,
    pub crowded: u32,
    /// Writes refused because the store or a repository was full: a world
    /// whose forge fills tests a forge that refuses everything.
    pub full: u32,
    /// Calls answered late.
    pub late: u32,
    /// Webhooks delivered, those of them late, those lost by chance, and those
    /// dropped for want of room.
    pub hooks: u32,
    pub hooks_late: u32,
    pub hooks_lost: u32,
    pub hooks_dropped: u32,
    /// CI verdicts reported, contexts left pending by chance, commits CI did
    /// not report on for want of room, and commits whose statuses were
    /// forgotten to make room, being no longer any head.
    pub verdicts: u32,
    pub silent: u32,
    pub unreported: u32,
    pub forgotten: u32,
}

impl Tally {
    const ZERO: Tally = Tally {
        answered: 0,
        busy: 0,
        unavailable: 0,
        timeouts: 0,
        limited: 0,
        crowded: 0,
        full: 0,
        late: 0,
        hooks: 0,
        hooks_late: 0,
        hooks_lost: 0,
        hooks_dropped: 0,
        verdicts: 0,
        silent: 0,
        unreported: 0,
        forgotten: 0,
    };
}

/// What more the forge has room for (see [`Model::room`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Room {
    pub commits: u32,
    pub statuses: u32,
}

/// The fake forge's state.
#[derive(Debug)]
pub struct Model {
    pub(crate) repositories: Slab<Repository>,
    /// The repositories by their names.
    pub(crate) names: Map<Box<[u8]>, Id<Repository>>,
    /// Every commit there is, by its name, and the last name given.
    pub(crate) commits: Map<u64, Object>,
    pub(crate) made: u64,
    /// The last id given to a comment.
    pub(crate) comments: u64,
    pub(crate) calls: Slab<Call>,
    pub(crate) deliveries: Slab<Delivery>,
    /// Each calling user's rate window.
    pub(crate) windows: Map<u64, Window>,
    pub(crate) timers: Deadlines<Alarm>,
    pub(crate) observations: Observations,
    pub(crate) rng: Rng,
    pub(crate) tally: Tally,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Alarm {
    /// The call's answer goes out.
    Call(Id<Call>),
    /// CI reports the context `context` of the repository's checks on
    /// `commit`.
    Check { repository: Id<Repository>, commit: u64, context: u32 },
    /// The webhook is delivered.
    Hook(Id<Delivery>),
}

/// A call being answered.
#[derive(Debug)]
pub(crate) struct Call {
    state: State,
}

#[derive(Debug)]
enum State {
    /// The answer is decided, and goes out when the call's timer fires.
    Waiting { reply_to: ReplyTo, result: Result<Answer, Error> },
    /// Terminal: holds nothing.
    Closed,
}

impl Model {
    /// An empty forge, drawing from `seed`.
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Model {
        let limits = &config.limits;
        assert!(limits::worst_case(limits).is_some(), "the limits fit");
        assert!(
            limits.repositories > 0 && limits.users > 0 && limits.items > 0 && limits.commits > 0,
            "a forge holds something"
        );
        assert!(limits.files > 0 && limits.branches > 0, "a repository holds a first commit on its default branch");
        assert!(limits.page_size > 0 && limits.calls > 0, "a listing answers something, and a call is taken");
        assert!(
            config.latency_min <= config.latency_max
                && config.late_min <= config.late_max
                && config.hook_min <= config.hook_max,
            "latencies are ranges"
        );
        assert!(config.rate_limit == 0 || config.rate_window > Duration::ZERO, "a rate is over a window");
        Model {
            repositories: Slab::with_capacity(limits.repositories),
            names: Map::with_capacity(limits.repositories),
            commits: Map::with_capacity(limits.commits),
            made: 0,
            comments: 0,
            calls: Slab::with_capacity(limits.calls),
            deliveries: Slab::with_capacity(limits.hooks),
            windows: Map::with_capacity(limits.users),
            timers: Deadlines::with_capacity(limits::timers(limits).unwrap_or(u32::MAX)),
            observations: Observations::with_capacity(limits.observations),
            rng: Rng::new(seed),
            tally: Tally::ZERO,
        }
    }

    /// Where `branch` of `repository` is.
    #[must_use]
    pub fn branch(&self, repository: &[u8], branch: &[u8]) -> Option<u64> {
        let repository = self.repositories.get(self.id(repository)).expect("a repository of the forge");
        repository.branches.get(branch).copied()
    }

    /// The branches of `repository`, and where each is.
    #[must_use]
    pub fn branches(&self, repository: &[u8]) -> &Map<Box<[u8]>, u64> {
        &self.repositories.get(self.id(repository)).expect("a repository of the forge").branches
    }

    /// The commits `repository` has: what its branches reach, and what was
    /// pushed or merged. A working tree's git cloning it has them all. (A
    /// world that wants the moves of branches git's way keeps them from the
    /// observations.)
    #[must_use]
    pub fn has(&self, repository: &[u8]) -> &Set<u64> {
        &self.repositories.get(self.id(repository)).expect("a repository of the forge").has
    }

    /// The commit `commit`: its parent and its tree. A working tree's git
    /// fetching it walks its parents here.
    #[must_use]
    pub fn object(&self, commit: u64) -> Option<&Object> {
        self.commits.get(&commit)
    }

    /// Whether `ancestor` is `commit` or one of its ancestors.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: u64, commit: u64) -> bool {
        git::is_ancestor(self, ancestor, commit)
    }

    /// What `read` answers on `repository`, for whoever may read it, with no
    /// faults: for a world inspecting the store.
    pub fn inspect(&self, config: &Config, repository: &[u8], read: &Read) -> Result<Answer, Error> {
        let Some(&id) = self.names.get(repository) else {
            return Err(Error::Missing(What::Repository));
        };
        reads::read(self, config, id, read)
    }

    /// The next observation, oldest first.
    pub fn pop_observation(&mut self) -> Option<Observation> {
        self.observations.pop()
    }

    /// How many observations were dropped for want of room, since the forge
    /// was made.
    #[must_use]
    pub fn observations_lost(&self) -> u64 {
        self.observations.lost()
    }

    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// What more the forge has room for: commits in its store, and statuses
    /// in the repository with the least room for them.
    #[must_use]
    pub fn room(&self) -> Room {
        let commits = self.commits.capacity().saturating_sub(self.commits.len());
        let mut statuses = u32::MAX;
        for (_, &id) in &self.names {
            let repository = self.repositories.get(id).expect("a named repository");
            statuses = statuses.min(repository.statuses.capacity().saturating_sub(repository.statuses.len()));
        }
        Room { commits, statuses }
    }

    /// Calls held, answered ones included until they are reclaimed.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    /// Webhooks in flight, delivered ones included until they are reclaimed.
    #[must_use]
    pub fn deliveries(&self) -> u32 {
        self.deliveries.len()
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.timers.next()
    }

    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.timers.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// The reclaim point: frees the calls answered and the webhooks delivered.
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
        self.deliveries.reclaim();
    }

    pub(crate) fn id(&self, repository: &[u8]) -> Id<Repository> {
        *self.names.get(repository).expect("a repository of the forge")
    }
}

/// Handles one event, emitting at most [`MAX_OUT`] requests: a call refused
/// at once for want of room.
pub fn step(model: &mut Model, env: &Env<Config>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Call { reply_to, user, repository, op } => call(model, env, reply_to, user, &repository, op, out),
    }
}

/// Fires the earliest timer due at `env.now`, if there is one, emitting at
/// most [`MAX_OUT`] requests: a call's answer, or a webhook.
pub fn fire(model: &mut Model, env: &Env<Config>, out: &mut Queue<Request>) {
    let Some(alarm) = model.timers.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Call(id) => answer(model, id, out),
        Alarm::Check { repository, commit, context } => ci::report(model, env, repository, commit, context),
        Alarm::Hook(id) => hooks::deliver(model, id, out),
    }
}

fn call(
    model: &mut Model,
    env: &Env<Config>,
    reply_to: ReplyTo,
    user: u64,
    repository: &[u8],
    op: Op,
    out: &mut Queue<Request>,
) {
    if model.calls.is_full() {
        model.tally.busy = model.tally.busy.saturating_add(1);
        model.tally.answered = model.tally.answered.saturating_add(1);
        out.push(Request::Reply { to: reply_to, result: Err(Error::Unavailable) });
        return;
    }
    let result = match faults::admit(model, env, user) {
        Err(error) => Err(error),
        Ok(()) => {
            let subject = observe::subject(&op);
            let result = execute(model, env, user, repository, op);
            if let Err(error) = result
                && let Some((what, number, commit)) = subject
            {
                if error == Error::Full {
                    model.tally.full = model.tally.full.saturating_add(1);
                }
                refused(model, repository, what, number, commit, error, user);
            }
            faults::finish(model, env, result)
        }
    };
    let at = faults::latency(model, env);
    let id = model.calls.insert(Call { state: State::Waiting { reply_to, result } }).expect("checked for room above");
    model.timers.arm(Alarm::Call(id), at).expect("a timer per call fits");
}

/// Makes the call, as `user`.
fn execute(model: &mut Model, env: &Env<Config>, user: u64, repository: &[u8], op: Op) -> Result<Answer, Error> {
    let Some(&id) = model.names.get(repository) else {
        return Err(Error::Missing(What::Repository));
    };
    match op {
        Op::Read(read) => {
            let repository = model.repositories.get(id).expect("a named repository");
            repository.require(user, Permission::Read)?;
            reads::read(model, &env.limits, id, &read)
        }
        Op::Write(write) => match write {
            Write::CreateIssue { title, body, labels } => issues::create(model, env, id, user, title, body, labels),
            Write::EditItem { number, title, body } => issues::revise(model, env, id, user, number, title, body),
            Write::SetDependencies { number, dependencies } => {
                issues::depend(model, env, id, user, number, dependencies)
            }
            Write::Comment { number, body } => issues::comment(model, env, id, user, number, body),
            Write::EditComment { id: comment, body } => issues::edit(model, env, id, user, comment, body),
            Write::DeleteComment { id: comment } => issues::remove(model, env, id, user, comment),
            Write::SetLabels { number, labels } => issues::label(model, env, id, user, number, labels),
            Write::AddLabels { number, labels } => issues::add_labels(model, env, id, user, number, labels),
            Write::RemoveLabels { number, labels } => issues::remove_labels(model, env, id, user, number, labels),
            Write::DefineLabel { name } => issues::define(model, env, id, user, name),
            Write::Close { number } => issues::close(model, env, id, user, number),
            Write::Reopen { number } => issues::reopen(model, env, id, user, number),
            Write::OpenPull { title, body, head, base } => pulls::open(model, env, id, user, title, body, head, base),
            Write::SetReviewers { number, reviewers } => pulls::request(model, env, id, user, number, reviewers),
            Write::Review { number, verdict, body } => pulls::review(model, env, id, user, number, verdict, body),
            Write::Merge { number, head } => pulls::merge(model, env, id, user, number, head),
            Write::DeleteBranch { branch } => git::delete(model, env, id, user, &branch),
            Write::Status { commit, context, state } => ci::status(model, env, id, user, commit, context, state),
            Write::PutPage { name, content } => wiki::put(model, env, id, user, name, content),
            Write::DeletePage { name } => wiki::delete(model, env, id, user, &name),
        },
        Op::Git(git) => git::serve(model, env, id, user, git),
    }
}

/// A write or a git call on `repository` was refused: observed, if the
/// forge has the repository.
fn refused(
    model: &mut Model,
    repository: &[u8],
    what: Operation,
    number: Option<u64>,
    commit: Option<u64>,
    error: Error,
    by: u64,
) {
    let Some(&id) = model.names.get(repository) else {
        return;
    };
    let name = copy_of(&model.repositories.get(id).expect("a named repository").name);
    let observation = Observation::Refused { repository: name, what, number, commit, error, by };
    model.observations.push(observation);
}

/// A call's timer: its answer goes out.
fn answer(model: &mut Model, id: Id<Call>, out: &mut Queue<Request>) {
    let call = model.calls.get_mut(id).expect("a call lives until its timer fires");
    let state = mem::replace(&mut call.state, State::Closed);
    call.state = match state {
        State::Waiting { reply_to, result } => {
            out.push(Request::Reply { to: reply_to, result });
            State::Closed
        }
        State::Closed => unreachable!("a closed call has no timer"),
    };
    model.calls.retire(id);
    model.tally.answered = model.tally.answered.saturating_add(1);
}

/// The time now, at the forge's resolution: what it keeps and shows.
pub(crate) fn clock(env: &Env<Config>) -> Time {
    stamp(&env.limits, env.now)
}

/// `time` at the forge's resolution.
pub(crate) fn stamp(config: &Config, time: Time) -> Time {
    let nanos = time.as_nanos();
    match nanos.checked_rem(config.resolution.as_nanos()) {
        Some(part) => Time::from_nanos(nanos.saturating_sub(part)),
        None => time,
    }
}

/// Something changed on `repository`: what a world sees, and what its
/// subscriber hears.
pub(crate) fn changed(
    model: &mut Model,
    env: &Env<Config>,
    repository: Id<Repository>,
    observation: Observation,
    hook: Hook,
) {
    model.observations.push(observation);
    hooks::notify(model, env, repository, hook);
}
