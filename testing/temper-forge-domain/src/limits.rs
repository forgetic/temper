//! What the forge holds at most (programming-model.md, section 7), and the
//! memory that takes (6.3).

use alloc::boxed::Box;
use core::mem::size_of;

use temper_lib::{Deadlines, Id, List, Map, Queue, Set, Slab, Time};

use crate::api::{Comment, File, Head, PageName, Permission, Review, Status as StatusView, Summary};
use crate::domain::{Alarm, Call};
use crate::faults::Window;
use crate::git::Object;
use crate::hooks::Delivery;
use crate::observe::Observation;
use crate::store::{Comment as Posted, Item, Kept, Page, Repository, Status};

/// The forge's limits, part of its configuration. What would pass one is
/// refused: a name, title, body or content too long as too large, and one
/// more of anything a repository holds as full.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Repositories on the forge.
    pub repositories: u32,
    /// Users with a permission on a repository, and users calling within a
    /// rate window.
    pub users: u32,
    /// Labels defined in a repository.
    pub labels: u32,
    /// Items in a repository; comments on an item, and items it depends on.
    pub items: u32,
    pub comments: u32,
    pub dependencies: u32,
    /// Reviews of a pull request.
    pub reviews: u32,
    /// Branches of a repository.
    pub branches: u32,
    /// Commits in the forge's store, and files in a commit's tree.
    pub commits: u32,
    pub files: u32,
    /// Commits of a repository with statuses, and contexts on each.
    pub statuses: u32,
    pub contexts: u32,
    /// Wiki pages of a repository.
    pub pages: u32,
    /// The most bytes of a name: a repository's, a branch's, a label's, a
    /// context's, a wiki page's, or a path.
    pub name_bytes: u32,
    pub title_bytes: u32,
    /// The most bytes of an item's, a comment's or a review's body.
    pub body_bytes: u32,
    /// The most bytes of a file's content, or a wiki page's.
    pub content_bytes: u32,
    /// The most entries a listing answers at once: items, comments, or wiki
    /// page names.
    pub page_size: u32,
    /// Calls held at once, until they are answered and reclaimed. A call
    /// beyond them is answered at once as unavailable.
    pub calls: u32,
    /// Webhook deliveries in flight. A change beyond them is not delivered.
    pub hooks: u32,
    /// Observations kept until a world drains them. Beyond them, they are
    /// dropped and counted.
    pub observations: u32,
}

/// The timers armed at most: one per call, one per delivery, and one per
/// context of each commit with statuses.
pub(crate) fn timers(limits: &Limits) -> Option<u32> {
    let checks = limits.repositories.checked_mul(limits.statuses)?.checked_mul(limits.contexts)?;
    limits.calls.checked_add(limits.hooks)?.checked_add(checks)
}

/// The comments a repository holds at most.
pub(crate) fn comments(limits: &Limits) -> Option<u32> {
    limits.items.checked_mul(limits.comments)
}

/// The most memory the forge holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, the payloads, the
/// answers held until they go out, and the scratch a merge takes, not
/// allocator overhead. What a call brings is moved into the store or dropped
/// in the step it arrives in.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let repositories = Slab::<Repository>::worst_case(limits.repositories)?
        .checked_add(Map::<Box<[u8]>, Id<Repository>>::worst_case(limits.repositories)?)?
        .checked_add(times(limits.repositories, repository(limits)?)?)?;
    let commits = Map::<u64, Object>::worst_case(limits.commits)?.checked_add(times(limits.commits, tree(limits)?)?)?;
    // A call holds its answer until it goes out, or what it brought until it
    // lands, late.
    let held = answer(limits)?.max(call(limits)?);
    let calls = Slab::<Call>::worst_case(limits.calls)?.checked_add(times(limits.calls, held)?)?;
    // A webhook about a push names its branch.
    let hooks =
        Slab::<Delivery>::worst_case(limits.hooks)?.checked_add(times(limits.hooks, u64::from(limits.name_bytes))?)?;
    let windows = Map::<u64, Window>::worst_case(limits.users)?;
    let timers = Deadlines::<Alarm>::worst_case(timers(limits)?)?;
    let observations = Queue::<Observation>::worst_case(limits.observations)?
        .checked_add(times(limits.observations, observation(limits)?)?)?;
    // A merge walks the base's history into a set, and makes a tree it may
    // refuse; a branch moving or a status reported lists the pull requests
    // that follow it; what a call brings is held until it is stored or
    // dropped; a write replaces what an item held, an observation is made
    // before a full queue drops it.
    let scratch = Set::<u64>::worst_case(limits.commits)?
        .checked_add(tree(limits)?)?
        .checked_add(List::<u64>::worst_case(limits.items)?)?
        .checked_add(call(limits)?)?
        .checked_add(item(limits)?)?
        .checked_add(observation(limits)?)?;
    repositories
        .checked_add(commits)?
        .checked_add(calls)?
        .checked_add(hooks)?
        .checked_add(windows)?
        .checked_add(timers)?
        .checked_add(observations)?
        .checked_add(scratch)
}

