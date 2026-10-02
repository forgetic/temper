//! Writes to items that issues and pull requests share: opening an issue,
//! comments, labels, closing and reopening. Each refuses what Forgejo refuses:
//! too little permission, someone else's comment for a user who may not
//! write, a label not defined, an empty title or comment, what is past the
//! limits.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Map, Set};

use crate::api::{Answer, Change, Error, Kind, Permission, State, What};
use crate::ci;
use crate::model::{self, Config, Model};
use crate::observe::Observation;
use crate::store::{Comment, Item, Repository, fit_names, fits, names};

/// Opens an issue carrying `labels`.
pub(crate) fn create(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    title: Box<[u8]>,
    body: Box<[u8]>,
    labels: Box<[Box<[u8]>]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&title, limits.title_bytes)?;
    fits(&body, limits.body_bytes)?;
    fit_names(&labels, limits.labels, limits)?;
    if title.is_empty() {
        return Err(Error::Empty);
    }
    repository.room()?;
    // As Forgejo's API does, the labels of a user who may not label are
    // dropped, not refused.
    let labels = if repository.permission(user) >= Permission::Write {
        repository.label_set(limits, labels)?
    } else {
        Set::with_capacity(limits.labels)
    };
    let item = Item {
        title,
        body,
        author: user,
        state: State::Open,
        labels,
        comments: Map::with_capacity(limits.comments),
        created: model::clock(env),
        updated: model::clock(env),
        pull: None,
    };
    let number = repository.number(item);
    let item = repository.items.get(&number).expect("the item just opened");
    let observation = Observation::Opened {
        repository: copy_of(&repository.name),
        number,
        kind: Kind::Issue,
        title: copy_of(&item.title),
        body: copy_of(&item.body),
        labels: names(&item.labels),
        by: user,
    };
    model::changed(model, env, id, observation, Change::Issue, Some(number));
    Ok(Answer::Created(number))
}

/// Comments on the item `number`.
pub(crate) fn comment(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    body: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&body, env.limits.limits.body_bytes)?;
    if body.is_empty() {
        return Err(Error::Empty);
    }
    let comment = model.comments.checked_add(1).expect("comment ids do not run out");
    let observed = copy_of(&body);
    let item = repository.item_mut(number)?;
    if item.comments.insert(comment, Comment { author: user, body, created: model::clock(env), edited: None }).is_err()
    {
        return Err(Error::Full);
    }
    repository.comments.insert(comment, number).expect("the index has room for every comment");
    repository.touch(number, model::clock(env));
    model.comments = comment;
    let observation =
        Observation::Commented { repository: copy_of(&repository.name), number, id: comment, body: observed, by: user };
    model::changed(model, env, id, observation, Change::Comment, Some(number));
    Ok(Answer::Commented(comment))
}

/// Edits the comment `comment`: the user's own, or anyone's for a user
/// with write permission, as Forgejo's web interface lets people.
pub(crate) fn edit(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    comment: u64,
    body: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&body, env.limits.limits.body_bytes)?;
    if body.is_empty() {
        return Err(Error::Empty);
    }
    let number = *repository.comments.get(&comment).ok_or(Error::Missing(What::Comment))?;
    let writer = repository.permission(user) >= Permission::Write;
    let item = repository.items.get_mut(&number).expect("the index names items");
    let kept = item.comments.get_mut(&comment).expect("the index names comments");
    if kept.author != user && !writer {
        return Err(Error::Forbidden);
    }
    let observed = copy_of(&body);
    kept.body = body;
    kept.edited = Some(model::clock(env));
    if env.limits.edit_updates {
        repository.touch(number, model::clock(env));
    }
    let observation =
        Observation::Edited { repository: copy_of(&repository.name), number, id: comment, body: observed, by: user };
    model::changed(model, env, id, observation, Change::Comment, Some(number));
    Ok(Answer::Done)
}

