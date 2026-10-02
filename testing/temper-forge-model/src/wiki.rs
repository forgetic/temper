//! Wikis: a repository's pages by name, each with its content, its author and
//! a revision that grows with every write to the wiki (engine-model.md,
//! section 10: notes live here). Listing gives names and revisions a page at
//! a time; writing needs write permission.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, List};

use crate::api::{Answer, Error, Page as PageView, PageName, Permission, What};
use crate::hooks::Hook;
use crate::limits::Limits;
use crate::model::{self, Config, Model};
use crate::observe::Observation;
use crate::store::{Page, Repository, fits};

/// Creates or replaces the page `name`.
pub(crate) fn put(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    name: Box<[u8]>,
    content: Box<[u8]>,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(&name, limits.name_bytes)?;
    fits(&content, limits.content_bytes)?;
    let revision = repository.revisions.checked_add(1).expect("revisions do not run out");
    let observed = copy_of(&content);
    let observation = Observation::Wiki {
        repository: copy_of(&repository.name),
        name: copy_of(&name),
        content: Some(observed),
        revision,
        by: user,
    };
    if repository.wiki.insert(name, Page { content, author: user, revision }).is_err() {
        return Err(Error::Full);
    }
    repository.revisions = revision;
    model::changed(model, env, id, observation, Hook::wiki());
    Ok(Answer::Revision(revision))
}

/// Deletes the page `name`.
pub(crate) fn delete(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    name: &[u8],
) -> Result<Answer, Error> {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    if repository.wiki.remove(name).is_none() {
        return Err(Error::Missing(What::Page));
    }
    let revision = repository.revisions.checked_add(1).expect("revisions do not run out");
    repository.revisions = revision;
    let observation = Observation::Wiki {
        repository: copy_of(&repository.name),
        name: copy_of(name),
        content: None,
        revision,
        by: user,
    };
    model::changed(model, env, id, observation, Hook::wiki());
    Ok(Answer::Done)
}

/// A page of the wiki's page names, in their order, from after `after`.
pub(crate) fn pages(repository: &Repository, limits: &Limits, after: Option<&[u8]>) -> Answer {
    let mut pages: List<PageName> = List::with_capacity(limits.page_size);
    let mut next = None;
    for (name, page) in &repository.wiki {
        if let Some(after) = after
            && **name <= *after
        {
            continue;
        }
        if pages.room() == 0 {
            let Some(last) = pages.last() else {
                break;
            };
            next = Some(copy_of(&last.name));
            break;
        }
        pages.push(PageName { name: copy_of(name), revision: page.revision }).expect("checked for room above");
    }
    Answer::Pages { pages: pages.into_boxed(), next }
}

/// The page `name`.
pub(crate) fn page(repository: &Repository, name: &[u8]) -> Result<Answer, Error> {
    match repository.wiki.get(name) {
        Some(page) => Ok(Answer::Page(PageView {
            name: copy_of(name),
            content: copy_of(&page.content),
            author: page.author,
            revision: page.revision,
        })),
        None => Err(Error::Missing(What::Page)),
    }
}
