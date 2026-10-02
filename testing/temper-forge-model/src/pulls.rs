//! Pull requests: opening one, reviewing it, merging it, and how a read shows
//! it.
//!
//! A pull request's head commit follows its head branch while it is open. A
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
use crate::limits::Limits;
use crate::model::{self, Config, Model};
use crate::observe::Observation;
use crate::store::{Item, Pull, Repository, fits};

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
    let pull = Pull { head, base, commit, merged: None, reviews: List::with_capacity(limits.reviews) };
    let item = Item {
        title,
        body,
        author: user,
        state: State::Open,
        labels: Set::with_capacity(limits.labels),
        comments: Map::with_capacity(limits.comments),
        created: model::clock(env),
        updated: model::clock(env),
        pull: Some(pull),
    };
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    let number = repository.number(item)?;
    let item = repository.items.get(&number).expect("the pull request just opened");
    let observation = Observation::Opened {
        repository: copy_of(&repository.name),
        number,
        kind: Kind::Pull,
        title: copy_of(&item.title),
        body: copy_of(&item.body),
        labels: Box::new([]),
        by: user,
    };
    model::changed(model, env, id, observation, Change::Pull, Some(number));
    ci::start(model, env, id, commit);
    Ok(Answer::Created(number))
}

/// Reviews the open pull request `number` at its head. Its author may only
/// comment.
pub(crate) fn review(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    verdict: Verdict,
    body: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&body, env.limits.limits.body_bytes)?;
    let item = repository.item_mut(number)?;
    let author = item.author;
    let state = item.state;
    let Some(pull) = &mut item.pull else {
        return Err(Error::Missing(What::Pull));
    };
    if state == State::Closed {
        return Err(Error::Closed);
    }
    if author == user && verdict != Verdict::Comment {
        return Err(Error::Forbidden);
    }
    let commit = pull.commit;
    let observed = copy_of(&body);
    if pull.reviews.push(Review { author: user, verdict, commit, body, at: model::clock(env) }).is_err() {
        return Err(Error::Full);
    }
    repository.touch(number, model::clock(env));
    let observation = Observation::Reviewed {
        repository: copy_of(&repository.name),
        number,
        commit,
        verdict,
        body: observed,
        by: user,
    };
    model::changed(model, env, id, observation, Change::Review, Some(number));
    Ok(Answer::Done)
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
    let observation = Observation::Merged { repository: copy_of(&repository.name), number, head, commit, by: user };
    model::changed(model, env, id, observation, Change::Pull, Some(number));
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
    for review in &pull.reviews {
        reviews.push(review.clone()).expect("a list as long as the reviews");
    }
    let statuses = statuses(repository, limits, pull.commit);
    Ok(Answer::Pull(PullView {
        number,
        state: item.state,
        head: copy_of(&pull.head),
        base: copy_of(&pull.base),
        commit: pull.commit,
        base_commit,
        merged: pull.merged,
        mergeable,
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
/// merge at its head: each required context passed on it, and enough users
/// with write permission, its author aside, approving it in their last
/// review that was not a comment.
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
    for (index, review) in reviews.iter().enumerate() {
        if review.verdict != Verdict::Approve
            || review.commit != pull.commit
            || review.author == item.author
            || repository.permission(review.author) < Permission::Write
        {
            continue;
        }
        let mut last = true;
        for later in reviews.get(index.saturating_add(1)..).unwrap_or(&[]) {
            if later.author == review.author && later.verdict != Verdict::Comment {
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
