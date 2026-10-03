//! The records that cross the boundary with the notes' parent, the engine's
//! root domain (programming-model.md, 4.5), which routes them to and
//! from the forge child domain, the brief child domain and runs' relayed calls.
//! The notes child domain defines them; its parent depends on it.
//!
//! Two shapes cross it. Calls in: an [`Event::Index`], [`Event::Search`],
//! [`Event::Recall`] or [`Event::Note`] carries a `reply_to` and is answered
//! by exactly one record back that echoes it: [`Request::Indexed`],
//! [`Request::Found`], [`Request::Recalled`] or [`Request::Noted`], or
//! [`Request::Refused`] at the entrance. And wiki operations out: a
//! [`Request::List`], [`Request::Fetch`], [`Request::Create`],
//! [`Request::Edit`] or [`Request::Delete`] is ended by exactly one terminal
//! event that echoes its `owner`: [`Event::Listed`] for a list,
//! [`Event::Fetched`] for a fetch, [`Event::Wrote`] for a write. The parent
//! bounds each operation's time, and says when it failed.
//!
//! Hints come in without an answer: [`Event::Refresh`] when a scope's pages
//! may have changed, [`Event::Changed`] when one of them did. A hint about a
//! scope the notes do not keep changes nothing.
//!
//! Pages are typed here: the protocol layer decodes a wiki page (its first
//! line the description, then a typed block of who wrote it and what it
//! refers to, then the body) and encodes one back, and maps a scope and a
//! name to the page's path in the right wiki.

use alloc::boxed::Box;

use skein_lib::{ReplyTo, Token};

/// Where notes are kept (engine-domain.md, section 10): the deployment's, in
/// the wiki of its home repository; a repository's, in its wiki, by its place
/// in the deployment's list; a goal's, under the goal's path in its item's
/// repository's wiki.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Scope {
    Deployment,
    Repository(u32),
    Goal { repository: u32, number: u64 },
}

/// The scopes of a run's notes: the deployment's, its repository's, and its
/// goal's if it has one, in the goal's repository, which may be another.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Scopes {
    pub repository: u32,
    pub goal: Option<Item>,
}

/// An issue or pull request of one of the deployment's repositories: the
/// repository's index in the deployment's list, and the item's number there.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Item {
    pub repository: u32,
    pub number: u64,
}

/// Who wrote an entry: a person, or a run and the item it ran for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Author {
    Person(u64),
    Run { repository: u32, number: u64 },
}

/// An item an entry refers to, closed ones included.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Reference {
    pub repository: u32,
    pub number: u64,
}

/// An entry's page: its one-line description, who wrote it, what it refers
/// to, and its body.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub description: Box<[u8]>,
    pub author: Author,
    pub references: Box<[Reference]>,
    pub body: Box<[u8]>,
}

/// A page of a scope's list, by its name and the revision it is at.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Listed {
    pub name: Box<[u8]>,
    pub revision: u64,
}

/// One line of an index: an entry, less its body.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Line {
    pub scope: Scope,
    pub name: Box<[u8]>,
    pub description: Box<[u8]>,
    pub author: Author,
    pub references: Box<[Reference]>,
}

/// An entry recalled, as its page was when it was read, at `revision`: what
/// a run revising it names.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Entry {
    pub scope: Scope,
    pub name: Box<[u8]>,
    pub revision: u64,
    pub page: Page,
}

/// What a run recalls: an entry by its name, or those of its scopes whose
/// descriptions hold `query`, the first `most` of them.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Recall {
    Name { scope: Scope, name: Box<[u8]> },
    Search { scopes: Scopes, query: Box<[u8]>, most: u32 },
}

/// What a `note` write does to an entry. A revision names the revision the
/// run recalled: the notes read the page afresh first, and refuse it as
/// moved if a person, or another run, wrote it since, since the forge is
/// right. The wiki has no conditional edit, so a write made between that
/// read and the edit is still overwritten: the window is short, not closed.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    New(Page),
    Revise { page: Page, revision: u64 },
    Remove,
}

/// How a `note` write went.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Noted {
    Done,
    /// A revised or removed entry has no page.
    Missing,
    /// A new entry's page is there already.
    Exists,
    /// A revised entry's page was written since the revision the run
    /// recalled: nothing was written.
    Moved,
    /// The wiki could not be written.
    Unavailable,
}

