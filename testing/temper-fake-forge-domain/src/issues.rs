//! Writes to items that issues and pull requests share: opening an issue,
//! comments, labels, closing and reopening. Each refuses what Forgejo refuses:
//! too little permission, someone else's comment for a user who may not
//! write, a label not defined, an empty title or comment, what is past the
//! limits.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Env, Id, Map, Set};

use crate::api::{Answer, Change, Error, Kind, Permission, State, What};
use crate::ci;
use crate::domain::{self, Config, Domain};
use crate::hooks::Hook;
use crate::observe::Observation;
use crate::store::{Comment, Item, Repository, fit_names, fit_numbers, fits, names, numbers};

/// Opens an issue carrying `labels`.
pub(crate) fn create(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    title: Box<[u8]>,
    body: Box<[u8]>,
    labels: Box<[Box<[u8]>]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
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
        dependencies: Set::with_capacity(limits.dependencies),
        created: domain::clock(env),
        updated: domain::clock(env),
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
        branches: None,
        by: user,
    };
    domain::changed(domain, env, id, observation, Hook::item(Change::Issue, number, user));
    Ok(Answer::Created(number))
}

/// Comments on the item `number`.
pub(crate) fn comment(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    body: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Read)?;
    fits(&body, env.limits.limits.body_bytes)?;
    if body.is_empty() {
        return Err(Error::Empty);
    }
    let comment = domain.comments.checked_add(1).expect("comment ids do not run out");
    let observed = copy_of(&body);
    let item = repository.item_mut(number)?;
    if item.comments.insert(comment, Comment { author: user, body, created: domain::clock(env), edited: None }).is_err()
    {
        return Err(Error::Full);
    }
    repository.comments.insert(comment, number).expect("the index has room for every comment");
    repository.touch(number, domain::clock(env));
    domain.comments = comment;
    let observation =
        Observation::Commented { repository: copy_of(&repository.name), number, id: comment, body: observed, by: user };
    domain::changed(domain, env, id, observation, Hook::item(Change::Comment, number, user));
    Ok(Answer::Commented(comment))
}

/// Edits the title, the body, or both, of the item `number`: the user's
/// own, or any with write permission.
pub(crate) fn revise(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    title: Option<Box<[u8]>>,
    body: Option<Box<[u8]>>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    may_change(repository, user, number)?;
    if let Some(title) = &title {
        fits(title, limits.title_bytes)?;
        if title.is_empty() {
            return Err(Error::Empty);
        }
    }
    if let Some(body) = &body {
        fits(body, limits.body_bytes)?;
    }
    let item = repository.items.get_mut(&number).expect("an item to edit");
    if let Some(title) = title {
        item.title = title;
    }
    if let Some(body) = body {
        item.body = body;
    }
    let change = change(item.kind());
    let observation = Observation::Revised {
        repository: copy_of(&repository.name),
        number,
        title: copy_of(&item.title),
        body: copy_of(&item.body),
        by: user,
    };
    repository.touch(number, domain::clock(env));
    domain::changed(domain, env, id, observation, Hook::item(change, number, user));
    Ok(Answer::Done)
}

/// Makes the items the item `number` depends on exactly `dependencies`.
pub(crate) fn depend(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    dependencies: Box<[u64]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    repository.item(number)?;
    fit_numbers(&dependencies, limits.dependencies)?;
    let mut set = Set::with_capacity(limits.dependencies);
    for &dependency in &dependencies {
        if dependency == number || repository.item(dependency)?.dependencies.contains(&number) {
            return Err(Error::Circular);
        }
        if set.insert(dependency).is_err() {
            return Err(Error::TooLarge);
        }
    }
    let observed = numbers(&set);
    let item = repository.item_mut(number)?;
    item.dependencies = set;
    let change = change(item.kind());
    repository.touch(number, domain::clock(env));
    let observation =
        Observation::Depends { repository: copy_of(&repository.name), number, dependencies: observed, by: user };
    domain::changed(domain, env, id, observation, Hook::item(change, number, user));
    Ok(Answer::Done)
}

/// Edits the comment `comment`: the user's own, or anyone's for a user
/// with write permission, as Forgejo's web interface lets people.
pub(crate) fn edit(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    comment: u64,
    body: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
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
    kept.edited = Some(domain::clock(env));
    if env.limits.edit_updates {
        repository.touch(number, domain::clock(env));
    }
    let observation =
        Observation::Edited { repository: copy_of(&repository.name), number, id: comment, body: observed, by: user };
    domain::changed(domain, env, id, observation, Hook::item(Change::Comment, number, user));
    Ok(Answer::Done)
}

