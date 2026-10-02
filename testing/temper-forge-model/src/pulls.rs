//! Pull requests: opening one, reviewing it, merging it, and how a read shows
//! it.
//!
//! A pull request's head commit follows its head branch while it is open. A
//! review is made at once, or started pending, which no read shows until its
//! author submits it; it keeps the id it was started with, so it is shown
//! before reviews submitted while it was pending. A
//! merge happens at an exact head, refused if the head moved, as a squash: one
//! commit on the base's tip with the merged tree. The merge is three-way from
//! the newest commit head and base share, and conflicts where both changed a
//! path differently since. A protected base takes a merge only with the
//! statuses and approvals its protection requires, on the exact head.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List, Map, Set};

use crate::api::{
    Answer, Change, Check, Error, Kind, Permission, Pull as PullView, Review, State, Status as StatusView, Verdict,
    What,
};
use crate::ci;
use crate::git::{self, Object, Tree};
use crate::hooks::Hook;
use crate::limits::Limits;
use crate::model::{self, Config, Model};
use crate::observe::{Branches, Observation};
use crate::store::{Item, Kept, Pull, Repository, fit_numbers, fits, numbers};

/// Opens a pull request to merge `head` into `base`.
#[expect(clippy::too_many_arguments, reason = "a cell handler takes the call's fields as they come")]
pub(crate) fn open(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    title: Box<[u8]>,
    body: Box<[u8]>,
    head: Box<[u8]>,
    base: Box<[u8]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&title, limits.title_bytes)?;
    fits(&body, limits.body_bytes)?;
    let Some(&commit) = repository.branches.get(&*head) else {
        return Err(Error::Missing(What::Branch));
    };
    let Some(&onto) = repository.branches.get(&*base) else {
        return Err(Error::Missing(What::Branch));
    };
    if head == base || git::is_ancestor(model, commit, onto) {
        return Err(Error::NothingToMerge);
    }
    if repository.open_pull(&head, &base).is_some() {
        return Err(Error::Exists);
    }
    if title.is_empty() {
        return Err(Error::Empty);
    }
    repository.room()?;
    let pull = Pull {
        head,
        base,
        commit,
        merged: None,
        requested: Set::with_capacity(limits.users),
        reviews: List::with_capacity(limits.reviews),
    };
    let item = Item {
        title,
        body,
        author: user,
        state: State::Open,
        labels: Set::with_capacity(limits.labels),
        comments: Map::with_capacity(limits.comments),
        dependencies: Set::with_capacity(limits.dependencies),
        created: model::clock(env),
        updated: model::clock(env),
        pull: Some(pull),
    };
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    let number = repository.number(item);
    let item = repository.items.get(&number).expect("the pull request just opened");
    let pull = item.pull.as_ref().expect("a pull request");
    let observation = Observation::Opened {
        repository: copy_of(&repository.name),
        number,
        kind: Kind::Pull,
        title: copy_of(&item.title),
        body: copy_of(&item.body),
        labels: Box::new([]),
        branches: Some(Branches { head: copy_of(&pull.head), base: copy_of(&pull.base), commit }),
        by: user,
    };
    model::changed(model, env, id, observation, Hook::item(Change::Pull, number));
    ci::start(model, env, id, commit);
    Ok(Answer::Created(number))
}

/// Makes the users asked to review the open pull request `number` exactly
/// `reviewers`.
pub(crate) fn request(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    reviewers: Box<[u64]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fit_numbers(&reviewers, limits.users)?;
    let item = repository.item(number)?;
    if item.pull.is_none() {
        return Err(Error::Missing(What::Pull));
    }
    if item.state == State::Closed {
        return Err(Error::Closed);
    }
    let mut set = Set::with_capacity(limits.users);
    for &reviewer in &reviewers {
        // Forgejo asks neither the author nor whoever may not read.
        if reviewer == item.author || repository.permission(reviewer) < Permission::Read {
            return Err(Error::Forbidden);
        }
        if set.insert(reviewer).is_err() {
            return Err(Error::TooLarge);
        }
    }
    let observed = numbers(&set);
    let item = repository.item_mut(number)?;
    item.pull.as_mut().expect("a pull request").requested = set;
    repository.touch(number, model::clock(env));
    let observation =
        Observation::Requested { repository: copy_of(&repository.name), number, reviewers: observed, by: user };
    model::changed(model, env, id, observation, Hook::item(Change::Pull, number));
    Ok(Answer::Done)
}