/// What one repository owns beyond its slot.
fn repository(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let comments = comments(limits)?;
    // Its name twice (the index's key too), its default branch, and its
    // setup: the contexts of its checks and its protection, the cue's path
    // and what it holds, the protected branch.
    let setup = name
        .checked_mul(5)?
        .checked_add(names(limits.contexts, limits)?.checked_mul(2)?)?
        .checked_add(u64::from(limits.content_bytes))?;
    let permissions = Map::<u64, Permission>::worst_case(limits.users)?;
    let labels = Set::<Box<[u8]>>::worst_case(limits.labels)?.checked_add(times(limits.labels, name)?)?;
    let items = Map::<u64, Item>::worst_case(limits.items)?
        .checked_add(Set::<(Time, u64)>::worst_case(limits.items)?)?
        .checked_add(Map::<u64, u64>::worst_case(comments)?)?
        .checked_add(times(limits.items, item(limits)?)?)?;
    let branches = Map::<Box<[u8]>, u64>::worst_case(limits.branches)?.checked_add(times(limits.branches, name)?)?;
    let has = Set::<u64>::worst_case(limits.commits)?;
    let contexts = Map::<Box<[u8]>, Status>::worst_case(limits.contexts)?.checked_add(times(limits.contexts, name)?)?;
    let statuses = Map::<u64, Map<Box<[u8]>, Status>>::worst_case(limits.statuses)?
        .checked_add(times(limits.statuses, contexts)?)?;
    let page = name.checked_add(u64::from(limits.content_bytes))?;
    let wiki = Map::<Box<[u8]>, Page>::worst_case(limits.pages)?.checked_add(times(limits.pages, page)?)?;
    setup
        .checked_add(permissions)?
        .checked_add(labels)?
        .checked_add(items)?
        .checked_add(branches)?
        .checked_add(has)?
        .checked_add(statuses)?
        .checked_add(wiki)
}

/// What one item owns beyond its entry: its text, its labels, its comments,
/// and a pull request's branches and reviews.
fn item(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let body = u64::from(limits.body_bytes);
    let labels = Set::<Box<[u8]>>::worst_case(limits.labels)?.checked_add(times(limits.labels, name)?)?;
    let comments = Map::<u64, Posted>::worst_case(limits.comments)?.checked_add(times(limits.comments, body)?)?;
    let dependencies = Set::<u64>::worst_case(limits.dependencies)?;
    let pull = name
        .checked_mul(2)?
        .checked_add(Set::<u64>::worst_case(limits.users)?)?
        .checked_add(List::<Kept>::worst_case(limits.reviews)?)?
        .checked_add(times(limits.reviews, body)?)?;
    u64::from(limits.title_bytes)
        .checked_add(body)?
        .checked_add(labels)?
        .checked_add(comments)?
        .checked_add(dependencies)?
        .checked_add(pull)
}

/// What one commit's tree owns.
fn tree(limits: &Limits) -> Option<u64> {
    let file = u64::from(limits.name_bytes).checked_add(u64::from(limits.content_bytes))?;
    Map::<Box<[u8]>, Box<[u8]>>::worst_case(limits.files)?.checked_add(times(limits.files, file)?)
}

