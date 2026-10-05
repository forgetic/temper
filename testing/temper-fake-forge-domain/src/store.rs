//! The store: repositories and what they hold (testing.md, 4.2).
//!
//! A repository has its users' permissions, its labels, its items (issues and
//! pull requests, numbered together from one), its branches and the commits
//! it has, the statuses on those commits, and its wiki. Every container is
//! bounded by the limits, and one more of anything is refused as full.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{List, Map, Set, Time};

use crate::api::{
    Check, Checks, Comment as CommentView, Error, Kind, Permission, Protection, Review, State, Summary, What,
};
use crate::limits::{self, Limits};

/// A repository on the forge.
#[derive(Debug)]
pub(crate) struct Repository {
    pub(crate) name: Box<[u8]>,
    pub(crate) default: Box<[u8]>,
    pub(crate) styles: Styles,
    /// The users with a permission on it.
    pub(crate) permissions: Map<u64, Permission>,
    /// The labels defined.
    pub(crate) labels: Set<Box<[u8]>>,
    pub(crate) items: Map<u64, Item>,
    /// The items by when they were last updated, for listings.
    pub(crate) recent: Set<(Time, u64)>,
    /// The item each comment is on, by the comment's id.
    pub(crate) comments: Map<u64, u64>,
    /// The last number given to an item.
    pub(crate) numbered: u64,
    pub(crate) branches: Map<Box<[u8]>, u64>,
    /// The commits it has: what its branches reach, and what was pushed or
    /// merged.
    pub(crate) has: Set<u64>,
    /// The statuses on its commits, by commit and context.
    pub(crate) statuses: Map<u64, Map<Box<[u8]>, Status>>,
    pub(crate) wiki: Map<Box<[u8]>, Page>,
    /// The last revision given to a wiki page.
    pub(crate) revisions: u64,
    pub(crate) checks: Checks,
    pub(crate) protection: Option<Protection>,
    /// Whether a subscriber hears of its changes.
    pub(crate) hooked: bool,
    pub(crate) reachable: bool,
    pub(crate) refusing: bool,
}

/// An issue, or a pull request.
#[derive(Debug)]
pub(crate) struct Item {
    pub(crate) title: Box<[u8]>,
    pub(crate) body: Box<[u8]>,
    pub(crate) author: u64,
    pub(crate) state: State,
    pub(crate) labels: Set<Box<[u8]>>,
    pub(crate) comments: Map<u64, Comment>,
    /// The items of the same repository it depends on.
    pub(crate) dependencies: Set<u64>,
    pub(crate) created: Time,
    pub(crate) updated: Time,
    pub(crate) pull: Option<Pull>,
}

#[derive(Debug)]
pub(crate) struct Comment {
    pub(crate) author: u64,
    pub(crate) body: Box<[u8]>,
    pub(crate) created: Time,
    pub(crate) edited: Option<Time>,
}

/// What a pull request adds to an item.
#[derive(Debug)]
pub(crate) struct Pull {
    pub(crate) head: Box<[u8]>,
    pub(crate) base: Box<[u8]>,
    /// The head commit: where the head branch is, while the pull request is
    /// open and the branch is there.
    pub(crate) commit: u64,
    /// The commit its merge made.
    pub(crate) merged: Option<u64>,
    /// The users asked to review it who have not yet.
    pub(crate) requested: Set<u64>,
    /// Its reviews, in the order they were started.
    pub(crate) reviews: List<Kept>,
}

/// A review, and whether it is pending: its author has not submitted it, and
/// no read shows it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Kept {
    pub(crate) review: Review,
    pub(crate) pending: bool,
}

/// A context's status on a commit, and whether CI ran it again.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Status {
    pub(crate) state: Check,
    pub(crate) author: u64,
    pub(crate) at: Time,
    pub(crate) rerun: bool,
}