/// Reviews the open pull request `number` at its head with `verdict`, or
/// starts a pending review with none. Its author may only comment.
pub(crate) fn review(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    verdict: Option<Verdict>,
    body: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&body, env.limits.limits.body_bytes)?;
    let writer = repository.permission(user) >= Permission::Write;
    let item = repository.item_mut(number)?;
    let author = item.author;
    let state = item.state;
    let Some(pull) = &mut item.pull else {
        return Err(Error::Missing(What::Pull));
    };
    if state == State::Closed {
        return Err(Error::Closed);
    }
    // A pending review's verdict is the submission's to check.
    let commenting = match verdict {
        Some(verdict) => verdict == Verdict::Comment,
        None => true,
    };
    if author == user && !commenting {
        return Err(Error::Forbidden);
    }
    let review = model.reviews.checked_add(1).expect("ids do not run out");
    let kept = Kept {
        review: Review {
            id: review,
            author: user,
            verdict: verdict.unwrap_or(Verdict::Comment),
            commit: pull.commit,
            body,
            at: model::clock(env),
            official: writer,
        },
        pending: verdict.is_none(),
    };
    if pull.reviews.push(kept).is_err() {
        return Err(Error::Full);
    }
    model.reviews = review;
    if verdict.is_some() {
        shown(model, env, id, number, review);
    }
    Ok(Answer::Reviewed(review))
}

/// Submits the pending review `review` of the open pull request `number`,
/// the user's own, with `verdict`.
pub(crate) fn submit(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    review: u64,
    verdict: Verdict,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    let writer = repository.permission(user) >= Permission::Write;
    let item = repository.item_mut(number)?;
    let author = item.author;
    let state = item.state;
    let Some(pull) = &mut item.pull else {
        return Err(Error::Missing(What::Pull));
    };
    if state == State::Closed {
        return Err(Error::Closed);
    }
    let mut found = None;
    for kept in &pull.reviews {
        if kept.review.id == review && kept.pending {
            found = Some(kept.review.author);
        }
    }
    let Some(by) = found else {
        return Err(Error::Missing(What::Review));
    };
    if by != user {
        return Err(Error::Forbidden);
    }
    if author == user && verdict != Verdict::Comment {
        return Err(Error::Forbidden);
    }
    for index in 0..pull.reviews.len() {
        let kept = pull.reviews.get_mut(index).expect("within its length");
        if kept.review.id == review {
            kept.pending = false;
            kept.review.verdict = verdict;
            kept.review.official = writer;
            kept.review.at = model::clock(env);
        }
    }
    shown(model, env, id, number, review);
    Ok(Answer::Done)
}

/// The review `review` of the pull request `number` is shown: its reviewer's
/// request is answered, the pull request updated, and the review observed
/// and heard.
fn shown(model: &mut Model, env: &Env<Config>, id: Id<Repository>, number: u64, review: u64) {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    let item = repository.items.get_mut(&number).expect("a pull request reviewed");
    let pull = item.pull.as_mut().expect("a pull request");
    let mut shown = None;
    for kept in &pull.reviews {
        if kept.review.id == review {
            shown = Some(kept.review.clone());
        }
    }
    let shown = shown.expect("the review shown is kept");
    pull.requested.remove(&shown.author);
    repository.touch(number, model::clock(env));
    let observation = Observation::Reviewed {
        repository: copy_of(&repository.name),
        number,
        id: review,
        commit: shown.commit,
        verdict: shown.verdict,
        body: shown.body,
        by: shown.author,
    };
    model::changed(model, env, id, observation, Hook::item(Change::Review, number));
}

/// Merges the pull request `number` if its head is still `head`.
pub(crate) fn merge(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    head: u64,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    let item = repository.item(number)?;
    let Some(pull) = &item.pull else {
        return Err(Error::Missing(What::Pull));
    };
    if item.state == State::Closed {
        return Err(Error::Closed);
    }
    if pull.commit != head {
        return Err(Error::Stale);
    }
    if !repository.branches.contains_key(&*pull.head) {
        return Err(Error::Missing(What::Branch));
    }
    let Some(&onto) = repository.branches.get(&*pull.base) else {
        return Err(Error::Missing(What::Branch));
    };
    if !allowed(repository, item, pull) {
        return Err(Error::Protected);
    }
    if git::is_ancestor(model, head, onto) {
        return Err(Error::NothingToMerge);
    }
    let tree = merged(model, limits, head, onto)?;
    let base = copy_of(&pull.base);
    let commit = git::store(model, Object { parent: Some(onto), tree })?;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.has.insert(commit).expect("a repository has room for every commit");
    repository.branches.insert(copy_of(&base), commit).expect("the base branch is there");
    let item = repository.items.get_mut(&number).expect("the pull request merged");
    item.state = State::Closed;
    item.pull.as_mut().expect("a pull request").merged = Some(commit);
    repository.touch(number, model::clock(env));
    let observation = Observation::Merged {
        repository: copy_of(&repository.name),
        number,
        base: copy_of(&base),
        head,
        commit,
        by: user,
    };
    model::changed(model, env, id, observation, Hook::item(Change::Pull, number));
    git::moved(model, env, id, &base, Some(onto), commit, user);
    Ok(Answer::Merged(commit))
}