/// The most an answer held for a call owns.
fn answer(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let body = u64::from(limits.body_bytes);
    let content = u64::from(limits.content_bytes);
    let summary = summary(limits)?;
    let listed = summary.checked_add(u64::try_from(size_of::<Summary>()).ok()?)?;
    let comment = body.checked_add(u64::try_from(size_of::<Comment>()).ok()?)?;
    let review = body.checked_add(u64::try_from(size_of::<Review>()).ok()?)?;
    let status = name.checked_add(u64::try_from(size_of::<StatusView>()).ok()?)?;
    let file = name.checked_add(content)?.checked_add(u64::try_from(size_of::<File>()).ok()?)?;
    let named = name.checked_add(u64::try_from(size_of::<PageName>()).ok()?)?;
    let head = name.checked_add(u64::try_from(size_of::<Head>()).ok()?)?;
    let items = times(limits.page_size, listed)?;
    let item = summary.checked_add(times(limits.page_size, comment)?)?;
    let number = u64::try_from(size_of::<u64>()).ok()?;
    let pull = name
        .checked_mul(2)?
        .checked_add(times(limits.users, number)?)?
        .checked_add(times(limits.reviews, review)?)?
        .checked_add(times(limits.contexts, status)?)?;
    let dependencies = times(limits.dependencies, number)?;
    let labels = names(limits.labels, limits)?;
    let tree = times(limits.files, file)?;
    let pages = times(limits.page_size, named)?.checked_add(name)?;
    let page = name.checked_add(content)?;
    let cloned = name.checked_add(times(limits.branches, head)?)?;
    Some(items.max(item).max(pull).max(tree).max(pages).max(page).max(cloned).max(dependencies).max(labels))
}

/// The most a call within the limits brings: the repository's name, and
/// the largest of an issue's text and labels, a pull request's text and
/// branches, a wiki page, a set of numbers, or a working tree's commit.
fn call(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let number = u64::try_from(size_of::<u64>()).ok()?;
    let issue = summary(limits)?;
    let pull =
        u64::from(limits.title_bytes).checked_add(u64::from(limits.body_bytes))?.checked_add(name.checked_mul(2)?)?;
    let page = name.checked_add(u64::from(limits.content_bytes))?;
    let numbers = times(limits.users.max(limits.dependencies), number)?;
    let file =
        name.checked_add(u64::from(limits.content_bytes))?.checked_add(u64::try_from(size_of::<File>()).ok()?)?;
    let commit = times(limits.files, file)?;
    name.checked_add(issue.max(pull).max(page).max(numbers).max(commit))
}

/// What a summary of an item owns: its text and its labels.
fn summary(limits: &Limits) -> Option<u64> {
    u64::from(limits.title_bytes).checked_add(u64::from(limits.body_bytes))?.checked_add(names(limits.labels, limits)?)
}

/// The most one observation owns.
fn observation(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let body = u64::from(limits.body_bytes);
    // A pull request opened names its repository, head and base.
    let opened = name.checked_mul(3)?.checked_add(summary(limits)?)?;
    let posted = name.checked_add(body)?;
    let labelled = name.checked_add(names(limits.labels, limits)?)?;
    let wiki = name.checked_mul(2)?.checked_add(u64::from(limits.content_bytes))?;
    let number = u64::try_from(size_of::<u64>()).ok()?;
    let depends = name.checked_add(times(limits.dependencies, number)?)?;
    let requested = name.checked_add(times(limits.users, number)?)?;
    Some(opened.max(posted).max(labelled).max(wiki).max(depends).max(requested).max(name.checked_mul(2)?))
}

/// What a boxed list of `count` names owns.
fn names(count: u32, limits: &Limits) -> Option<u64> {
    times(count, u64::from(limits.name_bytes).checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?)
}

fn times(count: u32, each: u64) -> Option<u64> {
    u64::from(count).checked_mul(each)
}