/// A wiki page.
#[derive(Debug)]
pub(crate) struct Page {
    pub(crate) content: Box<[u8]>,
    pub(crate) author: u64,
    pub(crate) revision: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Styles {
    pub(crate) merge: bool,
    pub(crate) rebase: bool,
    pub(crate) squash: bool,
}

impl Repository {
    /// An empty repository named `name`, its default branch at `first`.
    pub(crate) fn new(
        limits: &Limits,
        name: Box<[u8]>,
        default: Box<[u8]>,
        first: u64,
        checks: Checks,
        protection: Option<Protection>,
        hooked: bool,
    ) -> Repository {
        let mut repository = Repository {
            name,
            default,
            styles: Styles { merge: true, rebase: true, squash: true },
            permissions: Map::with_capacity(limits.users),
            labels: Set::with_capacity(limits.labels),
            items: Map::with_capacity(limits.items),
            recent: Set::with_capacity(limits.items),
            comments: Map::with_capacity(limits::comments(limits).unwrap_or(u32::MAX)),
            numbered: 0,
            branches: Map::with_capacity(limits.branches),
            has: Set::with_capacity(limits.commits),
            statuses: Map::with_capacity(limits.statuses),
            wiki: Map::with_capacity(limits.pages),
            revisions: 0,
            checks,
            protection,
            hooked,
            reachable: true,
            refusing: false,
        };
        let default = copy_of(&repository.default);
        repository.branches.insert(default, first).expect("a repository has room for its default branch");
        repository.has.insert(first).expect("a repository has room for its first commit");
        repository
    }

    /// The permission of `user`.
    pub(crate) fn permission(&self, user: u64) -> Permission {
        match self.permissions.get(&user) {
            Some(&permission) => permission,
            None => Permission::None,
        }
    }