/// Why a call was refused at the entrance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// No room for one more call, or for the scopes it needs kept.
    Busy,
    /// A name, a query or a page past the limits.
    Oversized,
}

/// How a page's read ended.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Fetched {
    Page {
        revision: u64,
        page: Page,
    },
    /// There is no such page.
    Gone,
    Failed,
}

/// How a page's write ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Wrote {
    /// The page is at `revision`, or gone if it was deleted.
    Done {
        revision: u64,
    },
    /// An edited or deleted page is not there.
    Missing,
    /// A created page is there already.
    Exists,
    Failed,
}

/// parent -> notes
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The index of the notes in `scopes`, narrowest scope first, as many
    /// lines as fit `budget` bytes of names and descriptions. Answered by
    /// exactly one `Indexed`, or `Refused`.
    Index { reply_to: ReplyTo, scopes: Scopes, budget: u32 },
    /// The lines of `scopes` whose descriptions hold `query`, the first
    /// `most`. Answered by exactly one `Found`, or `Refused`.
    Search { reply_to: ReplyTo, scopes: Scopes, query: Box<[u8]>, most: u32 },
    /// A run's `recall`, read afresh from the wiki as it is when the read is
    /// served. A recall is not ordered after a note the run is writing: a
    /// run recalls what it noted once its `Noted` came. Answered by exactly
    /// one `Recalled`, or `Refused`.
    Recall { reply_to: ReplyTo, recall: Recall },
    /// A `note` write, the rules having passed it. Answered by exactly one
    /// `Noted`, or `Refused`.
    Note { reply_to: ReplyTo, scope: Scope, name: Box<[u8]>, change: Change },
    /// The pages of `scope` may have changed: they are listed again, if the
    /// scope is kept.
    Refresh { scope: Scope },
    /// The page `name` of `scope` changed (made, edited or deleted): it is
    /// read again, if the scope is kept.
    Changed { scope: Scope, name: Box<[u8]> },
    /// Terminal for `List`: the scope's pages, or `None` if they could not be
    /// listed. The parent's contract: at most the limits' `entries` pages, the
    /// protocol layer cutting a longer list there.
    Listed { owner: Token, pages: Option<Box<[Listed]>> },
    /// Terminal for `Fetch`. The parent's contract: a page within the limits'
    /// description, body and references, the protocol layer cutting a
    /// longer one (a page the notes find past them is left out all the
    /// same).
    Fetched { owner: Token, fetched: Fetched },
    /// Terminal for `Create`, `Edit` and `Delete`.
    Wrote { owner: Token, wrote: Wrote },
}

/// notes -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// The index for the `Index` of `reply_to`: the lines that fit, how many
    /// more there are, and how many of its scopes' pages could not be listed
    /// yet.
    Indexed {
        reply_to: ReplyTo,
        lines: Box<[Line]>,
        more: u32,
        unread: u32,
    },
    /// The lines the `Search` of `reply_to` found, how many more matched, and
    /// how many of its scopes' pages could not be listed yet.
    Found {
        reply_to: ReplyTo,
        lines: Box<[Line]>,
        more: u32,
        unread: u32,
    },
    /// The entries the `Recall` of `reply_to` read, and how many of the pages
    /// it wanted could not be read. An entry with no page is left out.
    Recalled {
        reply_to: ReplyTo,
        entries: Box<[Entry]>,
        failed: u32,
    },
    /// How the `Note` of `reply_to` went.
    Noted {
        reply_to: ReplyTo,
        noted: Noted,
    },
    /// The call of `reply_to` was refused at the entrance.
    Refused {
        reply_to: ReplyTo,
        refusal: Refusal,
    },
    /// List the pages of `scope`.
    List {
        owner: Token,
        scope: Scope,
    },
    /// Read the page `name` of `scope`.
    Fetch {
        owner: Token,
        scope: Scope,
        name: Box<[u8]>,
    },
    /// Write the page `name` of `scope`: make it, only if it is not there;
    /// edit it, only if it is; delete it.
    Create {
        owner: Token,
        scope: Scope,
        name: Box<[u8]>,
        page: Page,
    },
    /// Edit the page `name` of `scope`, only if it is still at `revision`,
    /// as it was read when the run's revision of it was checked.
    Edit {
        owner: Token,
        scope: Scope,
        name: Box<[u8]>,
        page: Page,
        revision: u64,
    },
    Delete {
        owner: Token,
        scope: Scope,
        name: Box<[u8]>,
    },
}