/// Deletes the comment `comment`: the user's own, or anyone's for a user
/// with write permission.
pub(crate) fn remove(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    comment: u64,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    let number = *repository.comments.get(&comment).ok_or(Error::Missing(What::Comment))?;
    let writer = repository.permission(user) >= Permission::Write;
    let item = repository.items.get_mut(&number).expect("the index names items");
    let author = item.comments.get(&comment).expect("the index names comments").author;
    if author != user && !writer {
        return Err(Error::Forbidden);
    }
    item.comments.remove(&comment);
    repository.comments.remove(&comment);
    if env.limits.edit_updates {
        repository.touch(number, model::clock(env));
    }
    let observation = Observation::Removed { repository: copy_of(&repository.name), number, id: comment, by: user };
    model::changed(model, env, id, observation, Change::Comment, Some(number));
    Ok(Answer::Done)
}

/// Makes the labels of the item `number` exactly `labels`.
pub(crate) fn label(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    labels: Box<[Box<[u8]>]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    repository.item(number)?;
    fit_names(&labels, limits.labels, limits)?;
    let labels = repository.label_set(limits, labels)?;
    let observed = names(&labels);
    let item = repository.item_mut(number)?;
    item.labels = labels;
    let change = change(item.kind());
    repository.touch(number, model::clock(env));
    let observation =
        Observation::Labelled { repository: copy_of(&repository.name), number, labels: observed, by: user };
    model::changed(model, env, id, observation, change, Some(number));
    Ok(Answer::Done)
}

/// Defines the label `name`.
pub(crate) fn define(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    name: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(&name, env.limits.limits.name_bytes)?;
    let observed = copy_of(&name);
    match repository.labels.insert(name) {
        Ok(true) => {}
        Ok(false) => return Err(Error::Exists),
        Err(_) => return Err(Error::Full),
    }
    let observation = Observation::Defined { repository: copy_of(&repository.name), label: observed, by: user };
    model.observations.push(observation);
    Ok(Answer::Done)
}

/// Closes the item `number`: the user's own, or any with write permission.
/// Closing a closed item changes nothing.
pub(crate) fn close(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    may_change(repository, user, number)?;
    if repository.item(number)?.state == State::Closed {
        return Ok(Answer::Done);
    }
    shut(model, env, id, number, user);
    Ok(Answer::Done)
}

/// Closes the open item `number`, as `by`.
pub(crate) fn shut(model: &mut Model, env: &Env<Config>, id: Id<Repository>, number: u64, by: u64) {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    let item = repository.items.get_mut(&number).expect("an item to close");
    assert!(item.state == State::Open, "only an open item is closed");
    item.state = State::Closed;
    let change = change(item.kind());
    repository.touch(number, model::clock(env));
    let observation = Observation::Closed { repository: copy_of(&repository.name), number, by };
    model::changed(model, env, id, observation, change, Some(number));
}

/// Reopens the item `number`: the user's own, or any with write permission.
/// A pull request merged stays closed; one reopened follows its head branch
/// again, which must be there. Reopening an open item changes nothing.
pub(crate) fn reopen(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    may_change(repository, user, number)?;
    let item = repository.item(number)?;
    if item.state == State::Open {
        return Ok(Answer::Done);
    }
    let head = match &item.pull {
        Some(pull) => {
            if pull.merged.is_some() {
                return Err(Error::Closed);
            }
            let Some(&head) = repository.branches.get(&*pull.head) else {
                return Err(Error::Missing(What::Branch));
            };
            if repository.open_pull(&pull.head, &pull.base).is_some() {
                return Err(Error::Exists);
            }
            Some(head)
        }
        None => None,
    };
    let item = repository.item_mut(number)?;
    item.state = State::Open;
    if let Some(pull) = &mut item.pull
        && let Some(head) = head
    {
        pull.commit = head;
    }
    let change = change(item.kind());
    repository.touch(number, model::clock(env));
    let observation = Observation::Reopened { repository: copy_of(&repository.name), number, by: user };
    model::changed(model, env, id, observation, change, Some(number));
    if let Some(head) = head {
        ci::start(model, env, id, head);
    }
    Ok(Answer::Done)
}

/// Refuses `user` changing the item `number` unless it is theirs, or they
/// have write permission.
fn may_change(repository: &Repository, user: u64, number: u64) -> Result<(), Error> {
    repository.require(user, Permission::Read)?;
    if repository.item(number)?.author != user {
        repository.require(user, Permission::Write)?;
    }
    Ok(())
}

/// What a webhook calls a change to an item of `kind`.
pub(crate) fn change(kind: Kind) -> Change {
    match kind {
        Kind::Issue => Change::Issue,
        Kind::Pull => Change::Pull,
    }
}