    /// Refuses `user` unless they have at least `needed`.
    pub(crate) fn require(&self, user: u64, needed: Permission) -> Result<(), Error> {
        if self.permission(user) < needed {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    pub(crate) fn item(&self, number: u64) -> Result<&Item, Error> {
        self.items.get(&number).ok_or(Error::Missing(What::Item))
    }

    pub(crate) fn item_mut(&mut self, number: u64) -> Result<&mut Item, Error> {
        self.items.get_mut(&number).ok_or(Error::Missing(What::Item))
    }

    /// Whether `branch` is protected.
    pub(crate) fn is_protected(&self, branch: &[u8]) -> bool {
        match &self.protection {
            Some(protection) => *protection.branch == *branch,
            None => false,
        }
    }

    /// Marks the item `number` updated at `now`.
    pub(crate) fn touch(&mut self, number: u64, now: Time) {
        let item = self.items.get_mut(&number).expect("an item to touch");
        let was = item.updated;
        item.updated = now;
        let removed = self.recent.remove(&(was, number));
        assert!(removed, "the listing index holds every item");
        self.recent.insert((now, number)).expect("an item already listed fits again");
    }

    /// Refuses another item when the repository holds as many as it may.
    pub(crate) fn room(&self) -> Result<(), Error> {
        if self.items.len() >= self.items.capacity() {
            return Err(Error::Full);
        }
        Ok(())
    }

    /// Takes in the item `item` under the next number: there is room for it.
    pub(crate) fn number(&mut self, item: Item) -> u64 {
        let number = self.numbered.checked_add(1).expect("numbers do not run out");
        let created = item.created;
        self.items.insert(number, item).expect("checked for room before");
        self.recent.insert((created, number)).expect("the listing index has room for every item");
        self.numbered = number;
        number
    }

    /// The open pull requests whose head or base branch is `branch`.
    pub(crate) fn on_branch(&self, branch: &[u8]) -> List<u64> {
        let mut numbers = List::with_capacity(self.items.len());
        for (&number, item) in &self.items {
            if let Some(pull) = &item.pull
                && item.state == State::Open
                && (*pull.head == *branch || *pull.base == *branch)
            {
                numbers.push(number).expect("a list as long as the items");
            }
        }
        numbers
    }

    /// The open pull requests whose head branch is `branch`.
    pub(crate) fn following(&self, branch: &[u8]) -> List<u64> {
        let mut numbers = List::with_capacity(self.items.len());
        for (&number, item) in &self.items {
            if let Some(pull) = &item.pull
                && item.state == State::Open
                && *pull.head == *branch
            {
                numbers.push(number).expect("a list as long as the items");
            }
        }
        numbers
    }

    /// The open pull request that merges `head` into `base`, if there is
    /// one.
    pub(crate) fn open_pull(&self, head: &[u8], base: &[u8]) -> Option<u64> {
        for (&number, item) in &self.items {
            if let Some(pull) = &item.pull
                && item.state == State::Open
                && *pull.head == *head
                && *pull.base == *base
            {
                return Some(number);
            }
        }
        None
    }

    /// The newest pull request, open or not, that merges `head` into `base`,
    /// if there is one.
    pub(crate) fn newest_pull(&self, head: &[u8], base: &[u8]) -> Option<u64> {
        let mut newest = None;
        for (&number, item) in &self.items {
            if let Some(pull) = &item.pull
                && *pull.head == *head
                && *pull.base == *base
            {
                newest = Some(number);
            }
        }
        newest
    }

    /// Whether `commit` is a branch's head or an open pull request's.
    pub(crate) fn is_head(&self, commit: u64) -> bool {
        for (_, &at) in &self.branches {
            if at == commit {
                return true;
            }
        }
        for (_, item) in &self.items {
            if let Some(pull) = &item.pull
                && item.state == State::Open
                && pull.commit == commit
            {
                return true;
            }
        }
        false
    }

    /// The open pull requests whose head is `commit`.
    pub(crate) fn heads(&self, commit: u64) -> List<u64> {
        let mut numbers = List::with_capacity(self.items.len());
        for (&number, item) in &self.items {
            if let Some(pull) = &item.pull
                && item.state == State::Open
                && pull.commit == commit
            {
                numbers.push(number).expect("a list as long as the items");
            }
        }
        numbers
    }

    /// The labels `labels` as a set, each of them defined, or why not.
    pub(crate) fn label_set(&self, limits: &Limits, labels: Box<[Box<[u8]>]>) -> Result<Set<Box<[u8]>>, Error> {
        let mut set = Set::with_capacity(limits.labels);
        for label in labels {
            if !self.labels.contains(&*label) {
                return Err(Error::Missing(What::Label));
            }
            if set.insert(label).is_err() {
                return Err(Error::TooLarge);
            }
        }
        Ok(set)
    }
}

impl Item {
    pub(crate) fn kind(&self) -> Kind {
        match self.pull {
            Some(_) => Kind::Pull,
            None => Kind::Issue,
        }
    }

    /// How a listing shows the item `number`.
    pub(crate) fn summary(&self, number: u64) -> Summary {
        Summary {
            number,
            kind: self.kind(),
            state: self.state,
            title: copy_of(&self.title),
            body: copy_of(&self.body),
            author: self.author,
            labels: names(&self.labels),
            created: self.created,
            updated: self.updated,
        }
    }

    pub(crate) fn has_labels(&self, labels: &[Box<[u8]>]) -> bool {
        for label in labels {
            if !self.labels.contains(&**label) {
                return false;
            }
        }
        true
    }
}

impl Comment {
    pub(crate) fn view(&self, id: u64) -> CommentView {
        CommentView { id, author: self.author, body: copy_of(&self.body), created: self.created, edited: self.edited }
    }
}

/// A copy of the numbers in `set`, in their order.
pub(crate) fn numbers(set: &Set<u64>) -> Box<[u64]> {
    let mut numbers = List::with_capacity(set.len());
    for &number in set {
        numbers.push(number).expect("a list as long as the set");
    }
    numbers.into_boxed()
}

/// A copy of the names in `set`, in their order.
pub(crate) fn names(set: &Set<Box<[u8]>>) -> Box<[Box<[u8]>]> {
    let mut names = List::with_capacity(set.len());
    for name in set {
        names.push(copy_of(name)).expect("a list as long as the set");
    }
    names.into_boxed()
}

/// Refuses `bytes` past `limit`.
pub(crate) fn fits(bytes: &[u8], limit: u32) -> Result<(), Error> {
    match u32::try_from(bytes.len()) {
        Ok(len) if len <= limit => Ok(()),
        Ok(_) | Err(_) => Err(Error::TooLarge),
    }
}

/// Refuses a list of numbers longer than `count`.
pub(crate) fn fit_numbers(numbers: &[u64], count: u32) -> Result<(), Error> {
    match u32::try_from(numbers.len()) {
        Ok(len) if len <= count => Ok(()),
        Ok(_) | Err(_) => Err(Error::TooLarge),
    }
}

/// Refuses a list of names longer than `count`, or holding a name past the
/// limit.
pub(crate) fn fit_names(names: &[Box<[u8]>], count: u32, limits: &Limits) -> Result<(), Error> {
    match u32::try_from(names.len()) {
        Ok(len) if len <= count => {}
        Ok(_) | Err(_) => return Err(Error::TooLarge),
    }
    for name in names {
        fits(name, limits.name_bytes)?;
    }
    Ok(())
}
