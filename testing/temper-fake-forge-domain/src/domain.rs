//! The fake forge's state and its entry points.
//!
//! A call is decided when it arrives: refused for its user's rate, failed by
//! chance, or made, its answer held until its latency is past, and then
//! answered by [`fire`]; or, by chance, answered as timed out and made only
//! after, landing late, as a request a client gave up on may still be acted
//! on. CI's verdicts and webhook deliveries go out the same way, as their
//! timers fire.
//!
//! The forge keeps its own clock, which may be ahead of or behind the
//! world's ([`Skew`]): every time it keeps and shows is its own, at its
//! resolution ([`time`]).

use alloc::boxed::Box;
use core::mem;

use skein_lib::bytes::copy_of;
use skein_lib::{Deadlines, Duration, Env, Id, Map, Queue, ReplyTo, Rng, Set, Slab, Time};

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

/// Provider-neutral repository metadata, read without referee observations.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Metadata {
    pub name: Box<[u8]>,
    pub default_branch: Box<[u8]>,
}

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
    /// The chance that a write or a git call fails as timed out before it
    /// is made, and is made, landing, a time drawn from
    /// `land_min..=land_max` after its answer went out.
    pub landing: u32,
    pub land_min: Duration,
    pub land_max: Duration,
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
    /// How far the forge's clock is from the world's.
    pub skew: Skew,
    /// Whether a status reported on an open pull request's head moves its
    /// updated time, and whether editing or deleting a comment moves its
    /// item's. Neither does on Forgejo.
    pub status_updates: bool,
    pub edit_updates: bool,
}

/// How far the forge's clock is from the world's: the forge's time is the
/// world's moved ahead or back, and never before zero.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Skew {
    None,
    Ahead(Duration),
    Behind(Duration),
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
    /// Calls answered late, and calls made after their answer went out.
    pub late: u32,
    pub landed: u32,
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
        landed: 0,
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

/// What more the forge has room for (see [`Domain::room`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Room {
    pub commits: u32,
    pub statuses: u32,
}

/// The fake forge's state.
#[derive(Debug)]
pub struct Domain {
    pub(crate) repositories: Slab<Repository>,
    /// The repositories by their names.
    pub(crate) names: Map<Box<[u8]>, Id<Repository>>,
    /// Every commit there is, by its name, and the last name given.
    pub(crate) commits: Map<u64, Object>,
    pub(crate) made: u64,
    /// The last id given to a comment, and to a review.
    pub(crate) comments: u64,
    pub(crate) reviews: u64,
    /// The forge's time at the last step or timer: what [`Domain::inspect`]
    /// shows a listing was made at.
    pub(crate) clock: Time,
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
    /// `commit`; or runs it again.
    Check {
        repository: Id<Repository>,
        commit: u64,
        context: u32,
    },
    Rerun {
        repository: Id<Repository>,
        commit: u64,
        context: u32,
    },
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
    /// The answer is decided, and goes out when the call's timer fires; then
    /// the call lands, if it is to land late.
    Waiting { reply_to: ReplyTo, result: Result<Answer, Error>, landing: Option<Landing> },
    /// Answered as timed out: it is made when the call's timer fires again.
    Landing(Landing),
    /// Terminal: holds nothing.
    Closed,
}

/// A call to be made late: who made it, on which repository, and what.
#[derive(Debug)]
struct Landing {
    user: u64,
    repository: Box<[u8]>,
    op: Op,
}