/// Deletes the comment `comment`: the user's own, or anyone's for a user
/// with write permission.
pub(crate) fn remove(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    comment: u64,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
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
        repository.touch(number, domain::clock(env));
    }
    let observation = Observation::Removed { repository: copy_of(&repository.name), number, id: comment, by: user };
    domain::changed(domain, env, id, observation, Hook::item(Change::Comment, number, user));
    Ok(Answer::Done)
}

/// Makes the labels of the item `number` exactly `labels`.
pub(crate) fn label(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    labels: Box<[Box<[u8]>]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    repository.item(number)?;
    fit_names(&labels, limits.labels, limits)?;
    let labels = repository.label_set(limits, labels)?;
    let observed = names(&labels);
    let item = repository.item_mut(number)?;
    item.labels = labels;
    let change = change(item.kind());
    repository.touch(number, domain::clock(env));
    let observation =
        Observation::Labelled { repository: copy_of(&repository.name), number, labels: observed, by: user };
    domain::changed(domain, env, id, observation, Hook::item(change, number, user));
    Ok(Answer::Done)
}

/// Adds `labels` to the item `number`, leaving those it carries. Adding
/// what it carries already changes nothing.
pub(crate) fn add_labels(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    labels: Box<[Box<[u8]>]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    repository.item(number)?;
    fit_names(&labels, limits.labels, limits)?;
    let added = repository.label_set(limits, labels)?;
    let item = repository.item_mut(number)?;
    let mut changed = false;
    for label in &added {
        if !item.labels.contains(&**label) {
            if item.labels.insert(copy_of(label)).is_err() {
                return Err(Error::TooLarge);
            }
            changed = true;
        }
    }
    if changed {
        relabelled(domain, env, id, user, number);
    }
    Ok(Answer::Done)
}

/// Removes `labels` from the item `number`, those it does not carry aside.
/// Removing what it does not carry changes nothing.
pub(crate) fn remove_labels(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    labels: Box<[Box<[u8]>]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    repository.item(number)?;
    fit_names(&labels, limits.labels, limits)?;
    let item = repository.item_mut(number)?;
    let mut changed = false;
    for label in &labels {
        if item.labels.remove(&**label) {
            changed = true;
        }
    }
    if changed {
        relabelled(domain, env, id, user, number);
    }
    Ok(Answer::Done)
}

/// The labels of the item `number` changed, as `user`: it is updated, and
/// the change observed and heard.
fn relabelled(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, user: u64, number: u64) {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    let item = repository.items.get(&number).expect("an item relabelled");
    let observed = names(&item.labels);
    let change = change(item.kind());
    repository.touch(number, domain::clock(env));
    let observation =
        Observation::Labelled { repository: copy_of(&repository.name), number, labels: observed, by: user };
    domain::changed(domain, env, id, observation, Hook::item(change, number, user));
}

/// Defines the label `name`.
pub(crate) fn define(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    name: Box<[u8]>,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(&name, env.limits.limits.name_bytes)?;
    let observed = copy_of(&name);
    match repository.labels.insert(name) {
        Ok(true) => {}
        Ok(false) => return Err(Error::Exists),
        Err(_) => return Err(Error::Full),
    }
    let observation = Observation::Defined { repository: copy_of(&repository.name), label: observed, by: user };
    domain.observations.push(observation);
    Ok(Answer::Done)
}

/// Closes the item `number`: the user's own, or any with write permission.
/// Closing a closed item changes nothing.
pub(crate) fn close(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    may_change(repository, user, number)?;
    if repository.item(number)?.state == State::Closed {
        return Ok(Answer::Done);
    }
    shut(domain, env, id, number, user);
    Ok(Answer::Done)
}

/// Closes the open item `number`, as `by`.
pub(crate) fn shut(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, number: u64, by: u64) {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    let item = repository.items.get_mut(&number).expect("an item to close");
    assert!(item.state == State::Open, "only an open item is closed");
    item.state = State::Closed;
    let change = change(item.kind());
    repository.touch(number, domain::clock(env));
    let observation = Observation::Closed { repository: copy_of(&repository.name), number, by };
    domain::changed(domain, env, id, observation, Hook::item(change, number, by));
}

/// Reopens the item `number`: the user's own, or any with write permission.
/// A pull request merged stays closed; one reopened follows its head branch
/// again, which must be there. Reopening an open item changes nothing.
pub(crate) fn reopen(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
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
    repository.touch(number, domain::clock(env));
    let observation = Observation::Reopened { repository: copy_of(&repository.name), number, by: user };
    domain::changed(domain, env, id, observation, Hook::item(change, number, user));
    if let Some(head) = head {
        ci::start(domain, env, id, head);
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
