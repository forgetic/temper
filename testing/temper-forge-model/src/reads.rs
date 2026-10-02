//! The reads the engine needs: listings a page at a time, an item with its
//! comments, a pull request with its reviews and statuses, a user's
//! permission, a branch, a commit's tree or one file of it, and the wiki.
//! Reads change nothing; the caller checked the permission.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Id, List, Time};

use crate::api::{Answer, Cursor, Error, File, Kind, Read, State, What};
use crate::limits::Limits;
use crate::model::Model;
use crate::store::Repository;
use crate::{pulls, wiki};

/// What `read` answers on the repository `id`.
pub(crate) fn read(model: &Model, limits: &Limits, id: Id<Repository>, read: &Read) -> Result<Answer, Error> {
    let repository = model.repositories.get(id).expect("a repository of the forge");
    match read {
        Read::Items { state, kind, labels, since, after } => {
            Ok(items(repository, limits, *state, *kind, labels, *since, *after))
        }
        Read::Item { number, after } => item(repository, limits, *number, *after),
        Read::Pull { number } => pulls::view(model, limits, repository, *number),
        Read::PullFor { head, base } => match repository.newest_pull(head, base) {
            Some(number) => pulls::view(model, limits, repository, number),
            None => Err(Error::Missing(What::Pull)),
        },
        Read::Statuses { commit } => {
            if !repository.has.contains(commit) {
                return Err(Error::Missing(What::Commit));
            }
            Ok(Answer::Statuses(pulls::statuses(repository, limits, *commit)))
        }
        Read::Permission { user } => Ok(Answer::Permission(repository.permission(*user))),
        Read::Branch { branch } => match repository.branches.get(&**branch) {
            Some(&commit) => Ok(Answer::Commit(commit)),
            None => Err(Error::Missing(What::Branch)),
        },
        Read::Tree { commit } => {
            if !repository.has.contains(commit) {
                return Err(Error::Missing(What::Commit));
            }
            let tree = &model.commits.get(commit).expect("a commit of the store").tree;
            let mut files = List::with_capacity(tree.len());
            for (path, content) in tree {
                files
                    .push(File { path: copy_of(path), content: copy_of(content) })
                    .expect("a list as long as the tree");
            }
            Ok(Answer::Tree(files.into_boxed()))
        }
        Read::File { commit, path } => {
            if !repository.has.contains(commit) {
                return Err(Error::Missing(What::Commit));
            }
            match model.commits.get(commit).expect("a commit of the store").tree.get(&**path) {
                Some(content) => Ok(Answer::File(copy_of(content))),
                None => Err(Error::Missing(What::File)),
            }
        }
        Read::Pages { after } => Ok(wiki::pages(repository, limits, after.as_deref())),
        Read::Page { name } => wiki::page(repository, name),
    }
}

/// A page of the items that match, in the order they were last updated.
fn items(
    repository: &Repository,
    limits: &Limits,
    state: Option<State>,
    kind: Option<Kind>,
    labels: &[Box<[u8]>],
    since: Time,
    after: Option<Cursor>,
) -> Answer {
    let mut items = List::with_capacity(limits.page_size);
    let mut last = None;
    let mut next = None;
    for &(updated, number) in &repository.recent {
        let cursor = Cursor { updated, number };
        if updated < since {
            continue;
        }
        if let Some(after) = after
            && cursor <= after
        {
            continue;
        }
        let item = repository.items.get(&number).expect("the listing index names items");
        let wanted = match state {
            Some(state) => item.state == state,
            None => true,
        } && match kind {
            Some(kind) => item.kind() == kind,
            None => true,
        } && item.has_labels(labels);
        if !wanted {
            continue;
        }
        if items.room() == 0 {
            next = last;
            break;
        }
        items.push(item.summary(number)).expect("checked for room above");
        last = Some(cursor);
    }
    Answer::Items { items: items.into_boxed(), next }
}

/// The item `number` and a page of its comments with ids above `after`.
fn item(repository: &Repository, limits: &Limits, number: u64, after: u64) -> Result<Answer, Error> {
    let item = repository.item(number)?;
    let mut comments = List::with_capacity(limits.page_size);
    let mut more = false;
    for (&id, comment) in &item.comments {
        if id <= after {
            continue;
        }
        if comments.room() == 0 {
            more = true;
            break;
        }
        comments.push(comment.view(id)).expect("checked for room above");
    }
    Ok(Answer::Item { item: item.summary(number), comments: comments.into_boxed(), more })
}