impl Domain {
    /// An empty forge, drawing from `seed`.
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Domain {
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
        Domain {
            repositories: Slab::with_capacity(limits.repositories),
            names: Map::with_capacity(limits.repositories),
            commits: Map::with_capacity(limits.commits),
            made: 0,
            comments: 0,
            reviews: 0,
            clock: Time::ZERO,
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

    /// The commit `commit`: its parents and its tree. A working tree's git
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

    /// Metadata for the protocol's provider-specific repository document.
    pub fn metadata(&self, repository: &[u8]) -> Result<Metadata, Error> {
        let Some(&id) = self.names.get(repository) else {
            return Err(Error::Missing(What::Repository));
        };
        let stored = self.repositories.get(id).expect("live repository name");
        Ok(Metadata { name: copy_of(&stored.name), default_branch: copy_of(&stored.default) })
    }

    /// A stored user's permission, with no rate, fault or observation effect.
    pub fn permission(&self, repository: &[u8], user: u64) -> Result<Permission, Error> {
        let Some(&id) = self.names.get(repository) else {
            return Err(Error::Missing(What::Repository));
        };
        Ok(self.repositories.get(id).expect("live repository name").permission(user))
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
pub fn step(domain: &mut Domain, env: &Env<Config>, event: Event, out: &mut Queue<Request>) {
    domain.clock = clock(env);
    match event {
        Event::Call { reply_to, user, repository, op } => call(domain, env, reply_to, user, &repository, op, out),
    }
}

/// Fires the earliest timer due at `env.now`, if there is one, emitting at
/// most [`MAX_OUT`] requests: a call's answer, or a webhook.
pub fn fire(domain: &mut Domain, env: &Env<Config>, out: &mut Queue<Request>) {
    domain.clock = clock(env);
    let Some(alarm) = domain.timers.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Call(id) => answer(domain, env, id, out),
        Alarm::Check { repository, commit, context } => ci::report(domain, env, repository, commit, context),
        Alarm::Rerun { repository, commit, context } => ci::rerun(domain, env, repository, commit, context),
        Alarm::Hook(id) => hooks::deliver(domain, id, out),
    }
}

fn call(
    domain: &mut Domain,
    env: &Env<Config>,
    reply_to: ReplyTo,
    user: u64,
    repository: &[u8],
    op: Op,
    out: &mut Queue<Request>,
) {
    if domain.calls.is_full() {
        domain.tally.busy = domain.tally.busy.saturating_add(1);
        domain.tally.answered = domain.tally.answered.saturating_add(1);
        out.push(Request::Reply { to: reply_to, result: Err(Error::Unavailable) });
        return;
    }
    let (result, landing) = match faults::admit(domain, env, user) {
        Err(error) => (Err(error), None),
        Ok(()) if observe::subject(&op).is_some() && faults::lands_late(domain, env) => {
            (Err(Error::Timeout), Some(Landing { user, repository: copy_of(repository), op }))
        }
        Ok(()) => {
            let result = make(domain, env, user, repository, op);
            (faults::finish(domain, env, result), None)
        }
    };
    let at = faults::latency(domain, env);
    let call = Call { state: State::Waiting { reply_to, result, landing } };
    let id = domain.calls.insert(call).expect("checked for room above");
    domain.timers.arm(Alarm::Call(id), at).expect("a timer per call fits");
}

/// Makes the call, as `user`, observing a write or a git call refused.
fn make(domain: &mut Domain, env: &Env<Config>, user: u64, repository: &[u8], op: Op) -> Result<Answer, Error> {
    let subject = observe::subject(&op);
    let result = execute(domain, env, user, repository, op);
    if let Err(error) = result
        && let Some((what, number, commit)) = subject
    {
        if error == Error::Full {
            domain.tally.full = domain.tally.full.saturating_add(1);
        }
        refused(domain, repository, what, number, commit, error, user);
    }
    result
}

/// Makes the call, as `user`.
fn execute(domain: &mut Domain, env: &Env<Config>, user: u64, repository: &[u8], op: Op) -> Result<Answer, Error> {
    let Some(&id) = domain.names.get(repository) else {
        return Err(Error::Missing(What::Repository));
    };
    match op {
        Op::Read(read) => {
            let repository = domain.repositories.get(id).expect("a named repository");
            repository.require(user, Permission::Read)?;
            reads::read(domain, &env.limits, id, &read)
        }
        Op::Write(write) => match write {
            Write::CreateIssue { title, body, labels } => issues::create(domain, env, id, user, title, body, labels),
            Write::EditItem { number, title, body } => issues::revise(domain, env, id, user, number, title, body),
            Write::SetDependencies { number, dependencies } => {
                issues::depend(domain, env, id, user, number, dependencies)
            }
            Write::Comment { number, body } => issues::comment(domain, env, id, user, number, body),
            Write::EditComment { id: comment, body } => issues::edit(domain, env, id, user, comment, body),
            Write::DeleteComment { id: comment } => issues::remove(domain, env, id, user, comment),
            Write::SetLabels { number, labels } => issues::label(domain, env, id, user, number, labels),
            Write::AddLabels { number, labels } => issues::add_labels(domain, env, id, user, number, labels),
            Write::RemoveLabels { number, labels } => issues::remove_labels(domain, env, id, user, number, labels),
            Write::DefineLabel { name } => issues::define(domain, env, id, user, name),
            Write::Close { number } => issues::close(domain, env, id, user, number),
            Write::Reopen { number } => issues::reopen(domain, env, id, user, number),
            Write::OpenPull { title, body, head, base } => pulls::open(domain, env, id, user, title, body, head, base),
            Write::SetReviewers { number, reviewers } => pulls::request(domain, env, id, user, number, reviewers),
            Write::Review { number, verdict, body } => pulls::review(domain, env, id, user, number, verdict, body),
            Write::Submit { number, review, verdict } => pulls::submit(domain, env, id, user, number, review, verdict),
            Write::Merge { number, head } => pulls::merge(domain, env, id, user, number, head),
            Write::DeleteBranch { branch } => git::delete(domain, env, id, user, &branch),
            Write::Status { commit, context, state } => ci::status(domain, env, id, user, commit, context, state),
            Write::PutPage { name, content } => wiki::put(domain, env, id, user, name, content),
            Write::DeletePage { name } => wiki::delete(domain, env, id, user, &name),
        },
        Op::Git(git) => git::serve(domain, env, id, user, git),
    }
}

/// A write or a git call on `repository` was refused: observed, if the
/// forge has the repository.
fn refused(
    domain: &mut Domain,
    repository: &[u8],
    what: Operation,
    number: Option<u64>,
    commit: Option<u64>,
    error: Error,
    by: u64,
) {
    let Some(&id) = domain.names.get(repository) else {
        return;
    };
    let name = copy_of(&domain.repositories.get(id).expect("a named repository").name);
    let observation = Observation::Refused { repository: name, what, number, commit, error, by };
    domain.observations.push(observation);
}

/// A call's timer: its answer goes out, and it is retired, or waits to land;
/// or it lands.
fn answer(domain: &mut Domain, env: &Env<Config>, id: Id<Call>, out: &mut Queue<Request>) {
    let call = domain.calls.get_mut(id).expect("a call lives until its timer fires");
    let state = mem::replace(&mut call.state, State::Closed);
    match state {
        State::Waiting { reply_to, result, landing } => {
            out.push(Request::Reply { to: reply_to, result });
            domain.tally.answered = domain.tally.answered.saturating_add(1);
            match landing {
                Some(landing) => {
                    call.state = State::Landing(landing);
                    let config = &env.limits;
                    let at = env.now.saturating_add(faults::draw(domain, config.land_min, config.land_max));
                    domain.timers.arm(Alarm::Call(id), at).expect("a timer per call fits");
                }
                None => domain.calls.retire(id),
            }
        }
        State::Landing(Landing { user, repository, op }) => {
            domain.calls.retire(id);
            domain.tally.landed = domain.tally.landed.saturating_add(1);
            // What it answered went out long ago.
            let _made = make(domain, env, user, &repository, op);
        }
        State::Closed => unreachable!("a closed call has no timer"),
    }
}

/// The forge's time at the world's `now`, at its resolution: what it keeps
/// and shows.
#[must_use]
pub fn time(config: &Config, now: Time) -> Time {
    let skewed = match config.skew {
        Skew::None => now,
        Skew::Ahead(by) => now.saturating_add(by),
        Skew::Behind(by) => Time::from_nanos(now.as_nanos().saturating_sub(by.as_nanos())),
    };
    stamp(config, skewed)
}

/// The forge's time now.
pub(crate) fn clock(env: &Env<Config>) -> Time {
    time(&env.limits, env.now)
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
    domain: &mut Domain,
    env: &Env<Config>,
    repository: Id<Repository>,
    observation: Observation,
    hook: Hook,
) {
    domain.observations.push(observation);
    hooks::notify(domain, env, repository, hook);
}
