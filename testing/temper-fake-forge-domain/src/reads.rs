//! The reads the engine needs: listings a page at a time, an item with its
//! comments, a pull request with its reviews and statuses, a user's
//! permission, a branch, a commit's tree or one file of it, and the wiki.
//! Reads change nothing; the caller checked the permission.
//!
//! Listing items is Forgejo's: those updated at or after `since`, both at
//! the forge's resolution (Forgejo's is a second), least recently updated
//! first and then by number, a page at a time by page number. Each page is
//! read from the order as it is when its call arrives; nothing holds the
//! order between pages. So what a pass over the pages finds is this: every
//! item that matched when the pass began and did not change during it,
//! unless an item listed before it changed meanwhile, moving to the end and
//! shifting the rest back by one, so that one item can fall on a page the
//! pass already read. An item that changes during the pass shows an updated
//! time no earlier than the pass's start, which is how a client knows the
//! pass moved under it. What moves an item's updated time is what moves it
//! on Forgejo: a new comment, a review, its labels, its title or body, a
//! push to a pull request's head, closing, reopening, merging; not a status
//! on its head, nor editing or deleting a comment (unless the configuration
//! says otherwise). A client that keeps up lists again from the newest
//! updated time it has seen, inclusive, drops what it has already seen at
//! that time, and lists again from the same `since` after a pass that moved
//! under it; CI it learns from status webhooks and by reading the statuses
//! of the heads it holds.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Id, List, Time};

use crate::api::{Answer, Error, File, Kind, Read, State, Summary, What};
use crate::domain::{self, Config, Domain};
use crate::limits::Limits;
use crate::store::{Repository, names, numbers};
use crate::{pulls, wiki};

/// What `read` answers on the repository `id`.
pub(crate) fn read(domain: &Domain, config: &Config, id: Id<Repository>, read: &Read) -> Result<Answer, Error> {
    let limits = &config.limits;
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    match read {
        Read::Items { state, kind, labels, author, since, page, limit } => {
            let (items, more) = items(repository, config, *state, *kind, labels, *author, *since, *page, *limit);
            Ok(Answer::Items { items, more, now: domain.clock })
        }
        Read::Item { number, after } => item(repository, limits, *number, *after),
        Read::Comment { id } => {
            let Some(&number) = repository.comments.get(id) else {
                return Err(Error::Missing(What::Comment));
            };
            let item = repository.item(number)?;
            let comment = item.comments.get(id).expect("the index names comments").view(*id);
            Ok(Answer::Comment { number, comment })
        }
        Read::Dependencies { number } => Ok(Answer::Dependencies(numbers(&repository.item(*number)?.dependencies))),
        Read::Labels => Ok(Answer::Labels(names(&repository.labels))),
        Read::Pull { number } => pulls::view(domain, limits, repository, *number),
        Read::PullFiles { number, page, limit } => {
            crate::next::pull_files(domain, limits, repository, *number, *page, *limit)
        }
        Read::Compare { base, head, page: _, limit: _ } => {
            crate::next::compare(domain, limits, repository, *base, *head)
        }
        Read::Checks { commit } => crate::next::checks(repository, limits, *commit, config.ci),
        Read::Job { commit, run, job, attempt, max_bytes } => {
            crate::next::job(repository, *commit, *run, *job, *attempt, *max_bytes, config.ci)
        }
        Read::Protection { branch } => {
            let protection = match &repository.protection {
                Some(protection) if protection.branch == *branch => Some(protection.clone()),
                Some(_) | None => None,
            };
            Ok(Answer::Protection(protection))
        }
        Read::Settings => Ok(crate::next::settings(repository)),
        Read::Collaborators => Ok(crate::next::collaborators(repository)),
        Read::PullFor { head, base } => match repository.newest_pull(head, base) {
            Some(number) => pulls::view(domain, limits, repository, number),
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
            let tree = &domain.commits.get(commit).expect("a commit of the store").tree;
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
            match domain.commits.get(commit).expect("a commit of the store").tree.get(&**path) {
                Some(content) => Ok(Answer::File(copy_of(content))),
                None => Err(Error::Missing(What::File)),
            }
        }
        Read::Pages { after } => Ok(wiki::pages(repository, limits, after.as_deref())),
        Read::Page { name } => wiki::page(repository, name),
    }
}

/// A page of the items that match, least recently updated first.
#[expect(clippy::too_many_arguments, reason = "a listing takes the read's filters as they come")]
fn items(
    repository: &Repository,
    config: &Config,
    state: Option<State>,
    kind: Option<Kind>,
    labels: &[Box<[u8]>],
    author: Option<u64>,
    since: Time,
    page: u32,
    limit: u32,
) -> (Box<[Summary]>, bool) {
    let size = if limit == 0 { config.limits.page_size } else { limit.min(config.limits.page_size) };
    let since = domain::stamp(config, since);
    // Forgejo takes a page before the first as the first.
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(size));
    let mut skipped: u64 = 0;
    let mut items = List::with_capacity(size);
    let mut more = false;
    for &(updated, number) in &repository.recent {
        if updated < since {
            continue;
        }
        let item = repository.items.get(&number).expect("the listing index names items");
        let wanted = match state {
            Some(state) => item.state == state,
            None => true,
        } && match kind {
            Some(kind) => item.kind() == kind,
            None => true,
        } && match author {
            Some(author) => item.author == author,
            None => true,
        } && item.has_labels(labels);
        if !wanted {
            continue;
        }
        if skipped < skip {
            skipped = skipped.saturating_add(1);
            continue;
        }
        if items.room() == 0 {
            more = true;
            break;
        }
        items.push(item.summary(number)).expect("checked for room above");
    }
    (items.into_boxed(), more)
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