/// The pull request `number` as a read shows it.
pub(crate) fn view(model: &Model, limits: &Limits, repository: &Repository, number: u64) -> Result<Answer, Error> {
    let item = repository.item(number)?;
    let Some(pull) = &item.pull else {
        return Err(Error::Missing(What::Pull));
    };
    let base_commit = repository.branches.get(&*pull.base).copied();
    let mergeable = match base_commit {
        Some(onto) if item.state == State::Open && !git::is_ancestor(model, pull.commit, onto) => {
            merged(model, limits, pull.commit, onto).is_ok()
        }
        Some(_) | None => false,
    };
    let mut reviews = List::with_capacity(pull.reviews.len());
    for kept in &pull.reviews {
        if !kept.pending {
            reviews.push(kept.review.clone()).expect("a list as long as the reviews");
        }
    }
    let statuses = statuses(repository, limits, pull.commit);
    let reviewers = numbers(&pull.requested);
    Ok(Answer::Pull(PullView {
        number,
        state: item.state,
        head: copy_of(&pull.head),
        base: copy_of(&pull.base),
        commit: pull.commit,
        base_commit,
        merged: pull.merged,
        mergeable,
        reviewers,
        reviews: reviews.into_boxed(),
        statuses,
    }))
}

/// The latest status of each context on `commit`, in their contexts' order.
pub(crate) fn statuses(repository: &Repository, limits: &Limits, commit: u64) -> Box<[StatusView]> {
    let mut statuses = List::with_capacity(limits.contexts);
    if let Some(on) = repository.statuses.get(&commit) {
        for (context, status) in on {
            let view =
                StatusView { context: copy_of(context), state: status.state, author: status.author, at: status.at };
            statuses.push(view).expect("no more contexts than the limits");
        }
    }
    statuses.into_boxed()
}

/// Whether the base's protection, if it has one, lets the pull request
/// merge at its head: each required context passed on it, and enough
/// official approvals (by users who had write permission when they
/// reviewed, its author aside, in their last review that was not a
/// comment), of any head unless the protection dismisses stale ones, as
/// Forgejo counts them.
fn allowed(repository: &Repository, item: &Item, pull: &Pull) -> bool {
    let Some(protection) = &repository.protection else {
        return true;
    };
    if *protection.branch != *pull.base {
        return true;
    }
    for context in &protection.contexts {
        let passed = match repository.statuses.get(&pull.commit) {
            Some(statuses) => match statuses.get(&**context) {
                Some(status) => status.state == Check::Passed,
                None => false,
            },
            None => false,
        };
        if !passed {
            return false;
        }
    }
    let reviews = pull.reviews.as_slice();
    let mut approvals: u32 = 0;
    for (index, kept) in reviews.iter().enumerate() {
        let review = &kept.review;
        let stale = review.commit != pull.commit && protection.dismiss_stale;
        if kept.pending
            || review.verdict != Verdict::Approve
            || stale
            || review.author == item.author
            || !review.official
        {
            continue;
        }
        let mut last = true;
        for later in reviews.get(index.saturating_add(1)..).unwrap_or(&[]) {
            if !later.pending && later.review.author == review.author && later.review.verdict != Verdict::Comment {
                last = false;
            }
        }
        if last {
            approvals = approvals.saturating_add(1);
        }
    }
    approvals >= protection.approvals
}

/// The tree of `head` merged onto `onto`, three-way from the newest commit
/// they share, or why not: a conflict, or a tree past the limits.
fn merged(model: &Model, limits: &Limits, head: u64, onto: u64) -> Result<Tree, Error> {
    let empty = Map::with_capacity(0);
    let ancestor = match git::merge_base(model, head, onto) {
        Some(commit) => &model.commits.get(&commit).expect("a commit of the store").tree,
        None => &empty,
    };
    let ours = &model.commits.get(&onto).expect("a commit of the store").tree;
    let theirs = &model.commits.get(&head).expect("a commit of the store").tree;
    let mut tree = Map::with_capacity(limits.files);
    for side in [ancestor, ours, theirs] {
        for (path, _) in side {
            let was = ancestor.get(&**path);
            let base = ours.get(&**path);
            let changed = theirs.get(&**path);
            let kept = if changed == was || base == changed {
                base
            } else if base == was {
                changed
            } else {
                return Err(Error::Conflict);
            };
            let Some(content) = kept else {
                continue;
            };
            if tree.contains_key(&**path) {
                continue;
            }
            if tree.len() >= tree.capacity() {
                return Err(Error::TooLarge);
            }
            tree.insert(copy_of(path), copy_of(content)).expect("checked for room above");
        }
    }
    Ok(tree)
}
